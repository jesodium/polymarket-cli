//! Model Context Protocol (MCP) server — a fourth entry point alongside the
//! TUI, CLI, and shell.
//!
//! Speaks JSON-RPC 2.0 over a stdio transport (newline-delimited messages).
//! Each `tools/call` is dispatched by re-invoking this same binary with the
//! equivalent subcommand and `--output json`. That means MCP shares the exact
//! code path — and therefore the paper/live behaviour — of the CLI:
//!
//!   * Paper mode is honoured automatically. The global paper toggle
//!     (`paper enable`) routes order tools through the simulator, and the
//!     order tools also accept a per-call `paper` argument.
//!   * Live trading uses the configured wallet (config file or
//!     `POLYMARKET_PRIVATE_KEY`), inherited by the child process, exactly as
//!     the CLI resolves it.
//!
//! Running the child as a subprocess keeps the parent's stdout reserved for
//! the protocol stream while the child's stdout (the command's JSON) is
//! captured and returned as the tool result.

pub(crate) mod status;
mod tools;

use std::io::{BufRead, Read, Write};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use anyhow::Result;
use serde_json::{Value, json};

/// MCP protocol revision we implement against, and the default we advertise
/// when a client doesn't request one (or requests an unsupported one).
const PROTOCOL_VERSION: &str = "2025-06-18";

/// Protocol revisions we can speak. We echo the client's requested version
/// when it appears here; otherwise we fall back to [`PROTOCOL_VERSION`].
const SUPPORTED_PROTOCOL_VERSIONS: &[&str] = &[PROTOCOL_VERSION];

pub(crate) fn run() -> Result<()> {
    let stdin = std::io::stdin();
    let mut reader = stdin.lock();
    let stdout = std::io::stdout();
    let mut out = stdout.lock();

    // Best-effort liveness record the Settings tab reads (see `status`).
    let mut status = status::McpStatus::start();

    let mut line = String::new();
    loop {
        line.clear();
        if reader.read_line(&mut line)? == 0 {
            status.stop(); // EOF — client closed the pipe.
            break;
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let msg: Value = match serde_json::from_str(trimmed) {
            Ok(v) => v,
            Err(e) => {
                write_message(
                    &mut out,
                    &error_response(Value::Null, -32700, &format!("Parse error: {e}")),
                )?;
                continue;
            }
        };

        // Requests carry an "id"; notifications do not and expect no response.
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();

        match (id, method) {
            (Some(id), "initialize") => {
                let params = msg.get("params");
                let info = params.and_then(|p| p.get("clientInfo"));
                status.set_client(
                    info.and_then(|i| i.get("name")).and_then(Value::as_str),
                    info.and_then(|i| i.get("version")).and_then(Value::as_str),
                );
                let requested = params
                    .and_then(|p| p.get("protocolVersion"))
                    .and_then(Value::as_str);
                write_message(&mut out, &success(id, initialize_result(requested)))?;
            }
            (Some(id), "tools/list") => {
                write_message(
                    &mut out,
                    &success(id, json!({"tools": tools::definitions()})),
                )?;
            }
            (Some(id), "tools/call") => {
                let params = msg.get("params");
                if let Some(name) = params.and_then(|p| p.get("name")).and_then(Value::as_str) {
                    status.record_call(name);
                }
                let result = handle_call(params);
                write_message(&mut out, &success(id, result))?;
            }
            (Some(id), "ping") => write_message(&mut out, &success(id, json!({})))?,
            (Some(id), other) => {
                write_message(
                    &mut out,
                    &error_response(id, -32601, &format!("Method not found: {other}")),
                )?;
            }
            // Notifications (e.g. notifications/initialized) need no reply.
            (None, _) => {}
        }
    }

    Ok(())
}

/// Run a single `tools/call`, returning an MCP tool result. Tool-level
/// failures are reported as `isError` content (not JSON-RPC errors) so the
/// model can read and react to them.
fn handle_call(params: Option<&Value>) -> Value {
    let Some(params) = params else {
        return tool_error("missing params");
    };
    let Some(name) = params.get("name").and_then(Value::as_str) else {
        return tool_error("missing tool name");
    };
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));

    match tools::build_argv(name, &args) {
        Ok(argv) => match run_subcommand(&argv) {
            Ok((true, body)) => tool_success(&body),
            Ok((false, body)) => tool_error(&body),
            Err(e) => tool_error(&format!("Failed to run command: {e}")),
        },
        Err(e) => tool_error(&e.to_string()),
    }
}

/// Hard ceiling on how long a single tool subprocess may run. The MCP loop is
/// single-threaded, so a child that hangs (e.g. a stalled CLOB/Gamma request)
/// would otherwise wedge the whole server. CLI calls normally finish in well
/// under a second; this only fires on a genuine stall.
const SUBCOMMAND_TIMEOUT: Duration = Duration::from_secs(30);

