//! Background risk worker lifecycle and status helpers.
//!
//! The worker (`fiberglass worker`) is the TUI's two background jobs with no screen: it
//! evaluates TP/SL guards *and* polls copy-trading, and it now brings up the
//! background MCP listener too. `fiberglass start` (and the TUI on launch / an
//! order placement) spawns it detached so those keep running after you close
//! the terminal. Coordination is two small heartbeat files in the config dir:
//!
//! * `guard-worker.json` — worker liveness, read by `status` / `ensure_worker`.
//! * `tui-heartbeat.json` — written by a running TUI; while it's fresh the
//!   worker skips that TUI's guards and stands down copy-trading entirely, so
//!   the two never evaluate a position or mirror a trade twice.
//!
//! "Launch at login" is a macOS LaunchAgent plist; its presence on disk *is*
//! the setting.

use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::process::{Command, Stdio};

use anyhow::{Context, Result, bail};
use chrono::{DateTime, Utc};
use polymarket_client_sdk_v2::types::Decimal;
use serde::{Deserialize, Serialize};

use std::sync::{Arc, Mutex};

use crate::copytrade::engine::CopyEngine;
use crate::guard::{self, Guard, GuardAction};
use crate::output::OutputFormat;
use crate::paper::types::PaperAccount;
use crate::paper::{engine as paper_engine, quotes, store};
use crate::trade::{self, LiveOrder};
use crate::{auth, config};

const WORKER_FILE: &str = "guard-worker.json";
const TUI_FILE: &str = "tui-heartbeat.json";
const LOG_FILE: &str = "guard-worker.log";
pub(crate) const DEFAULT_INTERVAL_SECS: u64 = 5;
/// A heartbeat older than this many intervals counts as dead.
const STALE_TICKS: i64 = 4;

pub(crate) fn print_status(output: OutputFormat) -> Result<()> {
    let worker = load_worker();
    let alive = worker.as_ref().is_some_and(WorkerStatus::is_recent);
    let mcp = crate::mcp::status::load();
    let book = guard::load().unwrap_or_default();
    if let OutputFormat::Json = output {
        println!(
            "{}",
            serde_json::json!({
                "alive": alive,
                "worker": worker,
                "mcp": mcp,
                "guards": book.guards.len(),
                "autostart": autostart_enabled(),
            })
        );
        return Ok(());
    }
    match worker {
        Some(w) if alive => println!(
            "Worker RUNNING (pid {}, {} guard(s), last tick {})",
            w.pid,
            w.guards,
            w.last_tick.map_or_else(|| "—".into(), |t| t.to_rfc3339())
        ),
        Some(_) => println!("Worker not running (stale heartbeat)."),
        None => println!("Worker not running."),
    }
    println!(
        "{} guard(s) armed. Autostart at login: {}.",
        book.guards.len(),
        if autostart_enabled() { "on" } else { "off" }
    );
    match mcp {
        Some(s) if s.state == "listening" => {
            println!("MCP listening on {}.", s.endpoint.unwrap_or_default());
        }
        Some(s) if s.is_recent() => {
            println!("MCP session active ({}, {}).", s.transport, s.state);
        }
        Some(_) => println!("MCP not active."),
        None => println!("MCP not running."),
    }
    println!("Log: {}", config::config_dir()?.join(LOG_FILE).display());
    Ok(())
}

pub(crate) fn print_events(limit: usize, output: OutputFormat) -> Result<()> {
    let events = crate::events::recent(limit);
    if let OutputFormat::Json = output {
        println!("{}", serde_json::to_string_pretty(&events)?);
        return Ok(());
    }
    if events.is_empty() {
        println!("No events yet.");
    }
    for e in &events {
        println!(
            "[{}] {} ({}) {}",
            e.ts.format("%Y-%m-%d %H:%M:%S"),
            e.kind,
            e.mode,
            e.message
        );
    }
    Ok(())
}

// --- heartbeat files --------------------------------------------------------

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct WorkerStatus {
    pub pid: u32,
    pub started_at: DateTime<Utc>,
    pub last_tick: Option<DateTime<Utc>>,
    pub interval_secs: u64,
    pub guards: usize,
}

