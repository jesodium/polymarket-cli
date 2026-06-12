//! The copy-trading runtime.
//!
//! For each followed wallet it polls the Data API for recent **trade**
//! activity, decides whether to mirror each new trade (price-band and holdings
//! filters, fixed copy size capped by a per-trade ceiling), and routes the
//! resulting order through the same paper/live execution path the manual
//! trader uses.
//!
//! The decision half ([`decide`]) is pure and unit-tested; the polling and
//! execution half does the I/O and keeps the locking discipline in one place.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use polymarket_client_sdk_v2::data::types::Side as DataSide;
use polymarket_client_sdk_v2::data::types::request::ActivityRequest;
use polymarket_client_sdk_v2::data::types::{ActivityType, response::Activity};
use polymarket_client_sdk_v2::types::{Address, Decimal};

use super::config::{CopyBook, CopyTrader};
use crate::paper::engine as paper_engine;
use crate::paper::quotes;
use crate::paper::types::{PaperAccount, TradeSide};

/// Where executed orders go: the local paper account or the real CLOB.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExecutionMode {
    Paper,
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

const MAX_LOGS: usize = 500;
const LOG_FILE: &str = "copytrade.log";
/// How many recent activities to pull per trader per poll.
const ACTIVITY_LIMIT: i32 = 100;

/// What to do about one of the followed trader's trades.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CopyAction {
    /// Mirror a buy by deploying `usd` pUSD into `token_id`.
    Buy { token_id: String, usd: Decimal },
    /// Mirror a sell by offloading `shares` of `token_id` we already hold.
    Sell { token_id: String, shares: Decimal },
}

/// Outcome of evaluating one trade against a trader's rules.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CopyDecision {
    Act(CopyAction),
    Skip(String),
}

/// The minimal slice of a trade activity the decision logic needs. Decoupled
/// from the SDK type so the rules can be tested without a network response.
#[derive(Clone, Debug)]
pub(crate) struct TradeEvent {
    pub token_id: String,
    pub side: TradeSide,
    pub price: Decimal,
}

/// Decide how to mirror one of the followed trader's trades.
///
/// `free_shares` is what we currently hold (and could sell) in the token.
/// Returns `None` when the trade isn't actionable for us at all (e.g. a sell we
/// aren't configured to mirror).
pub(crate) fn decide(
    cfg: &CopyTrader,
    ev: &TradeEvent,
    free_shares: Decimal,
) -> Option<CopyDecision> {
    match ev.side {
        TradeSide::Buy => {
            if ev.price < cfg.price_min || ev.price > cfg.price_max {
                return Some(CopyDecision::Skip(format!(
                    "buy @ {} outside price band {}–{}",
                    ev.price.round_dp(3),
                    cfg.price_min.normalize(),
                    cfg.price_max.normalize()
                )));
            }
            let usd = cfg.copy_size_usd.min(cfg.max_dollar_cap);
            if usd <= Decimal::ZERO {
                return Some(CopyDecision::Skip("copy size is zero".to_string()));
            }
            Some(CopyDecision::Act(CopyAction::Buy {
                token_id: ev.token_id.clone(),
                usd,
            }))
        }
        TradeSide::Sell => {
            if !cfg.mirror_sells {
                return None; // not mirroring exits for this trader
            }
            let shares =
                free_shares.round_dp_with_strategy(2, rust_decimal::RoundingStrategy::ToZero);
            if shares <= Decimal::ZERO {
                return Some(CopyDecision::Skip(
                    "trader sold but we hold nothing".to_string(),
                ));
            }
            Some(CopyDecision::Act(CopyAction::Sell {
                token_id: ev.token_id.clone(),
                shares,
            }))
        }
    }
}

/// A followed trader plus its rolling runtime state.
struct TraderState {
    cfg: CopyTrader,
    running: bool,
    /// False until the first poll establishes a baseline (so we don't replay
    /// the trader's whole history the moment we start following).
    primed: bool,
    /// Newest activity timestamp we've already accounted for.
    last_seen_ts: i64,
    copied: u64,
    skipped: u64,
    errors: u64,
    last_action: Option<String>,
    last_action_at: Option<DateTime<Utc>>,
}

/// Point-in-time view of a follower for the UI/CLI.
#[derive(Clone, Debug)]
pub(crate) struct TraderStatus {
    pub id: String,
    pub nickname: String,
    pub wallet: String,
    pub enabled: bool,
    pub running: bool,
    pub copied: u64,
    pub skipped: u64,
    pub errors: u64,
    pub last_action: Option<String>,
}

