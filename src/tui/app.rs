//! TUI application state and input handling.

use std::sync::{Arc, Mutex};

use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent};
use polymarket_client_sdk_v2::types::Decimal;

use super::data::{MarketRow, Shared};
use super::live::LiveOrder;
use crate::paper::engine as paper_engine;
use crate::paper::store;
use crate::paper::types::{MarketMeta, OrderKind, PaperAccount, Quote, TradeSide};
use crate::strategy::engine::{LogLevel, StrategyEngine};
use crate::strategy::registry;

/// The screens of the terminal, in tab order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum View {
    Dashboard,
    Markets,
    MarketDetail,
    Portfolio,
    Positions,
    Orders,
    History,
    Strategies,
    Logs,
    Settings,
}

impl View {
    /// Tabs shown in the top bar (MarketDetail is reached from Markets, not a
    /// top-level tab).
    pub const TABS: [View; 9] = [
        View::Dashboard,
        View::Markets,
        View::Portfolio,
        View::Positions,
        View::Orders,
        View::History,
        View::Strategies,
        View::Logs,
        View::Settings,
    ];

    pub fn title(self) -> &'static str {
        match self {
            View::Dashboard => "Dashboard",
            View::Markets => "Markets",
            View::MarketDetail => "Market",
            View::Portfolio => "Portfolio",
            View::Positions => "Positions",
            View::Orders => "Orders",
            View::History => "History",
            View::Strategies => "Strategies",
            View::Logs => "Logs",
            View::Settings => "Settings",
        }
    }
}

/// Modal order-entry form.
pub(crate) struct OrderModal {
    pub token_id: String,
    pub question: String,
    pub outcome: String,
    pub side: TradeSide,
    pub kind: OrderKind,
    /// Market: pUSD (buy) or shares (sell). Limit: ignored.
    pub amount: String,
    pub price: String,
    pub size: String,
    pub field: ModalField,
    pub error: Option<String>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ModalField {
    Amount,
    Price,
    Size,
}

/// New-strategy form: pick a plugin and a watchlist, create + start it.
pub(crate) struct StratModal {
    /// Index into `registry::available()`.
    pub kind_idx: usize,
    /// Comma-separated token IDs being typed.
    pub tokens: String,
    pub error: Option<String>,
}

pub(crate) struct App {
    pub view: View,
    pub should_quit: bool,
    pub data: Shared,
    pub account: Arc<Mutex<PaperAccount>>,
    pub engine: StrategyEngine,

    pub markets_sel: usize,
    pub positions_sel: usize,
    pub orders_sel: usize,
    pub strategies_sel: usize,
    pub history_scroll: usize,
    pub logs_scroll: usize,

    /// Markets search filter (active while `searching`).
    pub search: String,
    pub searching: bool,

    /// The market opened in MarketDetail and which outcome token is focused.
    pub detail: Option<MarketRow>,
    pub detail_token: usize,

    pub modal: Option<OrderModal>,
    /// New-strategy form (Strategies tab → `n`).
    pub strat_modal: Option<StratModal>,
    pub status: String,
    /// True in LIVE mode (real wallet + CLOB), false for the paper account.
    pub live: bool,
}

impl App {
    pub fn new(
        data: Shared,
        account: Arc<Mutex<PaperAccount>>,
        engine: StrategyEngine,
        live: bool,
    ) -> Self {
        let status = if live {
            "LIVE mode — real funds. Press ? for help, b/s on a market to trade.".to_string()
        } else {
            "PAPER mode — simulated. Press ? for help.".to_string()
        };
        Self {
            view: View::Dashboard,
            should_quit: false,
            data,
            account,
            engine,
            markets_sel: 0,
            positions_sel: 0,
            orders_sel: 0,
            strategies_sel: 0,
            history_scroll: 0,
            logs_scroll: 0,
            search: String::new(),
            searching: false,
            detail: None,
            detail_token: 0,
            modal: None,
            strat_modal: None,
            status,
            live,
        }
    }

    /// Per-frame housekeeping: refresh the watch set and surface any async
    /// notices (e.g. live-order results) in the status line.
    pub fn pre_frame(&mut self) {
        self.sync_watch();
        let notice = self.data.lock().unwrap().notices.pop();
        if let Some(n) = notice {
            self.status = n;
        }
    }