impl WorkerStatus {
    pub(crate) fn is_recent(&self) -> bool {
        self.last_tick.is_some_and(|t| {
            (Utc::now() - t).num_seconds() <= self.interval_secs as i64 * STALE_TICKS
        })
    }
}

fn worker_path() -> Option<PathBuf> {
    config::config_dir().ok().map(|d| d.join(WORKER_FILE))
}

pub(crate) fn load_worker() -> Option<WorkerStatus> {
    let data = fs::read_to_string(worker_path()?).ok()?;
    serde_json::from_str(&data).ok()
}

pub(crate) fn worker_alive() -> Option<WorkerStatus> {
    load_worker().filter(WorkerStatus::is_recent)
}

/// Best-effort write; a persistence hiccup must never kill the worker.
fn persist_worker(s: &WorkerStatus) {
    let Some(path) = worker_path() else { return };
    if let Some(dir) = path.parent() {
        let _ = fs::create_dir_all(dir);
    }
    if let Ok(json) = serde_json::to_string_pretty(s) {
        let _ = fs::write(&path, json);
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct TuiHeartbeat {
    pub pid: u32,
    /// True when the TUI is in live mode.
    pub live: bool,
    pub last_tick: DateTime<Utc>,
}

fn tui_path() -> Option<PathBuf> {
    config::config_dir().ok().map(|d| d.join(TUI_FILE))
}

/// Called by the TUI's data refresher every few seconds.
pub(crate) fn write_tui_heartbeat(live: bool) {
    let Some(path) = tui_path() else { return };
    let hb = TuiHeartbeat {
        pid: std::process::id(),
        live,
        last_tick: Utc::now(),
    };
    if let Ok(json) = serde_json::to_string(&hb) {
        let _ = fs::write(&path, json);
    }
}

/// `Some(is_live_mode)` when a TUI heartbeat is fresh — that TUI owns guard
/// evaluation for its mode and the worker must leave those guards alone.
fn tui_mode_alive() -> Option<bool> {
    let data = fs::read_to_string(tui_path()?).ok()?;
    let hb: TuiHeartbeat = serde_json::from_str(&data).ok()?;
    ((Utc::now() - hb.last_tick).num_seconds() <= 10).then_some(hb.live)
}

// --- worker lifecycle -------------------------------------------------------

/// Spawn the detached worker if none is alive. Never fails the caller: guard
/// commands and the TUI must work even if the spawn does not.
pub(crate) fn ensure_worker(quiet: bool) {
    if worker_alive().is_some() {
        return;
    }
    match spawn_worker() {
        Ok(()) if !quiet => {
            eprintln!("Background worker started (`fiberglass risk status`).");
        }
        Ok(()) => {}
        Err(e) => eprintln!("Could not start background worker: {e:#}"),
    }
}

/// Top-level `fiberglass start`: spawn the detached daemon that runs TP/SL
/// guards and copy-trading headless. Each guard/follower carries its own
/// paper/live flag, so there's nothing mode-wide to choose here.
pub fn start_daemon() -> Result<()> {
    if let Some(w) = worker_alive() {
        println!(
            "Background worker already running (pid {}). `stop` to halt.",
            w.pid
        );
        return Ok(());
    }
    spawn_worker()?;
    println!(
        "Started headless: TP/SL guards + copy-trading + MCP (each item runs in its own \
         paper/live mode). Survives closing this terminal.\n`risk status` to check, `stop` to halt."
    );
    Ok(())
}

fn spawn_worker() -> Result<()> {
    let exe = std::env::current_exe().context("Could not locate own binary")?;
    let dir = config::config_dir()?;
    fs::create_dir_all(&dir)?;
    let log = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(dir.join(LOG_FILE))?;
    let mut cmd = Command::new(exe);
    cmd.arg("worker");
    cmd.stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.process_group(0); // detach from our session so it survives us
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0000_0208); // DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP
    }
    cmd.spawn().context("Failed to spawn guard worker")?;
    Ok(())
}