struct EngineState {
    traders: Vec<TraderState>,
    logs: std::collections::VecDeque<LogLine>,
    mode: ExecutionMode,
    poll_count: u64,
    last_poll: Option<DateTime<Utc>>,
}

/// Cloneable handle to the copy-trading runtime (shares state via `Arc`).
#[derive(Clone)]
pub(crate) struct CopyEngine {
    state: Arc<Mutex<EngineState>>,
    account: Arc<Mutex<PaperAccount>>,
    interval: u64,
}

impl CopyEngine {
    pub fn new(account: Arc<Mutex<PaperAccount>>, interval: u64, mode: ExecutionMode) -> Self {
        let book = super::config::load().unwrap_or_default();
        Self::from_book(account, interval, mode, &book)
    }

    pub fn from_book(
        account: Arc<Mutex<PaperAccount>>,
        interval: u64,
        mode: ExecutionMode,
        book: &CopyBook,
    ) -> Self {
        let traders = book
            .traders
            .iter()
            .map(|c| TraderState {
                cfg: c.clone(),
                running: false,
                primed: false,
                last_seen_ts: 0,
                copied: 0,
                skipped: 0,
                errors: 0,
                last_action: None,
                last_action_at: None,
            })
            .collect();
        let state = EngineState {
            traders,
            logs: std::collections::VecDeque::new(),
            mode,
            poll_count: 0,
            last_poll: None,
        };
        Self {
            state: Arc::new(Mutex::new(state)),
            account,
            interval: interval.max(1),
        }
    }

    /// Configured poll cadence (seconds). Exposed for status displays.
    pub fn interval(&self) -> u64 {
        self.interval
    }

    pub fn mode(&self) -> ExecutionMode {
        self.state.lock().unwrap().mode
    }

    // --- Roster management -------------------------------------------------

    /// Follow a new wallet and persist the roster.
    pub fn add(&self, cfg: CopyTrader) -> Result<()> {
        // Validate the address up front so a bad wallet never reaches polling.
        Address::from_str(cfg.wallet.trim())
            .map_err(|_| anyhow::anyhow!("Invalid wallet address: {}", cfg.wallet))?;
        {
            let mut st = self.state.lock().unwrap();
            if st.traders.iter().any(|t| t.cfg.id == cfg.id) {
                bail!("A followed trader with id '{}' already exists", cfg.id);
            }
            let id = cfg.id.clone();
            let nick = cfg.nickname.clone();
            st.traders.push(TraderState {
                cfg,
                running: false,
                primed: false,
                last_seen_ts: 0,
                copied: 0,
                skipped: 0,
                errors: 0,
                last_action: None,
                last_action_at: None,
            });
            drop(st);
            self.log(LogLevel::Info, &id, &format!("Now following {nick}"));
        }
        self.persist()
    }

    pub fn remove(&self, id: &str) -> Result<()> {
        {
            let mut st = self.state.lock().unwrap();
            let before = st.traders.len();
            st.traders.retain(|t| t.cfg.id != id);
            if st.traders.len() == before {
                bail!("No followed trader with id '{id}'");
            }
        }
        self.persist()
    }

    pub fn set_enabled(&self, id: &str, enabled: bool) -> Result<()> {
        self.mutate(id, |t| {
            t.cfg.enabled = enabled;
            if !enabled {
                t.running = false;
            }
        })?;
        self.log(
            LogLevel::Info,
            id,
            if enabled { "Enabled" } else { "Disabled" },
        );
        self.persist()
    }

    pub fn start(&self, id: &str) -> Result<()> {
        let mut started = false;
        self.mutate(id, |t| {
            if t.cfg.enabled {
                t.running = true;
                started = true;
            }
        })?;
        if !started {
            bail!("Follower '{id}' is disabled; enable it first");
        }
        self.log(LogLevel::Info, id, "Started");
        Ok(())
    }

    pub fn stop(&self, id: &str) -> Result<()> {
        self.mutate(id, |t| t.running = false)?;
        self.log(LogLevel::Info, id, "Stopped");
        Ok(())
    }

    pub fn start_all(&self) {
        let ids: Vec<String> = {
            let st = self.state.lock().unwrap();
            st.traders.iter().map(|t| t.cfg.id.clone()).collect()
        };
        for id in ids {
            let _ = self.set_enabled(&id, true);
            let _ = self.start(&id);
        }
    }

    fn mutate(&self, id: &str, f: impl FnOnce(&mut TraderState)) -> Result<()> {
        let mut st = self.state.lock().unwrap();
        let t = st
            .traders
            .iter_mut()
            .find(|t| t.cfg.id == id)
            .ok_or_else(|| anyhow::anyhow!("No followed trader with id '{id}'"))?;
        f(t);
        Ok(())
    }