    /// Tokens the data refresher should keep books fresh for.
    pub fn watched_tokens(&self) -> Vec<String> {
        let mut tokens: Vec<String> = self
            .account
            .lock()
            .unwrap()
            .positions
            .keys()
            .cloned()
            .collect();
        if let Some(d) = &self.detail {
            tokens.extend(d.token_ids.iter().cloned());
        }
        tokens.sort();
        tokens.dedup();
        tokens
    }

    /// Push the current watch set to the shared data store each frame.
    pub fn sync_watch(&self) {
        let tokens = self.watched_tokens();
        self.data.lock().unwrap().watch = tokens;
    }

    /// Markets passing the current search filter, with original indices.
    pub fn filtered_markets(&self) -> Vec<MarketRow> {
        let d = self.data.lock().unwrap();
        if self.search.is_empty() {
            d.markets.clone()
        } else {
            let needle = self.search.to_lowercase();
            d.markets
                .iter()
                .filter(|m| m.question.to_lowercase().contains(&needle))
                .cloned()
                .collect()
        }
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Modals capture all input first.
        if self.modal.is_some() {
            self.modal_key(key);
            return;
        }
        if self.strat_modal.is_some() {
            self.strat_modal_key(key);
            return;
        }
        // Search box on Markets captures input.
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.search.clear();
                }
                KeyCode::Enter => self.searching = false,
                KeyCode::Backspace => {
                    self.search.pop();
                }
                KeyCode::Char(c) => self.search.push(c),
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Char('q') => self.should_quit = true,
            KeyCode::Char('?') => {
                self.status = "Tab/1-9 switch views · ↑↓/jk move · Enter open · b/s order · c cancel · g attach strat · q quit".to_string();
            }
            KeyCode::Tab => self.cycle_tab(1),
            KeyCode::BackTab => self.cycle_tab(-1),
            KeyCode::Char(c @ '1'..='9') => {
                let idx = c as usize - '1' as usize;
                if idx < View::TABS.len() {
                    self.view = View::TABS[idx];
                }
            }
            KeyCode::Down | KeyCode::Char('j') => self.move_sel(1),
            KeyCode::Up | KeyCode::Char('k') => self.move_sel(-1),
            KeyCode::Enter => self.activate(),
            KeyCode::Esc => {
                if self.view == View::MarketDetail {
                    self.view = View::Markets;
                }
            }
            KeyCode::Char('/') if self.view == View::Markets => {
                self.searching = true;
                self.search.clear();
            }
            KeyCode::Left | KeyCode::Char('h') if self.view == View::MarketDetail => {
                if self.detail_token > 0 {
                    self.detail_token -= 1;
                }
            }
            KeyCode::Right | KeyCode::Char('l') if self.view == View::MarketDetail => {
                if let Some(d) = &self.detail
                    && self.detail_token + 1 < d.token_ids.len()
                {
                    self.detail_token += 1;
                }
            }
            KeyCode::Char('b') if self.view == View::MarketDetail => {
                self.open_modal(TradeSide::Buy)
            }
            KeyCode::Char('s') if self.view == View::MarketDetail => {
                self.open_modal(TradeSide::Sell)
            }
            KeyCode::Char('g') if self.view == View::MarketDetail => self.attach_strategy(),
            KeyCode::Char('c') if self.view == View::Orders => self.cancel_selected_order(),
            // Strategy controls
            KeyCode::Char('n') if self.view == View::Strategies => {
                self.strat_modal = Some(StratModal {
                    kind_idx: 0,
                    tokens: String::new(),
                    error: None,
                });
            }
            KeyCode::Char('s') if self.view == View::Strategies => {
                self.strategy_action(StratAct::Start)
            }
            KeyCode::Char('x') if self.view == View::Strategies => {
                self.strategy_action(StratAct::Stop)
            }
            KeyCode::Char('e') if self.view == View::Strategies => {
                self.strategy_action(StratAct::Enable)
            }
            KeyCode::Char('d') if self.view == View::Strategies => {
                self.strategy_action(StratAct::Disable)
            }
            _ => {}
        }
    }

    fn cycle_tab(&mut self, dir: i32) {
        let cur = View::TABS.iter().position(|v| *v == self.view).unwrap_or(0);
        let n = View::TABS.len() as i32;
        let next = (cur as i32 + dir).rem_euclid(n) as usize;
        self.view = View::TABS[next];
    }

    fn move_sel(&mut self, dir: i32) {
        let step = |sel: &mut usize, len: usize| {
            if len == 0 {
                *sel = 0;
                return;
            }
            let n = (*sel as i32 + dir).clamp(0, len as i32 - 1);
            *sel = n as usize;
        };
        match self.view {
            View::Markets => {
                let len = self.filtered_markets().len();
                step(&mut self.markets_sel, len);
            }
            View::Positions => {
                let len = self.account.lock().unwrap().positions.len();
                step(&mut self.positions_sel, len);
            }
            View::Orders => {
                let len = self.account.lock().unwrap().open_orders.len();
                step(&mut self.orders_sel, len);
            }
            View::Strategies => {
                let len = self.engine.snapshot().len();
                step(&mut self.strategies_sel, len);
            }
            View::History => {
                if dir > 0 {
                    self.history_scroll += 1;
                } else {
                    self.history_scroll = self.history_scroll.saturating_sub(1);
                }
            }
            View::Logs => {
                if dir > 0 {
                    self.logs_scroll += 1;
                } else {
                    self.logs_scroll = self.logs_scroll.saturating_sub(1);
                }
            }
            _ => {}
        }
    }

    fn activate(&mut self) {
        if self.view == View::Markets {
            let markets = self.filtered_markets();
            if let Some(row) = markets.get(self.markets_sel) {
                self.detail = Some(row.clone());
                self.detail_token = 0;
                self.view = View::MarketDetail;
                self.status = format!("Opened: {}", row.question);
            }
        }
    }

    // --- Order modal -------------------------------------------------------

    fn open_modal(&mut self, side: TradeSide) {
        let Some(d) = &self.detail else { return };
        let Some(token_id) = d.token_ids.get(self.detail_token) else {
            return;
        };
        let outcome = d
            .outcomes
            .get(self.detail_token)
            .cloned()
            .unwrap_or_else(|| format!("Outcome {}", self.detail_token + 1));
        self.modal = Some(OrderModal {
            token_id: token_id.clone(),
            question: d.question.clone(),
            outcome,
            side,
            kind: OrderKind::Market,
            amount: String::new(),
            price: String::new(),
            size: String::new(),
            field: ModalField::Amount,
            error: None,
        });
    }

    fn modal_key(&mut self, key: KeyEvent) {
        let Some(m) = self.modal.as_mut() else { return };
        match key.code {
            KeyCode::Esc => {
                self.modal = None;
            }
            KeyCode::Char('m') => {
                m.kind = OrderKind::Market;
                m.field = ModalField::Amount;
            }
            KeyCode::Char('L') => {
                m.kind = OrderKind::Limit;
                m.field = ModalField::Price;
            }
            KeyCode::Tab => {
                m.field = next_field(m.kind, m.field);
            }
            KeyCode::Backspace => {
                field_mut(m).pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
                field_mut(m).push(c);
            }
            KeyCode::Enter => {
                self.submit_modal();
            }
            _ => {}
        }
    }

    fn submit_modal(&mut self) {
        if self.live {
            self.submit_live_order();
            return;
        }
        let Some(m) = self.modal.as_ref() else { return };
        let token_id = m.token_id.clone();
        let meta = MarketMeta {
            question: m.question.clone(),
            outcome: m.outcome.clone(),
        };
        let book = {
            let d = self.data.lock().unwrap();
            d.book(&token_id).cloned()
        };
        let Some(book) = book else {
            if let Some(m) = self.modal.as_mut() {
                m.error = Some("No live book yet — wait for a refresh.".into());
            }
            return;
        };
        let now = Utc::now();
        let quote = Quote {
            best_bid: book.best_bid,
            best_ask: book.best_ask,
        };
        let result: anyhow::Result<String> = (|| {
            let mut acct = self.account.lock().unwrap();
            match (m.kind, m.side) {
                (OrderKind::Market, TradeSide::Buy) => {
                    let usd = parse_dec(&m.amount)?;
                    let t = paper_engine::market_buy(
                        &mut acct, &token_id, &meta, &book.asks, usd, now,
                    )?;
                    Ok(format!(
                        "Bought {} @ {}",
                        t.size.round_dp(2),
                        t.price.round_dp(4)
                    ))
                }
                (OrderKind::Market, TradeSide::Sell) => {
                    let shares = parse_dec(&m.amount)?;
                    let t =
                        paper_engine::market_sell(&mut acct, &token_id, &book.bids, shares, now)?;
                    Ok(format!(
                        "Sold {} @ {} (pnl {})",
                        t.size.round_dp(2),
                        t.price.round_dp(4),
                        t.realized_pnl.unwrap_or_default().round_dp(2)
                    ))
                }
                (OrderKind::Limit, TradeSide::Buy) => {
                    let price = parse_dec(&m.price)?;
                    let size = parse_dec(&m.size)?;
                    match paper_engine::limit_buy(
                        &mut acct, &token_id, &meta, quote, price, size, now,
                    )? {
                        paper_engine::LimitOutcome::Filled(t) => Ok(format!(
                            "Limit buy filled {} @ {}",
                            t.size.round_dp(2),
                            t.price
                        )),
                        paper_engine::LimitOutcome::Resting(o) => Ok(format!(
                            "Limit buy resting #{} {} @ {}",
                            o.id,
                            o.size.round_dp(2),
                            o.price
                        )),
                    }
                }
                (OrderKind::Limit, TradeSide::Sell) => {
                    let price = parse_dec(&m.price)?;
                    let size = parse_dec(&m.size)?;
                    match paper_engine::limit_sell(&mut acct, &token_id, quote, price, size, now)? {
                        paper_engine::LimitOutcome::Filled(t) => Ok(format!(
                            "Limit sell filled {} @ {}",
                            t.size.round_dp(2),
                            t.price
                        )),
                        paper_engine::LimitOutcome::Resting(o) => Ok(format!(
                            "Limit sell resting #{} {} @ {}",
                            o.id,
                            o.size.round_dp(2),
                            o.price
                        )),
                    }
                }
            }
        })();

        match result {
            Ok(msg) => {
                let _ = store::save(&self.account.lock().unwrap());
                self.status = format!("[paper] {msg}");
                self.modal = None;
            }
            Err(e) => {
                if let Some(m) = self.modal.as_mut() {
                    m.error = Some(e.to_string());
                }
            }
        }
    }

    /// Build a real order from the modal and submit it to the CLOB in the
    /// background; the result lands in the status line and the Logs tab.
    fn submit_live_order(&mut self) {
        let (token_id, side, kind, amount_s, price_s, size_s) = {
            let Some(m) = self.modal.as_ref() else { return };
            (
                m.token_id.clone(),
                m.side,
                m.kind,
                m.amount.clone(),
                m.price.clone(),
                m.size.clone(),
            )
        };
        let order = match kind {
            OrderKind::Market => match parse_dec(&amount_s) {
                Ok(amount) => LiveOrder::Market {
                    token_id,
                    side,
                    amount,
                },
                Err(e) => return self.set_modal_error(e.to_string()),
            },
            OrderKind::Limit => {
                let price = match parse_dec(&price_s) {
                    Ok(p) => p,
                    Err(e) => return self.set_modal_error(e.to_string()),
                };
                let size = match parse_dec(&size_s) {
                    Ok(s) => s,
                    Err(e) => return self.set_modal_error(e.to_string()),
                };
                LiveOrder::Limit {
                    token_id,
                    side,
                    price,
                    size,
                }
            }
        };

        let shared = Arc::clone(&self.data);
        let engine = self.engine.clone();
        tokio::spawn(async move {
            let (level, msg) = match super::live::place(order).await {
                Ok(s) => (LogLevel::Trade, s),
                Err(e) => (LogLevel::Error, format!("Live order FAILED: {e}")),
            };
            engine.log(level, "live", &msg);
            shared.lock().unwrap().notices.push(msg);
        });
        self.status = "Submitting live order to the CLOB…".into();
        self.modal = None;
    }

    fn set_modal_error(&mut self, e: String) {
        if let Some(m) = self.modal.as_mut() {
            m.error = Some(e);
        }
    }

    // --- New-strategy modal -----------------------------------------------

    fn strat_modal_key(&mut self, key: KeyEvent) {
        let n = registry::available().len();
        let Some(m) = self.strat_modal.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.strat_modal = None,
            KeyCode::Left => m.kind_idx = m.kind_idx.saturating_sub(1),
            KeyCode::Right => {
                if m.kind_idx + 1 < n {
                    m.kind_idx += 1;
                }
            }
            KeyCode::Backspace => {
                m.tokens.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == ',' => m.tokens.push(c),
            KeyCode::Enter => self.submit_strat_modal(),
            _ => {}
        }
    }

    fn submit_strat_modal(&mut self) {
        let (kind, tokens_s) = {
            let Some(m) = self.strat_modal.as_ref() else {
                return;
            };
            let avail = registry::available();
            (avail[m.kind_idx].kind.to_string(), m.tokens.clone())
        };
        let tokens: Vec<String> = tokens_s
            .split(',')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
        if tokens.is_empty() {
            if let Some(m) = self.strat_modal.as_mut() {
                m.error = Some("Enter at least one token ID".into());
            }
            return;
        }
        let id = self.unique_strategy_id(&kind);
        match self.engine.add(&id, &kind, tokens) {
            Ok(()) => {
                let _ = self.engine.start(&id);
                self.status = format!("Created strategy '{id}' ({kind}) and started it.");
                self.strat_modal = None;
            }
            Err(e) => {
                if let Some(m) = self.strat_modal.as_mut() {
                    m.error = Some(e.to_string());
                }
            }
        }
    }

    fn unique_strategy_id(&self, kind: &str) -> String {
        let existing: Vec<String> = self.engine.snapshot().into_iter().map(|s| s.id).collect();
        if !existing.iter().any(|e| e == kind) {
            return kind.to_string();
        }
        (2..)
            .map(|n| format!("{kind}-{n}"))
            .find(|cand| !existing.contains(cand))
            .unwrap_or_else(|| kind.to_string())
    }

    // --- Orders ------------------------------------------------------------

    fn cancel_selected_order(&mut self) {
        if self.live {
            self.status =
                "Live order cancel from the TUI is coming soon — use `polymarket clob cancel <id>`."
                    .into();
            return;
        }
        let id = {
            let acct = self.account.lock().unwrap();
            acct.open_orders.get(self.orders_sel).map(|o| o.id)
        };
        let Some(id) = id else { return };
        let mut acct = self.account.lock().unwrap();
        match paper_engine::cancel_order(&mut acct, id) {
            Ok(o) => {
                let _ = store::save(&acct);
                drop(acct);
                self.status = format!(
                    "Cancelled order #{} ({} {} @ {})",
                    o.id, o.side, o.size, o.price
                );
            }
            Err(e) => self.status = e.to_string(),
        }
    }

    // --- Strategies --------------------------------------------------------

    fn attach_strategy(&mut self) {
        let Some(d) = &self.detail else { return };
        let Some(token_id) = d.token_ids.get(self.detail_token).cloned() else {
            return;
        };
        let id = format!("momentum-{}", &token_id[..token_id.len().min(6)]);
        match self.engine.add(&id, "momentum", vec![token_id]) {
            Ok(()) => {
                let _ = self.engine.start(&id);
                self.status =
                    format!("Attached momentum strategy '{id}' (running). See Strategies tab.");
            }
            Err(e) => self.status = format!("Attach failed: {e}"),
        }
    }

    fn strategy_action(&mut self, act: StratAct) {
        let snap = self.engine.snapshot();
        let Some(s) = snap.get(self.strategies_sel) else {
            return;
        };
        let id = s.id.clone();
        let res = match act {
            StratAct::Start => self.engine.start(&id),
            StratAct::Stop => self.engine.stop(&id),
            StratAct::Enable => self.engine.set_enabled(&id, true),
            StratAct::Disable => self.engine.set_enabled(&id, false),
        };
        self.status = match res {
            Ok(()) => format!("{} {}", act.verb(), id),
            Err(e) => e.to_string(),
        };
    }
}

enum StratAct {
    Start,
    Stop,
    Enable,
    Disable,
}

impl StratAct {
    fn verb(&self) -> &'static str {
        match self {
            StratAct::Start => "Started",
            StratAct::Stop => "Stopped",
            StratAct::Enable => "Enabled",
            StratAct::Disable => "Disabled",
        }
    }
}

fn field_mut(m: &mut OrderModal) -> &mut String {
    match m.field {
        ModalField::Amount => &mut m.amount,
        ModalField::Price => &mut m.price,
        ModalField::Size => &mut m.size,
    }
}

fn next_field(kind: OrderKind, field: ModalField) -> ModalField {
    match kind {
        OrderKind::Market => ModalField::Amount,
        OrderKind::Limit => match field {
            ModalField::Price => ModalField::Size,
            _ => ModalField::Price,
        },
    }
}

fn parse_dec(s: &str) -> anyhow::Result<Decimal> {
    use std::str::FromStr;
    Decimal::from_str(s.trim()).map_err(|_| anyhow::anyhow!("Enter a number (got '{s}')"))
}
