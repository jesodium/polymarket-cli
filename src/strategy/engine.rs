//! The strategy runtime.
//!
//! Owns the live strategy instances, the rolling price history they need, and
//! the tick loop that drives them. On each tick it pulls fresh order books,
//! settles any resting paper orders, asks every running strategy for signals,
//! and executes those signals against the account.
//!
//! Execution is abstracted behind [`ExecutionMode`]. Today only
//! [`ExecutionMode::Paper`] is wired (against the simulator in
//! [`crate::paper`]); [`ExecutionMode::Live`] is the seam where the
//! authenticated CLOB client plugs in — the signal path above it is identical,
//! which is the whole point of routing every order through this one place.

use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use chrono::{DateTime, Utc};
use polymarket_client_sdk_v2::types::Decimal;
use serde_json::Value;

use super::config::{StrategyBook, StrategyConfig};
use super::{Signal, Strategy, StrategyContext, TokenView, registry};
use crate::paper::engine as paper_engine;
use crate::paper::quotes::{self, BookLevels};
use crate::paper::types::{MarketMeta, PaperAccount, Quote};

const MAX_HISTORY: usize = 240;
const MAX_LOGS: usize = 500;
const LOG_FILE: &str = "strategy.log";

/// Where executed orders go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExecutionMode {
    /// Simulated fills against the local paper account.
    Paper,
    /// Real signed orders posted to the authenticated CLOB (via
    /// [`crate::trade::place`]).
    Live,
}

impl std::fmt::Display for ExecutionMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Paper => write!(f, "paper"),
            Self::Live => write!(f, "live"),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum LogLevel {
    Info,
    Trade,
    Warn,
    Error,
}