    pub fn persist(&self) -> Result<()> {
        let book = {
            let st = self.state.lock().unwrap();
            CopyBook {
                traders: st.traders.iter().map(|t| t.cfg.clone()).collect(),
            }
        };
        super::config::save(&book)
    }

    pub fn running_count(&self) -> usize {
        let st = self.state.lock().unwrap();
        st.traders.iter().filter(|t| t.running).count()
    }

    pub fn snapshot(&self) -> Vec<TraderStatus> {
        let st = self.state.lock().unwrap();
        st.traders
            .iter()
            .map(|t| TraderStatus {
                id: t.cfg.id.clone(),
                nickname: t.cfg.nickname.clone(),
                wallet: t.cfg.wallet.clone(),
                enabled: t.cfg.enabled,
                running: t.running,
                copied: t.copied,
                skipped: t.skipped,
                errors: t.errors,
                last_action: t.last_action.clone(),
            })
            .collect()
    }

    pub fn recent_logs(&self, n: usize) -> Vec<LogLine> {
        let st = self.state.lock().unwrap();
        st.logs.iter().rev().take(n).rev().cloned().collect()
    }

    pub fn save_account(&self) -> Result<()> {
        crate::paper::store::save(&self.account.lock().unwrap())
    }

    // --- The poll loop -----------------------------------------------------

    /// One poll: for every running follower, pull new trade activity and
    /// mirror anything that passes the rules. Safe to call with nothing
    /// running (it just advances the heartbeat).
    pub async fn poll(&self) -> Result<()> {
        // Snapshot the work to do without holding the lock across awaits.
        let jobs: Vec<(String, CopyTrader, bool, i64)> = {
            let st = self.state.lock().unwrap();
            st.traders
                .iter()
                .filter(|t| t.running && t.cfg.enabled)
                .map(|t| (t.cfg.id.clone(), t.cfg.clone(), t.primed, t.last_seen_ts))
                .collect()
        };

        if jobs.is_empty() {
            let mut st = self.state.lock().unwrap();
            st.last_poll = Some(Utc::now());
            st.poll_count += 1;
            return Ok(());
        }

        let data = polymarket_client_sdk_v2::data::Client::default();
        for (id, cfg, primed, last_seen) in jobs {
            if let Err(e) = self.poll_trader(&data, &id, &cfg, primed, last_seen).await {
                self.bump_error(&id);
                self.log(LogLevel::Warn, &id, &format!("poll failed: {e}"));
            }
        }

        let mut st = self.state.lock().unwrap();
        st.last_poll = Some(Utc::now());
        st.poll_count += 1;
        Ok(())
    }

    async fn poll_trader(
        &self,
        data: &polymarket_client_sdk_v2::data::Client,
        id: &str,
        cfg: &CopyTrader,
        primed: bool,
        last_seen: i64,
    ) -> Result<()> {
        let wallet = Address::from_str(cfg.wallet.trim())
            .map_err(|_| anyhow::anyhow!("Invalid wallet address: {}", cfg.wallet))?;
        let request = ActivityRequest::builder()
            .user(wallet)
            .activity_types(vec![ActivityType::Trade])
            .limit(ACTIVITY_LIMIT)?
            .build();
        let activities = data.activity(&request).await?;

        let newest = activities.iter().map(|a| a.timestamp).max();

        // First poll just establishes a baseline — never replay history.
        if !primed {
            let baseline = newest.unwrap_or_else(|| Utc::now().timestamp());
            self.mutate(id, |t| {
                t.primed = true;
                t.last_seen_ts = baseline;
            })?;
            self.log(
                LogLevel::Info,
                id,
                &format!("Baseline set for {}; watching for new trades", cfg.nickname),
            );
            return Ok(());
        }

        // Apply oldest-first so positions build in the order the trader made
        // them, and only trades newer than what we've already seen.
        let mut fresh: Vec<&Activity> = activities
            .iter()
            .filter(|a| a.timestamp > last_seen)
            .collect();
        fresh.sort_by_key(|a| a.timestamp);

        let mut high_water = last_seen;
        for a in fresh {
            high_water = high_water.max(a.timestamp);
            let Some(ev) = to_trade_event(a) else {
                continue; // missing fields — not actionable
            };
            let free = self.free_shares(&ev.token_id);
            let Some(decision) = decide(cfg, &ev, free) else {
                continue;
            };
            match decision {
                CopyDecision::Skip(reason) => {
                    self.bump_skip(id);
                    self.log(LogLevel::Info, id, &format!("Skipped: {reason}"));
                }
                CopyDecision::Act(action) => self.execute(id, cfg, action).await,
            }
        }

        if high_water > last_seen {
            self.mutate(id, |t| t.last_seen_ts = high_water)?;
        }
        Ok(())
    }