/// Kill the background worker. Also the top-level `stop` / `die` command.
pub(crate) fn stop_worker() -> Result<()> {
    let Some(w) = load_worker() else {
        println!("Worker not running.");
        return Ok(());
    };
    // IMPORTANT NOTE: shell out to kill/taskkill — no signal crate for one call.
    #[cfg(unix)]
    let ok = Command::new("kill").arg(w.pid.to_string()).status();
    #[cfg(windows)]
    let ok = Command::new("taskkill")
        .args(["/PID", &w.pid.to_string(), "/F"])
        .status();
    match ok {
        Ok(s) if s.success() => println!(
            "Worker stopped (pid {}). Launching the TUI or placing an order starts it again.",
            w.pid
        ),
        _ => println!("Worker (pid {}) was not running.", w.pid),
    }
    if let Some(p) = worker_path() {
        let _ = fs::remove_file(p);
    }
    crate::mcp::status::clear();
    let armed = guard::load().unwrap_or_default().guards.len();
    if armed > 0 {
        println!("Note: {armed} guard(s) still armed — they will NOT fire while stopped.");
    }
    if autostart_enabled() {
        println!(
            "Autostart is on: the worker returns at next login (`risk autostart off` to disable)."
        );
    }
    Ok(())
}

// --- the worker loop --------------------------------------------------------

pub(crate) async fn run_worker(interval: u64) -> Result<()> {
    if let Some(w) = worker_alive()
        && w.pid != std::process::id()
    {
        bail!("Another worker is already running (pid {})", w.pid);
    }
    let mut status = WorkerStatus {
        pid: std::process::id(),
        started_at: Utc::now(),
        last_tick: None,
        interval_secs: interval,
        guards: 0,
    };
    persist_worker(&status);

    let mcp_endpoint = match crate::mcp::start_background() {
        Ok(endpoint) => {
            eprintln!(
                "[{}] MCP daemon listening on {endpoint}",
                Utc::now().format("%H:%M:%S")
            );
            Some(endpoint)
        }
        Err(e) => {
            eprintln!(
                "[{}] MCP daemon failed to start: {e:#}",
                Utc::now().format("%H:%M:%S")
            );
            None
        }
    };

    // Copy-trading half: only spin it up if the roster actually has followers.
    // Each follower mirrors onto paper or live per its own flag — no mode here.
    let copy_secs = crate::settings::load().copy_poll_secs.max(1);
    let copy = {
        let acct = store::load()?.unwrap_or_else(|| PaperAccount::new(Decimal::ZERO, false));
        let acct = Arc::new(Mutex::new(acct));
        let engine = CopyEngine::new(acct, copy_secs);
        engine.start_all();
        (engine.running_count() > 0).then_some(engine)
    };
    let mut last_copy = Utc::now() - chrono::Duration::seconds(copy_secs as i64);

    println!(
        "[{}] worker up (pid {}, every {interval}s, copy-trading {}, mcp {})",
        Utc::now().format("%Y-%m-%d %H:%M:%S"),
        status.pid,
        copy.as_ref().map_or("off".into(), |c| format!(
            "{} follower(s)",
            c.running_count()
        )),
        mcp_endpoint
            .as_deref()
            .map_or_else(|| "off".to_string(), |e| e.to_string()),
    );

    let clob = auth::unauthenticated_clob_client()?;
    // Highest mark seen per token, for trailing stops.
    let mut peaks: HashMap<String, Decimal> = HashMap::new();

    loop {
        // A running TUI runs its own guards + copy engine; step back so we
        // never evaluate the same position or mirror the same trade twice.
        let tui_mode = tui_mode_alive();
        let book = guard::load().unwrap_or_default();
        let due: Vec<Guard> = book
            .guards
            .iter()
            .filter(|g| tui_mode != Some(g.live))
            .cloned()
            .collect();
        if !due.is_empty()
            && let Err(e) = tick(&clob, &due, &mut peaks).await
        {
            eprintln!("[{}] tick failed: {e:#}", Utc::now().format("%H:%M:%S"));
        }

        // Copy-trading: the TUI owns it whenever one is open. Run on its own
        // cadence, syncing from disk first so guard sells above aren't undone.
        if let Some(copy) = &copy
            && tui_mode.is_none()
            && (Utc::now() - last_copy).num_seconds() >= copy_secs as i64
        {
            copy.reload_account();
            if let Err(e) = copy.poll().await {
                eprintln!(
                    "[{}] copy poll failed: {e:#}",
                    Utc::now().format("%H:%M:%S")
                );
            }
            last_copy = Utc::now();
        }

        status.last_tick = Some(Utc::now());
        status.guards = book.guards.len();
        persist_worker(&status);
        tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
    }
}

