//! Notification events for background activity (guard exits, failures).
//!
//! Every event is appended to `~/.config/polymarket/events.jsonl` and
//! (optionally) surfaced as an OS notification. The JSONL file is the bridge
//! to agentic clients: the `guard_events` MCP tool and `guard events` command
//! read it, so an AI agent can poll and relay to whatever channel it likes.
//!
//! Best-effort throughout: a notification failure must never break trading.

use std::fs;
use std::io::Write;
use std::process::{Command, Stdio};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "events.jsonl";
// ponytail: append-only, no rotation — guard fires are rare (each fire clears
// its guard). Add rotation if some future emitter gets chatty.

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Event {
    pub ts: DateTime<Utc>,
    /// e.g. "guard-exit", "guard-exit-failed", "guard-dropped".
    pub kind: String,
    /// "paper" or "live".
    pub mode: String,
    pub token_id: String,
    pub message: String,
}

/// Record an event and, when `toast` is set, pop an OS notification.
pub(crate) fn emit(kind: &str, live: bool, token_id: &str, message: &str, toast: bool) {
    let ev = Event {
        ts: Utc::now(),
        kind: kind.to_string(),
        mode: if live { "live" } else { "paper" }.to_string(),
        token_id: token_id.to_string(),
        message: message.to_string(),
    };
    append(&ev);
    if toast {
        notify_os("Fiberglass", message);
    }
}

fn append(ev: &Event) {
    let Ok(dir) = crate::config::config_dir() else {
        return;
    };
    let _ = fs::create_dir_all(&dir);
    let Ok(json) = serde_json::to_string(ev) else {
        return;
    };
    if let Ok(mut f) = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(FILE_NAME))
    {
        let _ = writeln!(f, "{json}");
    }
}

/// The most recent `limit` events, oldest first.
pub(crate) fn recent(limit: usize) -> Vec<Event> {
    let Ok(dir) = crate::config::config_dir() else {
        return Vec::new();
    };
    let Ok(data) = fs::read_to_string(dir.join(FILE_NAME)) else {
        return Vec::new();
    };
    let all: Vec<Event> = data
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .collect();
    let skip = all.len().saturating_sub(limit);
    all.into_iter().skip(skip).collect()
}

/// Desktop notification: osascript on macOS, notify-send on Linux, silent
/// no-op elsewhere.
pub(crate) fn notify_os(title: &str, body: &str) {
    #[cfg(target_os = "macos")]
    {
        let script = format!(
            "display notification \"{}\" with title \"{}\"",
            applescript_escape(body),
            applescript_escape(title),
        );
        let _ = Command::new("osascript")
            .args(["-e", &script])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("notify-send")
            .args([title, body])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(target_os = "windows")]
    {
        // WinRT toast via PowerShell — no crate, works on Win 10/11. First run
        // registers an AppUserModelID under HKCU so the toast is titled
        // "Fiberglass" instead of "Windows PowerShell"; registry-only AUMIDs
        // are honored by unpackaged apps since Win 10 1703. Idempotent.
        let script = format!(
            "$aumid = 'Fiberglass.CLI'; \
             $reg = 'HKCU:\\Software\\Classes\\AppUserModelId\\' + $aumid; \
             if (-not (Test-Path $reg)) {{ \
                 New-Item -Path $reg -Force | Out-Null; \
                 New-ItemProperty -Path $reg -Name DisplayName -Value 'Fiberglass' -PropertyType String -Force | Out-Null \
             }}; \
             [Windows.UI.Notifications.ToastNotificationManager, Windows.UI.Notifications, ContentType = WindowsRuntime] | Out-Null; \
             $t = [Windows.UI.Notifications.ToastNotificationManager]::GetTemplateContent([Windows.UI.Notifications.ToastTemplateType]::ToastText02); \
             $x = $t.GetElementsByTagName('text'); \
             $x.Item(0).AppendChild($t.CreateTextNode('{}')) | Out-Null; \
             $x.Item(1).AppendChild($t.CreateTextNode('{}')) | Out-Null; \
             [Windows.UI.Notifications.ToastNotificationManager]::CreateToastNotifier($aumid).Show([Windows.UI.Notifications.ToastNotification]::new($t))",
            powershell_escape(title),
            powershell_escape(body),
        );
        let _ = Command::new("powershell")
            .args([
                "-NoProfile",
                "-NonInteractive",
                "-WindowStyle",
                "Hidden",
                "-Command",
                &script,
            ])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn();
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
    {
        let _ = (title, body);
    }
}

#[cfg(target_os = "macos")]
fn applescript_escape(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// Single-quoted PowerShell string: only `'` needs doubling.
#[cfg(target_os = "windows")]
fn powershell_escape(s: &str) -> String {
    s.replace('\'', "''")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_returns_last_n_in_order() {
        // Round-trip through a temp config dir via HOME override.
        let dir = std::env::temp_dir().join(format!("events-test-{}", std::process::id()));
        let _ = fs::create_dir_all(&dir);
        let old_home = std::env::var("HOME").ok();
        unsafe { std::env::set_var("HOME", &dir) };

        for i in 0..5 {
            emit("guard-exit", false, "tok", &format!("msg {i}"), false);
        }
        let last = recent(2);

        if let Some(h) = old_home {
            unsafe { std::env::set_var("HOME", h) };
        }
        let _ = fs::remove_dir_all(&dir);

        assert_eq!(last.len(), 2);
        assert_eq!(last[0].message, "msg 3");
        assert_eq!(last[1].message, "msg 4");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn escape_quotes_for_applescript() {
        assert_eq!(applescript_escape(r#"a "b" \c"#), r#"a \"b\" \\c"#);
    }
}