    /// Free (un-reserved) shares we hold in a token right now.
    fn free_shares(&self, token_id: &str) -> Decimal {
        let acct = self.account.lock().unwrap();
        let held = acct
            .positions
            .get(token_id)
            .map_or(Decimal::ZERO, |p| p.size);
        (held - acct.reserved_shares(token_id)).max(Decimal::ZERO)
    }

    /// Execute one copy action against the paper account or the live CLOB.
    async fn execute(&self, id: &str, cfg: &CopyTrader, action: CopyAction) {
        let mode = self.mode();
        match mode {
            ExecutionMode::Live => {
                let order = match &action {
                    CopyAction::Buy { token_id, usd } => crate::trade::LiveOrder::Market {
                        token_id: token_id.clone(),
                        side: TradeSide::Buy,
                        amount: *usd,
                    },
                    CopyAction::Sell { token_id, shares } => crate::trade::LiveOrder::Market {
                        token_id: token_id.clone(),
                        side: TradeSide::Sell,
                        amount: *shares,
                    },
                };
                match crate::trade::place(order).await {
                    Ok(s) => {
                        self.bump_copy(id, &describe_action(&action));
                        self.log(LogLevel::Trade, id, &s);
                    }
                    Err(e) => {
                        self.bump_error(id);
                        self.log(LogLevel::Warn, id, &format!("live order failed: {e}"));
                    }
                }
            }
            ExecutionMode::Paper => {
                if let Err(e) = self.execute_paper(id, cfg, &action).await {
                    self.bump_error(id);
                    self.log(
                        LogLevel::Warn,
                        id,
                        &format!("{} rejected: {e}", describe_action(&action)),
                    );
                }
            }
        }
    }

    async fn execute_paper(&self, id: &str, cfg: &CopyTrader, action: &CopyAction) -> Result<()> {
        let token_id = match action {
            CopyAction::Buy { token_id, .. } | CopyAction::Sell { token_id, .. } => {
                token_id.clone()
            }
        };
        let client = crate::auth::unauthenticated_clob_client()?;
        let token = quotes::parse_token_id(&token_id)?;
        let book = quotes::fetch_book(&client, token).await?;
        // Resolve the market name off-lock so positions read nicely; a hiccup
        // only yields placeholder text, never a blocked trade.
        let meta = {
            let gamma = polymarket_client_sdk_v2::gamma::Client::default();
            quotes::fetch_meta(&gamma, token).await
        };
        let now = Utc::now();

        let desc = {
            let mut acct = self.account.lock().unwrap();
            match action {
                CopyAction::Buy { usd, .. } => {
                    paper_engine::check_slippage(
                        &book.asks,
                        TradeSide::Buy,
                        *usd,
                        cfg.slippage_pct,
                    )?;
                    let t = paper_engine::market_buy(
                        &mut acct, &token_id, &meta, &book.asks, &book.bids, *usd, now,
                    )?;
                    format!(
                        "COPY BUY {} {} @ {} (${})",
                        t.size.round_dp(2),
                        crate::output::truncate(&t.question, 30),
                        t.price.round_dp(4),
                        t.notional.round_dp(2)
                    )
                }
                CopyAction::Sell { shares, .. } => {
                    paper_engine::check_slippage(
                        &book.bids,
                        TradeSide::Sell,
                        *shares,
                        cfg.slippage_pct,
                    )?;
                    let t =
                        paper_engine::market_sell(&mut acct, &token_id, &book.bids, *shares, now)?;
                    format!(
                        "COPY SELL {} @ {} (pnl {})",
                        t.size.round_dp(2),
                        t.price.round_dp(4),
                        t.realized_pnl.unwrap_or_default().round_dp(2)
                    )
                }
            }
        };

        let _ = crate::paper::store::save(&self.account.lock().unwrap());
        self.bump_copy(id, &desc);
        self.log(LogLevel::Trade, id, &desc);
        Ok(())
    }

    fn bump_copy(&self, id: &str, desc: &str) {
        let mut st = self.state.lock().unwrap();
        if let Some(t) = st.traders.iter_mut().find(|t| t.cfg.id == id) {
            t.copied += 1;
            t.last_action = Some(desc.to_string());
            t.last_action_at = Some(Utc::now());
        }
    }

    fn bump_skip(&self, id: &str) {
        let mut st = self.state.lock().unwrap();
        if let Some(t) = st.traders.iter_mut().find(|t| t.cfg.id == id) {
            t.skipped += 1;
        }
    }