impl LogLevel {
    pub fn label(self) -> &'static str {
        match self {
            Self::Info => "INFO",
            Self::Trade => "TRADE",
            Self::Warn => "WARN",
            Self::Error => "ERROR",
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct LogLine {
    pub time: DateTime<Utc>,
    pub level: LogLevel,
    pub source: String,
    pub message: String,
}

/// A running strategy plus its rolling state.
struct Instance {
    id: String,
    kind: String,
    enabled: bool,
    running: bool,
    tokens: Vec<String>,
    params: Value,
    strat: Box<dyn Strategy>,
    history: HashMap<String, VecDeque<Decimal>>,
    signals: u64,
    orders: u64,
    errors: u64,
    last_action: Option<String>,
    last_action_at: Option<DateTime<Utc>>,
}

/// A point-in-time view of an instance for the UI/CLI (no live trait object).
#[derive(Clone, Debug)]
pub(crate) struct InstanceStatus {
    pub id: String,
    pub kind: String,
    pub description: String,
    pub enabled: bool,
    pub running: bool,
    pub tokens: Vec<String>,
    pub signals: u64,
    pub orders: u64,
    pub errors: u64,
    pub last_action: Option<String>,
    /// Exposed for clients that render a timestamp; the bundled TUI shows the
    /// action text only.
    #[allow(dead_code)]
    pub last_action_at: Option<DateTime<Utc>>,
}

struct EngineState {
    instances: Vec<Instance>,
    logs: VecDeque<LogLine>,
    meta_cache: HashMap<String, MarketMeta>,
    mode: ExecutionMode,
    tick_count: u64,
    last_tick: Option<DateTime<Utc>>,
}

/// Cloneable handle to the strategy runtime. Cheap to clone (shares state via
/// `Arc`), so the TUI and the tick loop can both hold one.
#[derive(Clone)]
pub(crate) struct StrategyEngine {
    state: Arc<Mutex<EngineState>>,
    account: Arc<Mutex<PaperAccount>>,
    tick_secs: u64,
}

impl StrategyEngine {
    /// Build an engine from the persisted roster, sharing `account` with the
    /// rest of the app.
    pub fn new(account: Arc<Mutex<PaperAccount>>, tick_secs: u64, mode: ExecutionMode) -> Self {
        let book = super::config::load().unwrap_or_default();
        Self::from_book(account, tick_secs, mode, &book)
    }

    pub fn from_book(
        account: Arc<Mutex<PaperAccount>>,
        tick_secs: u64,
        mode: ExecutionMode,
        book: &StrategyBook,
    ) -> Self {
        let instances = book
            .strategies
            .iter()
            .filter_map(|c| Self::build_instance(c).ok())
            .collect();
        let state = EngineState {
            instances,
            logs: VecDeque::new(),
            meta_cache: HashMap::new(),
            mode,
            tick_count: 0,
            last_tick: None,
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            account,
            tick_secs: tick_secs.max(1),
        }
    }

    fn build_instance(c: &StrategyConfig) -> Result<Instance> {
        let strat = registry::build(&c.kind, &c.params)?;
        Ok(Instance {
            id: c.id.clone(),
            kind: c.kind.clone(),
            enabled: c.enabled,
            running: false,
            tokens: c.tokens.clone(),
            params: c.params.clone(),
            strat,
            history: HashMap::new(),
            signals: 0,
            orders: 0,
            errors: 0,
            last_action: None,
            last_action_at: None,
        })
    }

    pub fn tick_secs(&self) -> u64 {
        self.tick_secs
    }

    pub fn mode(&self) -> ExecutionMode {
        self.state.lock().unwrap().mode
    }

    // --- Roster management -------------------------------------------------

    /// Add a new strategy instance and persist the roster.
    pub fn add(&self, id: &str, kind: &str, tokens: Vec<String>) -> Result<()> {
        let mut cfg = StrategyConfig::new(id, kind);
        cfg.tokens = tokens;
        cfg.enabled = true;
        let instance = Self::build_instance(&cfg)?;
        {
            let mut st = self.state.lock().unwrap();
            if st.instances.iter().any(|i| i.id == id) {
                anyhow::bail!("A strategy with id '{id}' already exists");
            }
            st.instances.push(instance);
        }
        self.log(LogLevel::Info, id, &format!("Added strategy '{kind}'"));
        self.persist()
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        {
            let mut st = self.state.lock().unwrap();
            let before = st.instances.len();
            st.instances.retain(|i| i.id != id);
            if st.instances.len() == before {
                anyhow::bail!("No strategy with id '{id}'");
            }
        }
        self.persist()
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.mutate(id, |i| {
            i.enabled = enabled;
            if !enabled {
                i.running = false;
            }
        })?;
        self.log(
            LogLevel::Info,
            id,
            if enabled { "Enabled" } else { "Disabled" },
        );
        self.persist()
    }

    /// Begin ticking a strategy (must be enabled).
    pub fn start(&self, id: &str) -> Result<()> {
        let mut started = false;
        self.mutate(id, |i| {
            if i.enabled {
                i.running = true;
                started = true;
            }
        })?;
        if !started {
            anyhow::bail!("Strategy '{id}' is disabled; enable it first");
        }
        self.log(LogLevel::Info, id, "Started");
        Ok(())
    }

    pub fn stop(&self, id: &str) -> Result<()> {
        self.mutate(id, |i| i.running = false)?;
        self.log(LogLevel::Info, id, "Stopped");
        Ok(())
    }

    /// Enable + start every configured strategy.
    pub fn start_all(&self) {
        let ids: Vec<String> = {
            let st = self.state.lock().unwrap();
            st.instances.iter().map(|i| i.id.clone()).collect()
        };
        for id in ids {
            let _ = self.set_enabled(&id, true);
            let _ = self.start(&id);
        }
    }

    fn mutate(&self, id: &str, f: impl FnOnce(&mut Instance)) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let inst = st
            .instances
            .iter_mut()
            .find(|i| i.id == id)
            .ok_or_else(|| anyhow::anyhow!("No strategy with id '{id}'"))?;
        f(inst);
        Ok(())
    }

    /// Write the current roster back to disk.
    pub fn persist(&self) -> Result<()> {
        let book = {
            let st = self.state.lock().unwrap();
            StrategyBook {
                strategies: st
                    .instances
                    .iter()
                    .map(|i| StrategyConfig {
                        id: i.id.clone(),
                        kind: i.kind.clone(),
                        enabled: i.enabled,
                        tokens: i.tokens.clone(),
                        params: i.params.clone(),
                    })
                    .collect(),
            }
        };
        super::config::save(&book)
    }

    // --- Snapshots ---------------------------------------------------------

    pub fn snapshot(&self) -> Vec<InstanceStatus> {
        let st = self.state.lock().unwrap();
        st.instances
            .iter()
            .map(|i| InstanceStatus {
                id: i.id.clone(),
                kind: i.kind.clone(),
                description: i.strat.describe(),
                enabled: i.enabled,
                running: i.running,
                tokens: i.tokens.clone(),
                signals: i.signals,
                orders: i.orders,
                errors: i.errors,
                last_action: i.last_action.clone(),
                last_action_at: i.last_action_at,
            })
            .collect()
    }

    pub fn recent_logs(&self, n: usize) -> Vec<LogLine> {
        let st = self.state.lock().unwrap();
        st.logs.iter().rev().take(n).rev().cloned().collect()
    }

    pub fn running_count(&self) -> usize {
        let st = self.state.lock().unwrap();
        st.instances.iter().filter(|i| i.running).count()
    }

    /// `(ticks elapsed, time of last tick)` for status displays.
    pub fn runtime_stats(&self) -> (u64, Option<DateTime<Utc>>) {
        let st = self.state.lock().unwrap();
        (st.tick_count, st.last_tick)
    }

    /// Flush the account to disk (engine already persists on fills; this is a
    /// final-save hook on shutdown).
    pub fn save_account(&self) -> Result<()> {
        crate::paper::store::save(&self.account.lock().unwrap())
    }

    // --- The tick loop -----------------------------------------------------

    /// Run one tick: fetch data, settle resting orders, run strategies,
    /// execute signals. Safe to call when nothing is running (it no-ops).
    pub async fn tick(&self) -> Result<()> {
        // Which tokens do running strategies care about this tick?
        let tokens = {
            let st = self.state.lock().unwrap();
            let mut tokens: Vec<String> = st
                .instances
                .iter()
                .filter(|i| i.running && i.enabled)
                .flat_map(|i| i.tokens.iter().cloned())
                .collect();
            tokens.sort();
            tokens.dedup();
            tokens
        };

        // Always advance the clock so the UI shows a live heartbeat.
        if tokens.is_empty() {
            let mut st = self.state.lock().unwrap();
            st.last_tick = Some(Utc::now());
            st.tick_count += 1;
            return Ok(());
        }

        // Fetch books (and any missing metadata) without holding locks.
        let client = crate::auth::unauthenticated_clob_client()?;
        let mut books: HashMap<String, BookLevels> = HashMap::new();
        for tid in &tokens {
            if let Ok(token) = quotes::parse_token_id(tid)
                && let Ok(book) = quotes::fetch_book(&client, token).await
            {
                books.insert(tid.clone(), book);
            }
        }
        self.ensure_meta(&tokens).await;

        // Apply synchronously under lock.
        let now = Utc::now();
        let (fills_logs, pending_live);
        {
            let mut st = self.state.lock().unwrap();
            let mut acct = self.account.lock().unwrap();
            (fills_logs, pending_live) = apply_tick(&mut st, &mut acct, &books, now);
            st.last_tick = Some(now);
            st.tick_count += 1;
        }
        // Persist the paper account and emit logs outside the data locks.
        if self.mode() == ExecutionMode::Paper && !fills_logs.is_empty() {
            let _ = crate::paper::store::save(&self.account.lock().unwrap());
        }
        for (level, source, msg) in fills_logs {
            self.log(level, &source, &msg);
        }

        // Submit any live orders the strategies requested (real CLOB orders,
        // same path as `clob create-order`). Done here, off the locks.
        for (id, order, summary) in pending_live {
            match crate::trade::place(order).await {
                Ok(s) => {
                    self.record_live(&id, true, &now);
                    self.log(LogLevel::Trade, &id, &s);
                }
                Err(e) => {
                    self.record_live(&id, false, &now);
                    self.log(LogLevel::Warn, &id, &format!("{summary} FAILED: {e}"));
                }
            }
        }
        Ok(())
    }

    /// Update an instance's counters after a live order attempt.
    fn record_live(&self, id: &str, ok: bool, now: &DateTime<Utc>) {
        let mut st = self.state.lock().unwrap();
        if let Some(inst) = st.instances.iter_mut().find(|i| i.id == id) {
            if ok {
                inst.orders += 1;
                inst.last_action = Some("live order submitted".to_string());
                inst.last_action_at = Some(*now);
            } else {
                inst.errors += 1;
            }
        }
    }

    /// Fetch and cache market metadata for any tokens we haven't seen.
    async fn ensure_meta(&self, tokens: &[String]) {
        let missing: Vec<String> = {
            let st = self.state.lock().unwrap();
            tokens
                .iter()
                .filter(|t| !st.meta_cache.contains_key(*t))
                .cloned()
                .collect()
        };
        if missing.is_empty() {
            return;
        }
        let gamma = polymarket_client_sdk_v2::gamma::Client::default();
        for tid in missing {
            if let Ok(token) = quotes::parse_token_id(&tid) {
                let meta = quotes::fetch_meta(&gamma, token).await;
                self.state.lock().unwrap().meta_cache.insert(tid, meta);
            }
        }
    }

    /// Run the tick loop forever, sleeping `tick_secs` between ticks. Intended
    /// to be `tokio::spawn`ed; returns only on error.
    pub async fn run_forever(self) {
        loop {
            if let Err(e) = self.tick().await {
                self.log(LogLevel::Error, "engine", &format!("tick failed: {e}"));
            }
            tokio::time::sleep(std::time::Duration::from_secs(self.tick_secs)).await;
        }
    }

    pub fn log(&self, level: LogLevel, source: &str, message: &str) {
        let line = LogLine {
            time: Utc::now(),
            level,
            source: source.to_string(),
            message: message.to_string(),
        };
        append_log_file(&line);
        let mut st = self.state.lock().unwrap();
        st.logs.push_back(line);
        while st.logs.len() > MAX_LOGS {
            st.logs.pop_front();
        }
    }
}

/// Pure-ish tick application: mutates engine state and the account, returning
/// log lines to emit. Kept free of I/O and locking so the locking discipline
/// stays in one place (the caller).
/// Orders a strategy wants placed live, queued for async submission after the
/// data locks are released: `(strategy id, order, human summary)`.
type PendingLive = (String, crate::trade::LiveOrder, String);

fn apply_tick(
    st: &mut EngineState,
    acct: &mut PaperAccount,
    books: &HashMap<String, BookLevels>,
    now: DateTime<Utc>,
) -> (Vec<(LogLevel, String, String)>, Vec<PendingLive>) {
    let mut out = Vec::new();
    let mut pending: Vec<PendingLive> = Vec::new();

    // 1. Settle resting limit orders against the freshest quotes.
    let quotes_map: HashMap<String, Quote> =
        books.iter().map(|(k, v)| (k.clone(), v.quote())).collect();
    let quotes_btree: std::collections::BTreeMap<String, Quote> = quotes_map.into_iter().collect();
    let fills = paper_engine::settle_open_orders(acct, &quotes_btree, now);
    for fill in &fills {
        out.push((
            LogLevel::Trade,
            "engine".to_string(),
            format!(
                "Resting {} filled: {} {} @ {}",
                fill.side,
                fill.size.round_dp(2),
                truncate(&fill.question, 40),
                fill.price
            ),
        ));
    }

    let mode = st.mode;
    let meta_cache = &st.meta_cache;

    // 2. Drive each running strategy.
    for inst in st.instances.iter_mut().filter(|i| i.running && i.enabled) {
        // Update rolling price history from this tick's books.
        for tid in &inst.tokens {
            if let Some(book) = books.get(tid) {
                let q = book.quote();
                if let Some(mid) = midpoint(&q) {
                    let h = inst.history.entry(tid.clone()).or_default();
                    h.push_back(mid);
                    while h.len() > MAX_HISTORY {
                        h.pop_front();
                    }
                }
            }
        }

        // Build the context the strategy sees.
        let token_views: Vec<TokenView> = inst
            .tokens
            .iter()
            .map(|tid| {
                let q = books.get(tid).map(BookLevels::quote).unwrap_or_default();
                let meta = meta_cache.get(tid).cloned().unwrap_or_default();
                let pos = acct.positions.get(tid);
                let held = pos.map_or(Decimal::ZERO, |p| p.size);
                let free = held - acct.reserved_shares(tid);
                TokenView {
                    token_id: tid.clone(),
                    question: meta.question,
                    outcome: meta.outcome,
                    best_bid: q.best_bid,
                    best_ask: q.best_ask,
                    mid: midpoint(&q),
                    history: inst
                        .history
                        .get(tid)
                        .map(|h| h.iter().copied().collect())
                        .unwrap_or_default(),
                    position_size: free.max(Decimal::ZERO),
                    avg_price: pos.map_or(Decimal::ZERO, |p| p.avg_price),
                }
            })
            .collect();

        let ctx = StrategyContext {
            cash: acct.cash,
            tokens: token_views,
        };
        let signals = inst.strat.on_tick(&ctx);
        if signals.is_empty() {
            continue;
        }
        inst.signals += signals.len() as u64;

        for sig in signals {
            let tid = sig.token_id().to_string();
            let Some(book) = books.get(&tid) else {
                continue;
            };
            if mode == ExecutionMode::Live {
                // Queue for real submission once locks are released.
                pending.push((inst.id.clone(), signal_to_live(&sig), sig.summary()));
                continue;
            }
            let meta = meta_cache.get(&tid).cloned().unwrap_or_default();
            match execute_paper(acct, &sig, book, &meta, now) {
                Ok(desc) => {
                    inst.orders += 1;
                    inst.last_action = Some(desc.clone());
                    inst.last_action_at = Some(now);
                    out.push((LogLevel::Trade, inst.id.clone(), desc));
                }
                Err(e) => {
                    inst.errors += 1;
                    out.push((
                        LogLevel::Warn,
                        inst.id.clone(),
                        format!("{} rejected: {e}", sig.summary()),
                    ));
                }
            }
        }
    }
    (out, pending)
}

/// Map a strategy signal onto a real CLOB order.
fn signal_to_live(sig: &Signal) -> crate::trade::LiveOrder {
    use crate::paper::types::TradeSide;
    match sig {
        Signal::MarketBuy { token_id, usd } => crate::trade::LiveOrder::Market {
            token_id: token_id.clone(),
            side: TradeSide::Buy,
            amount: *usd,
        },
        Signal::MarketSell { token_id, shares } => crate::trade::LiveOrder::Market {
            token_id: token_id.clone(),
            side: TradeSide::Sell,
            amount: *shares,
        },
        Signal::LimitBuy {
            token_id,
            price,
            size,
        } => crate::trade::LiveOrder::Limit {
            token_id: token_id.clone(),
            side: TradeSide::Buy,
            price: *price,
            size: *size,
        },
        Signal::LimitSell {
            token_id,
            price,
            size,
        } => crate::trade::LiveOrder::Limit {
            token_id: token_id.clone(),
            side: TradeSide::Sell,
            price: *price,
            size: *size,
        },
    }
}

/// Execute one signal against the paper account, returning a log description.
fn execute_paper(
    acct: &mut PaperAccount,
    sig: &Signal,
    book: &BookLevels,
    meta: &MarketMeta,
    now: DateTime<Utc>,
) -> Result<String> {
    let q = book.quote();
    match sig {
        Signal::MarketBuy { token_id, usd } => {
            let t = paper_engine::market_buy(acct, token_id, meta, &book.asks, *usd, now)?;
            Ok(format!(
                "BUY {} {} @ {} (${})",
                t.size.round_dp(2),
                truncate(&t.question, 36),
                t.price.round_dp(4),
                t.notional.round_dp(2)
            ))
        }
        Signal::MarketSell { token_id, shares } => {
            let t = paper_engine::market_sell(acct, token_id, &book.bids, *shares, now)?;
            Ok(format!(
                "SELL {} {} @ {} (pnl {})",
                t.size.round_dp(2),
                truncate(&t.question, 36),
                t.price.round_dp(4),
                t.realized_pnl.unwrap_or_default().round_dp(2)
            ))
        }
        Signal::LimitBuy {
            token_id,
            price,
            size,
        } => match paper_engine::limit_buy(acct, token_id, meta, q, *price, *size, now)? {
            paper_engine::LimitOutcome::Filled(t) => Ok(format!(
                "LIMIT BUY filled {} @ {}",
                t.size.round_dp(2),
                t.price
            )),
            paper_engine::LimitOutcome::Resting(o) => Ok(format!(
                "LIMIT BUY resting {} @ {}",
                o.size.round_dp(2),
                o.price
            )),
        },
        Signal::LimitSell {
            token_id,
            price,
            size,
        } => match paper_engine::limit_sell(acct, token_id, q, *price, *size, now)? {
            paper_engine::LimitOutcome::Filled(t) => Ok(format!(
                "LIMIT SELL filled {} @ {}",
                t.size.round_dp(2),
                t.price
            )),
            paper_engine::LimitOutcome::Resting(o) => Ok(format!(
                "LIMIT SELL resting {} @ {}",
                o.size.round_dp(2),
                o.price
            )),
        },
    }
}

fn midpoint(q: &Quote) -> Option<Decimal> {
    match (q.best_bid, q.best_ask) {
        (Some(b), Some(a)) => Some((b + a) / Decimal::from(2)),
        (Some(b), None) => Some(b),
        (None, Some(a)) => Some(a),
        (None, None) => None,
    }
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn append_log_file(line: &LogLine) {
    let Ok(dir) = crate::config::config_dir() else {
        return;
    };
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(LOG_FILE);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        use std::io::Write as _;
        let _ = writeln!(
            f,
            "{} [{}] {} — {}",
            line.time.format("%Y-%m-%d %H:%M:%S"),
            line.level.label(),
            line.source,
            line.message
        );
    }
}