/// Re-invoke this binary with `--output json` and the mapped subcommand,
/// capturing its stdout. Returns `(success, body)` where `body` is stdout
/// (the command's JSON), falling back to stderr when stdout is empty. The
/// child is killed if it exceeds [`SUBCOMMAND_TIMEOUT`].
fn run_subcommand(argv: &[String]) -> Result<(bool, String)> {
    let exe = std::env::current_exe()?;
    let child = Command::new(exe)
        .arg("--output")
        .arg("json")
        .args(argv)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;
    wait_with_timeout(child, SUBCOMMAND_TIMEOUT)
}

/// Wait for `child`, killing it if it outlives `timeout`. stdout/stderr are
/// drained on dedicated threads so a child that fills a pipe buffer can't
/// deadlock against our wait. Returns `(success, body)`; a timeout reports as
/// an unsuccessful call with a `timed out` message.
fn wait_with_timeout(mut child: Child, timeout: Duration) -> Result<(bool, String)> {
    let mut stdout_pipe = child.stdout.take().expect("stdout is piped");
    let mut stderr_pipe = child.stderr.take().expect("stderr is piped");
    let out_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stdout_pipe.read_to_end(&mut buf);
        buf
    });
    let err_reader = std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = stderr_pipe.read_to_end(&mut buf);
        buf
    });

    let deadline = Instant::now() + timeout;
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            // Killing closes the pipes, so the reader threads can finish.
            let _ = out_reader.join();
            let _ = err_reader.join();
            return Ok((
                false,
                format!("command timed out after {}s", timeout.as_secs()),
            ));
        }
        std::thread::sleep(Duration::from_millis(20));
    };

    let stdout = String::from_utf8_lossy(&out_reader.join().unwrap_or_default())
        .trim()
        .to_string();
    let body = if stdout.is_empty() {
        String::from_utf8_lossy(&err_reader.join().unwrap_or_default())
            .trim()
            .to_string()
    } else {
        stdout
    };
    Ok((status.success(), body))
}

/// Successful tool result. Always carries the raw command output as a text
/// block (human-readable, backward compatible); when that output parses as
/// JSON it is also attached as `structuredContent` so MCP clients get typed
/// data without re-parsing a string. The spec requires `structuredContent` to
/// be an object, so non-object JSON (arrays, scalars) is wrapped under a
/// `result` key.
fn tool_success(body: &str) -> Value {
    let mut result = json!({
        "content": [{"type": "text", "text": body}],
        "isError": false,
    });
    if let Ok(parsed) = serde_json::from_str::<Value>(body) {
        let structured = if parsed.is_object() {
            parsed
        } else {
            json!({ "result": parsed })
        };
        result["structuredContent"] = structured;
    }
    result
}

fn tool_error(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": true})
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error_response(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn write_message(out: &mut impl Write, msg: &Value) -> Result<()> {
    serde_json::to_writer(&mut *out, msg)?;
    out.write_all(b"\n")?;
    out.flush()?;
    Ok(())
}

fn initialize_result(requested: Option<&str>) -> Value {
    json!({
        "protocolVersion": negotiate_version(requested),
        "capabilities": {"tools": {}},
        "serverInfo": {
            "name": "fiberglass",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}

/// Pick the protocol version to advertise: the client's requested version when
/// we support it, otherwise our default.
fn negotiate_version(requested: Option<&str>) -> &'static str {
    requested
        .and_then(|v| {
            SUPPORTED_PROTOCOL_VERSIONS
                .iter()
                .copied()
                .find(|&s| s == v)
        })
        .unwrap_or(PROTOCOL_VERSION)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structured_content_for_object() {
        let r = tool_success(r#"{"address":"0xabc","balance":"10"}"#);
        assert_eq!(r["isError"], json!(false));
        assert_eq!(r["structuredContent"]["address"], json!("0xabc"));
        // Text block is preserved for human-readable / legacy clients.
        assert!(r["content"][0]["text"].as_str().unwrap().contains("0xabc"));
    }

    #[test]
    fn structured_content_wraps_array() {
        let r = tool_success(r#"[{"q":"a"},{"q":"b"}]"#);
        assert_eq!(
            r["structuredContent"]["result"].as_array().unwrap().len(),
            2
        );
    }

    #[test]
    fn no_structured_content_for_non_json() {
        let r = tool_success("plain error text, not json");
        assert!(r.get("structuredContent").is_none());
    }

    #[test]
    fn negotiate_version_echoes_supported() {
        assert_eq!(negotiate_version(Some(PROTOCOL_VERSION)), PROTOCOL_VERSION);
        assert_eq!(negotiate_version(Some("1999-01-01")), PROTOCOL_VERSION);
        assert_eq!(negotiate_version(None), PROTOCOL_VERSION);
    }

    fn piped(cmd: &str, arg: &str) -> Child {
        Command::new(cmd)
            .arg(arg)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap()
    }

    #[test]
    fn wait_with_timeout_kills_slow_child() {
        let child = piped("sleep", "10");
        let (ok, body) = wait_with_timeout(child, Duration::from_millis(150)).unwrap();
        assert!(!ok);
        assert!(body.contains("timed out"), "body was: {body}");
    }

    #[test]
    fn wait_with_timeout_returns_output() {
        let child = piped("echo", "hello");
        let (ok, body) = wait_with_timeout(child, Duration::from_secs(5)).unwrap();
        assert!(ok);
        assert_eq!(body, "hello");
    }
}