    fn bump_error(&self, id: &str) {
        let mut st = self.state.lock().unwrap();
        if let Some(t) = st.traders.iter_mut().find(|t| t.cfg.id == id) {
            t.errors += 1;
        }
    }

    /// Run the poll loop forever, sleeping `interval` between polls. Spawned by
    /// the TUI; the CLI `run` drives `poll` itself so it can interleave Ctrl-C
    /// handling and log draining.
    pub async fn run_forever(self) {
        loop {
            if let Err(e) = self.poll().await {
                self.log(LogLevel::Error, "engine", &format!("poll failed: {e}"));
            }
            tokio::time::sleep(std::time::Duration::from_secs(self.interval)).await;
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

/// Pull the actionable fields out of a Data-API activity, if present.
fn to_trade_event(a: &Activity) -> Option<TradeEvent> {
    if a.activity_type != ActivityType::Trade {
        return None;
    }
    let token = a.asset?;
    let side = match a.side.as_ref()? {
        DataSide::Buy => TradeSide::Buy,
        DataSide::Sell => TradeSide::Sell,
        _ => return None,
    };
    let price = a.price?;
    Some(TradeEvent {
        token_id: token.to_string(),
        side,
        price,
    })
}

fn describe_action(action: &CopyAction) -> String {
    match action {
        CopyAction::Buy { usd, .. } => format!("copy buy ${usd}"),
        CopyAction::Sell { shares, .. } => format!("copy sell {shares} shares"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn trader() -> CopyTrader {
        CopyTrader {
            id: "whale".into(),
            wallet: "0x0000000000000000000000000000000000000001".into(),
            nickname: "Whale".into(),
            copy_size_usd: dec!(25),
            max_dollar_cap: dec!(100),
            price_min: dec!(0.20),
            price_max: dec!(0.80),
            slippage_pct: dec!(2),
            mirror_sells: true,
            enabled: true,
        }
    }

    fn buy(price: Decimal) -> TradeEvent {
        TradeEvent {
            token_id: "123".into(),
            side: TradeSide::Buy,
            price,
        }
    }

    fn sell(price: Decimal) -> TradeEvent {
        TradeEvent {
            token_id: "123".into(),
            side: TradeSide::Sell,
            price,
        }
    }

    #[test]
    fn copies_buy_inside_band_at_fixed_size() {
        let d = decide(&trader(), &buy(dec!(0.50)), dec!(0));
        assert_eq!(
            d,
            Some(CopyDecision::Act(CopyAction::Buy {
                token_id: "123".into(),
                usd: dec!(25),
            }))
        );
    }

    #[test]
    fn size_is_capped_by_max_dollar() {
        let mut t = trader();
        t.copy_size_usd = dec!(500);
        t.max_dollar_cap = dec!(80);
        let d = decide(&t, &buy(dec!(0.50)), dec!(0));
        assert_eq!(
            d,
            Some(CopyDecision::Act(CopyAction::Buy {
                token_id: "123".into(),
                usd: dec!(80),
            }))
        );
    }

    #[test]
    fn skips_buy_below_band() {
        let d = decide(&trader(), &buy(dec!(0.10)), dec!(0));
        assert!(matches!(d, Some(CopyDecision::Skip(_))));
    }

    #[test]
    fn skips_buy_above_band() {
        let d = decide(&trader(), &buy(dec!(0.95)), dec!(0));
        assert!(matches!(d, Some(CopyDecision::Skip(_))));
    }

    #[test]
    fn mirrors_sell_of_held_position() {
        let d = decide(&trader(), &sell(dec!(0.50)), dec!(40));
        assert_eq!(
            d,
            Some(CopyDecision::Act(CopyAction::Sell {
                token_id: "123".into(),
                shares: dec!(40),
            }))
        );
    }

    #[test]
    fn skips_sell_when_flat() {
        let d = decide(&trader(), &sell(dec!(0.50)), dec!(0));
        assert!(matches!(d, Some(CopyDecision::Skip(_))));
    }

    #[test]
    fn ignores_sell_when_mirror_disabled() {
        let mut t = trader();
        t.mirror_sells = false;
        assert_eq!(decide(&t, &sell(dec!(0.50)), dec!(40)), None);
    }

    #[test]
    fn sell_price_band_does_not_apply() {
        // A sell well outside the buy band still mirrors (we're closing risk).
        let d = decide(&trader(), &sell(dec!(0.99)), dec!(40));
        assert!(matches!(
            d,
            Some(CopyDecision::Act(CopyAction::Sell { .. }))
        ));
    }
}