/// One evaluation pass over the guards the worker owns this tick.
async fn tick(
    clob: &polymarket_client_sdk_v2::clob::Client,
    due: &[Guard],
    peaks: &mut HashMap<String, Decimal>,
) -> Result<()> {
    let now = Utc::now();

    // Paper guards settle against the paper store on disk.
    let paper_due: Vec<&Guard> = due.iter().filter(|g| !g.live).collect();
    if !paper_due.is_empty()
        && let Some(mut acct) = store::load()?
    {
        for g in paper_due {
            let (free, avg) = match acct.positions.get(&g.token_id) {
                Some(p) => (
                    (p.size - acct.reserved_shares(&g.token_id)).max(Decimal::ZERO),
                    p.avg_price,
                ),
                None => (Decimal::ZERO, Decimal::ZERO),
            };
            let levels = quotes::fetch_book(clob, quotes::parse_token_id(&g.token_id)?).await?;
            let (mid, best_bid) = mid_and_bid(&levels);
            match guard::evaluate(g, free, avg, mid, best_bid, peaks) {
                GuardAction::Hold => {}
                GuardAction::Drop => {
                    let _ = guard::clear(&g.token_id);
                    fire(
                        "guard-dropped",
                        false,
                        &g.token_id,
                        &format!("dropped paper guard {} (position gone)", g.token_id),
                        false,
                    );
                }
                GuardAction::Sell { shares, reason } => {
                    match paper_engine::market_sell(
                        &mut acct,
                        &g.token_id,
                        &levels.bids,
                        shares,
                        now,
                    ) {
                        Ok(t) => {
                            store::save(&acct)?;
                            fire(
                                "guard-exit",
                                false,
                                &g.token_id,
                                &format!(
                                    "{reason} exit (paper): sold {} of {} @ {} (pnl {})",
                                    t.size.round_dp(2),
                                    g.token_id,
                                    t.price.round_dp(4),
                                    t.realized_pnl.unwrap_or_default().round_dp(2)
                                ),
                                true,
                            );
                        }
                        Err(e) => fire(
                            "guard-exit-failed",
                            false,
                            &g.token_id,
                            &format!("{reason} exit rejected ({}): {e}", g.token_id),
                            true,
                        ),
                    }
                    let _ = guard::clear(&g.token_id);
                }
            }
        }
    }

    // Live guards settle against the wallet's CLOB positions.
    let live_due: Vec<&Guard> = due.iter().filter(|g| g.live).collect();
    if !live_due.is_empty() {
        let user = crate::tui::live::resolve_user_address()
            .context("live guard armed but no wallet configured")?;
        let positions = crate::tui::live::fetch_positions(user).await?;
        for g in live_due {
            let (free, avg) = positions
                .iter()
                .find(|p| p.token_id == g.token_id)
                .map_or((Decimal::ZERO, Decimal::ZERO), |p| (p.size, p.avg_price));
            let levels = quotes::fetch_book(clob, quotes::parse_token_id(&g.token_id)?).await?;
            let (mid, best_bid) = mid_and_bid(&levels);
            match guard::evaluate(g, free, avg, mid, best_bid, peaks) {
                GuardAction::Hold => {}
                GuardAction::Drop => {
                    let _ = guard::clear(&g.token_id);
                    fire(
                        "guard-dropped",
                        true,
                        &g.token_id,
                        &format!("dropped live guard {} (position gone)", g.token_id),
                        false,
                    );
                }
                GuardAction::Sell { shares, reason } => {
                    let order = LiveOrder::Market {
                        token_id: g.token_id.clone(),
                        side: crate::paper::types::TradeSide::Sell,
                        amount: shares,
                    };
                    match trade::place(order).await {
                        Ok(s) => fire(
                            "guard-exit",
                            true,
                            &g.token_id,
                            &format!("{reason} exit (live, {}): {s}", g.token_id),
                            true,
                        ),
                        Err(e) => fire(
                            "guard-exit-failed",
                            true,
                            &g.token_id,
                            &format!("{reason} exit FAILED (live, {}): {e:#}", g.token_id),
                            true,
                        ),
                    }
                    let _ = guard::clear(&g.token_id);
                }
            }
        }
    }
    Ok(())
}

