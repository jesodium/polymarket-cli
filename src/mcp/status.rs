//! Best-effort status file for the MCP server.
//!
//! The server is spawned by an MCP client over stdio, so it runs in a separate
//! process from the TUI. To let the Settings tab show whether a client is
//! connected, the server records lifecycle events to a small JSON file
//! (`~/.config/polymarket/mcp-status.json`); the TUI reads it on render.
//!
//! Every write here is best-effort: a failure to persist status must never
//! interfere with the protocol stream, so persistence errors are swallowed.

use std::fs;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

const FILE_NAME: &str = "mcp-status.json";

/// Activity within this window counts as a live, connected session.
const RECENT_SECS: i64 = 120;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub(crate) struct McpStatus {
    /// "running" (started, no client yet) | "connected" | "stopped".
    pub state: String,
    pub pid: u32,
    pub started_at: Option<DateTime<Utc>>,
    pub last_activity: Option<DateTime<Utc>>,
    pub stopped_at: Option<DateTime<Utc>>,
    pub tool_calls: u64,
    pub last_tool: Option<String>,
    pub client_name: Option<String>,
    pub client_version: Option<String>,
}

pub(crate) fn path() -> Option<PathBuf> {
    crate::config::config_dir().ok().map(|d| d.join(FILE_NAME))
}

/// Read the current status, if any. Returns `None` when the file is missing or
/// unreadable — callers treat that as "never run".
pub(crate) fn load() -> Option<McpStatus> {
    let data = fs::read_to_string(path()?).ok()?;
    serde_json::from_str(&data).ok()
}

impl McpStatus {
    /// Initialise a fresh "running" record for this process and persist it.
    pub(crate) fn start() -> Self {
        let now = Utc::now();
        let s = Self {
            state: "running".into(),
            pid: std::process::id(),
            started_at: Some(now),
            last_activity: Some(now),
            ..Self::default()
        };
        s.persist();
        s
    }

    /// Record the connecting client (from `initialize`'s `clientInfo`).
    pub(crate) fn set_client(&mut self, name: Option<&str>, version: Option<&str>) {
        self.state = "connected".into();
        self.client_name = name.map(ToString::to_string);
        self.client_version = version.map(ToString::to_string);
        self.last_activity = Some(Utc::now());
        self.persist();
    }

    /// Record a `tools/call` dispatch.
    pub(crate) fn record_call(&mut self, tool: &str) {
        self.tool_calls += 1;
        self.last_tool = Some(tool.to_string());
        self.last_activity = Some(Utc::now());
        self.persist();
    }

    /// Mark a clean shutdown (client closed the pipe).
    pub(crate) fn stop(&mut self) {
        self.state = "stopped".into();
        self.stopped_at = Some(Utc::now());
        self.persist();
    }

    /// Whether the last activity is recent enough to call the session live.
    pub(crate) fn is_recent(&self) -> bool {
        self.state != "stopped"
            && self
                .last_activity
                .is_some_and(|t| (Utc::now() - t).num_seconds() <= RECENT_SECS)
    }

    fn persist(&self) {
        let Some(path) = path() else { return };
        if let Some(dir) = path.parent() {
            let _ = fs::create_dir_all(dir);
        }
        if let Ok(json) = serde_json::to_string_pretty(self) {
            let _ = fs::write(&path, json);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recent_when_active_and_not_stopped() {
        let s = McpStatus {
            state: "connected".into(),
            last_activity: Some(Utc::now()),
            ..McpStatus::default()
        };
        assert!(s.is_recent());
    }

    #[test]
    fn not_recent_when_stopped() {
        let s = McpStatus {
            state: "stopped".into(),
            last_activity: Some(Utc::now()),
            ..McpStatus::default()
        };
        assert!(!s.is_recent());
    }

    #[test]
    fn not_recent_when_stale() {
        let s = McpStatus {
            state: "connected".into(),
            last_activity: Some(Utc::now() - chrono::Duration::seconds(RECENT_SECS + 10)),
            ..McpStatus::default()
        };
        assert!(!s.is_recent());
    }
}
