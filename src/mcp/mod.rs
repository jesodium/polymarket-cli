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

use std::io::{BufRead, Write};

use anyhow::Result;
use serde_json::{Value, json};

/// MCP protocol revision we implement against.
const PROTOCOL_VERSION: &str = "2025-06-18";

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
                let info = msg.get("params").and_then(|p| p.get("clientInfo"));
                status.set_client(
                    info.and_then(|i| i.get("name")).and_then(Value::as_str),
                    info.and_then(|i| i.get("version")).and_then(Value::as_str),
                );
                write_message(&mut out, &success(id, initialize_result()))?;
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
            Ok((true, body)) => tool_text(&body),
            Ok((false, body)) => tool_error(&body),
            Err(e) => tool_error(&format!("Failed to run command: {e}")),
        },
        Err(e) => tool_error(&e.to_string()),
    }
}

/// Re-invoke this binary with `--output json` and the mapped subcommand,
/// capturing its stdout. Returns `(success, body)` where `body` is stdout
/// (the command's JSON), falling back to stderr when stdout is empty.
fn run_subcommand(argv: &[String]) -> Result<(bool, String)> {
    let exe = std::env::current_exe()?;
    let output = std::process::Command::new(exe)
        .arg("--output")
        .arg("json")
        .args(argv)
        .output()?;

    let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let body = if stdout.is_empty() {
        String::from_utf8_lossy(&output.stderr).trim().to_string()
    } else {
        stdout
    };
    Ok((output.status.success(), body))
}

fn tool_text(text: &str) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": false})
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

fn initialize_result() -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {"tools": {}},
        "serverInfo": {
            "name": "polymarket-cli",
            "version": env!("CARGO_PKG_VERSION"),
        },
    })
}