fn mid_and_bid(levels: &quotes::BookLevels) -> (Option<Decimal>, Option<Decimal>) {
    let q = levels.quote();
    let mid = match (q.best_bid, q.best_ask) {
        (Some(b), Some(a)) => Some((b + a) / Decimal::from(2)),
        (Some(b), None) => Some(b),
        (None, Some(a)) => Some(a),
        (None, None) => None,
    };
    (mid, q.best_bid)
}

fn log(msg: &str) {
    println!("[{}] {msg}", Utc::now().format("%Y-%m-%d %H:%M:%S"));
}

/// Log to the worker's stdout AND the shared event stream (+ OS toast).
fn fire(kind: &str, live: bool, token_id: &str, msg: &str, toast: bool) {
    log(msg);
    crate::events::emit(kind, live, token_id, msg, toast);
}

// --- launch at login (macOS LaunchAgent) -------------------------------------

const AGENT_LABEL: &str = "com.fiberglass.guard-worker";

fn agent_path() -> Option<PathBuf> {
    Some(
        dirs::home_dir()?
            .join("Library/LaunchAgents")
            .join(format!("{AGENT_LABEL}.plist")),
    )
}

/// The plist's presence on disk *is* the setting — nothing else to store.
pub(crate) fn autostart_enabled() -> bool {
    cfg!(target_os = "macos") && agent_path().is_some_and(|p| p.exists())
}

pub(crate) fn autostart_on() -> Result<()> {
    if !cfg!(target_os = "macos") {
        // IMPORTANT NOTE: launchd only; add a systemd unit if a Linux user asks.
        bail!("Autostart is macOS-only for now");
    }
    let exe = std::env::current_exe().context("Could not locate own binary")?;
    let path = agent_path().context("Could not determine LaunchAgents dir")?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let plist = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key><string>{AGENT_LABEL}</string>
    <key>ProgramArguments</key>
    <array>
        <string>{}</string>
        <string>guard</string>
        <string>run</string>
    </array>
    <key>RunAtLoad</key><true/>
    <key>StandardOutPath</key><string>{}</string>
    <key>StandardErrorPath</key><string>{}</string>
</dict>
</plist>
"#,
        exe.display(),
        config::config_dir()?.join(LOG_FILE).display(),
        config::config_dir()?.join(LOG_FILE).display(),
    );
    fs::write(&path, plist)?;
    let _ = launchctl(["load", "-w"], &path);
    Ok(())
}

pub(crate) fn autostart_off() -> Result<()> {
    let Some(path) = agent_path() else {
        return Ok(());
    };
    if path.exists() {
        let _ = launchctl(["unload", "-w"], &path);
        fs::remove_file(&path)?;
    }
    Ok(())
}

/// Run launchctl quietly — inside the TUI its stdout/stderr would scribble on
/// the alternate screen.
fn launchctl(args: [&str; 2], plist: &std::path::Path) -> std::io::Result<()> {
    Command::new("launchctl")
        .args(args)
        .arg(plist)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stale_heartbeat_is_not_recent() {
        let w = WorkerStatus {
            pid: 1,
            started_at: Utc::now(),
            last_tick: Some(Utc::now() - chrono::Duration::seconds(60)),
            interval_secs: 5,
            guards: 0,
        };
        assert!(!w.is_recent());
        let fresh = WorkerStatus {
            last_tick: Some(Utc::now()),
            ..w
        };
        assert!(fresh.is_recent());
    }
}
