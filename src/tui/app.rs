//! TUI application state and input handling.

use std::str::FromStr;
use std::sync::{Arc, Mutex};

use chrono::Utc;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use polymarket_client_sdk_v2::types::Decimal;

use super::data::{MarketRow, Shared};
use super::live::{LiveOrder, WalletInfo};
use crate::paper::engine as paper_engine;
use crate::paper::store;
use crate::paper::types::{
    MarketMeta, OrderKind, PaperAccount, Quote, TradeSide, default_starting_balance,
};
use crate::settings::{self, Settings};
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
    /// Take-profit percent (buys only); blank = none.
    pub tp: String,
    /// Stop-loss percent (buys only); blank = none.
    pub sl: String,
    pub field: ModalField,
    pub error: Option<String>,
    /// Index into the relevant preset list for the `p` quick-fill cycle.
    pub preset_idx: usize,
    /// Shares currently held in this token (for quicksell % presets).
    pub held: Decimal,
    /// True once the trading-mode confirmation gate has been shown and the
    /// next Enter should actually send the order.
    pub awaiting_confirm: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ModalField {
    Amount,
    Price,
    Size,
    TakeProfit,
    StopLoss,
}

/// Which setting the inline editor is changing.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SettingField {
    Threshold,
    Quickbuy,
    Quicksell,
    Slippage,
    TakeProfit,
    StopLoss,
    Trailing,
}

impl SettingField {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Threshold => "Confirmation threshold ($)",
            Self::Quickbuy => "Quickbuy presets ($, comma list)",
            Self::Quicksell => "Quicksell presets (%, comma list)",
            Self::Slippage => "Slippage tolerance (%)",
            Self::TakeProfit => "Default take-profit (%, blank=off)",
            Self::StopLoss => "Default stop-loss (%, blank=off)",
            Self::Trailing => "Default trailing-stop (%, blank=off)",
        }
    }
}

/// Inline editor for a single setting value.
pub(crate) struct SettingsEditModal {
    pub field: SettingField,
    pub input: String,
    pub error: Option<String>,
}

/// A row on the Settings tab — either the trading-mode toggle or an editable
/// value. The order here is the on-screen order and the selection index.
#[derive(Clone, Copy)]
pub(crate) enum SettingRow {
    Mode,
    Field(SettingField),
}

pub(crate) const SETTING_ROWS: [SettingRow; 8] = [
    SettingRow::Mode,
    SettingRow::Field(SettingField::Threshold),
    SettingRow::Field(SettingField::Quickbuy),
    SettingRow::Field(SettingField::Quicksell),
    SettingRow::Field(SettingField::Slippage),
    SettingRow::Field(SettingField::TakeProfit),
    SettingRow::Field(SettingField::StopLoss),
    SettingRow::Field(SettingField::Trailing),
];

/// Turn a pasted polymarket.com URL into a searchable slug; other queries
/// pass through untouched. E.g.
/// `https://polymarket.com/event/will-x-happen?tid=1` → `will x happen`.
fn normalize_search_query(raw: &str) -> String {
    if !raw.contains("polymarket.com/") {
        return raw.to_string();
    }
    let path = raw.split("polymarket.com/").nth(1).unwrap_or(raw);
    let path = path.split(['?', '#']).next().unwrap_or(path);
    let slug = path
        .split('/')
        .rfind(|seg| !seg.is_empty() && *seg != "event" && *seg != "market")
        .unwrap_or(path);
    slug.replace('-', " ")
}

/// Render a decimal list as a comma string for the editor, e.g. `10, 25, 50`.
fn join_decimals(values: &[Decimal]) -> String {
    values
        .iter()
        .map(|v| v.normalize().to_string())
        .collect::<Vec<_>>()
        .join(", ")
}

/// New-strategy form: pick a plugin and a watchlist, create + start it.
pub(crate) struct StratModal {
    /// Index into `registry::available()`.
    pub kind_idx: usize,
    /// Comma-separated token IDs being typed.
    pub tokens: String,
    pub error: Option<String>,
}

/// Paper-account reset form: choose a starting balance, wipe everything else.
pub(crate) struct ResetModal {
    pub balance: String,
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
    pub settings_sel: usize,
    pub history_scroll: usize,
    pub logs_scroll: usize,

    /// PolyGun-style trading settings (mode, presets, slippage, TP/SL).
    pub settings: Settings,
    /// Configured wallet details (live mode); `None` in paper mode.
    pub wallet: Option<WalletInfo>,
    /// Whether the private key is currently revealed on the Settings tab.
    pub reveal_key: bool,
    /// Inline settings editor.
    pub settings_modal: Option<SettingsEditModal>,

    /// Markets search filter (active while `searching`).
    pub search: String,
    pub searching: bool,

    /// The market opened in MarketDetail and which outcome token is focused.
    pub detail: Option<MarketRow>,
    pub detail_token: usize,

    pub modal: Option<OrderModal>,
    /// New-strategy form (Strategies tab → `n`).
    pub strat_modal: Option<StratModal>,
    /// Paper-account reset form (Settings tab → `r`).
    pub reset_modal: Option<ResetModal>,
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
        let wallet = if live { super::live::wallet_info() } else { None };
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
            settings_sel: 0,
            history_scroll: 0,
            logs_scroll: 0,
            settings: settings::load(),
            wallet,
            reveal_key: false,
            settings_modal: None,
            search: String::new(),
            searching: false,
            detail: None,
            detail_token: 0,
            modal: None,
            strat_modal: None,
            reset_modal: None,
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

    /// Markets to show: the default top-by-volume list, or live search results
    /// from the Gamma search API when a query is active.
    pub fn filtered_markets(&self) -> Vec<MarketRow> {
        let query = self.search.trim();
        let d = self.data.lock().unwrap();
        if query.is_empty() {
            d.markets.clone()
        } else if d.search_results_query.eq_ignore_ascii_case(query) {
            d.search_results.clone()
        } else {
            // Search in flight — results for this query haven't arrived yet.
            Vec::new()
        }
    }

    /// Fire a Gamma search for the current query (the real search endpoint,
    /// not a filter over the loaded list). Pasted polymarket.com links are
    /// reduced to their slug so a copied URL jumps straight to the market.
    fn run_market_search(&mut self) {
        let raw = self.search.trim().to_string();
        self.markets_sel = 0;
        if raw.is_empty() {
            return;
        }
        let query = normalize_search_query(&raw);
        if query != raw {
            // Show the extracted slug so results visibly match the query.
            self.search = query.clone();
        }
        self.status = format!("Searching markets for “{query}”…");
        super::data::run_search(Arc::clone(&self.data), query);
    }

    /// Whether a search is active but its results haven't arrived yet.
    pub fn search_pending(&self) -> bool {
        let query = self.search.trim();
        if query.is_empty() {
            return false;
        }
        !self
            .data
            .lock()
            .unwrap()
            .search_results_query
            .eq_ignore_ascii_case(query)
    }

    pub fn on_key(&mut self, key: KeyEvent) {
        // Always-available exit: Ctrl+C / Ctrl+Q work everywhere, including
        // inside modals and the search box (raw mode swallows the default
        // Ctrl+C, so we handle it ourselves).
        if key.modifiers.contains(KeyModifiers::CONTROL)
            && matches!(key.code, KeyCode::Char('c') | KeyCode::Char('q'))
        {
            self.should_quit = true;
            return;
        }
        // Modals capture all input first.
        if self.modal.is_some() {
            self.modal_key(key);
            return;
        }
        if self.strat_modal.is_some() {
            self.strat_modal_key(key);
            return;
        }
        if self.reset_modal.is_some() {
            self.reset_modal_key(key);
            return;
        }
        if self.settings_modal.is_some() {
            self.settings_modal_key(key);
            return;
        }
        // Search box on Markets captures input.
        if self.searching {
            match key.code {
                KeyCode::Esc => {
                    self.searching = false;
                    self.search.clear();
                }
                KeyCode::Enter => {
                    self.searching = false;
                    self.run_market_search();
                }
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
                self.status = "Tab/1-9 switch views · ↑↓/jk move · Enter open · b/s order · c cancel · g attach strat · q or Ctrl+C quit".to_string();
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
                } else if self.view == View::Markets && !self.search.is_empty() {
                    self.search.clear();
                    self.markets_sel = 0;
                    self.status = "Search cleared.".to_string();
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
            // Settings: reveal/hide the private key (live wallet).
            KeyCode::Char('w') if self.view == View::Settings => {
                if self.wallet.is_some() {
                    self.reveal_key = !self.reveal_key;
                    self.status = if self.reveal_key {
                        "⚠ Private key revealed — anyone seeing your screen can drain the wallet. Press w to hide.".into()
                    } else {
                        "Private key hidden.".into()
                    };
                } else {
                    self.status = "No wallet configured (paper mode).".into();
                }
            }
            // Settings: reset the paper account.
            KeyCode::Char('r') if self.view == View::Settings => {
                if self.live {
                    self.status =
                        "Reset only applies to the paper account. Relaunch with `--paper`.".into();
                } else {
                    let current = self.account.lock().unwrap().initial_balance;
                    let prefill = if current > Decimal::ZERO {
                        current.round_dp(0).to_string()
                    } else {
                        default_starting_balance().round_dp(0).to_string()
                    };
                    self.reset_modal = Some(ResetModal {
                        balance: prefill,
                        error: None,
                    });
                }
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
                let len = if self.live {
                    self.data.lock().unwrap().live_orders.len()
                } else {
                    self.account.lock().unwrap().open_orders.len()
                };
                step(&mut self.orders_sel, len);
            }
            View::Strategies => {
                let len = self.engine.snapshot().len();
                step(&mut self.strategies_sel, len);
            }
            View::Settings => {
                step(&mut self.settings_sel, SETTING_ROWS.len());
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
        match self.view {
            View::Markets => {
                let markets = self.filtered_markets();
                if let Some(row) = markets.get(self.markets_sel) {
                    self.detail = Some(row.clone());
                    self.detail_token = 0;
                    self.view = View::MarketDetail;
                    self.status = format!("Opened: {}", row.question);
                }
            }
            View::Settings => self.activate_setting(),
            _ => {}
        }
    }

    // --- Settings editing --------------------------------------------------

    /// Act on the selected Settings row: cycle the trading mode in place, or
    /// open the inline editor for a value.
    fn activate_setting(&mut self) {
        let Some(row) = SETTING_ROWS.get(self.settings_sel) else {
            return;
        };
        match row {
            SettingRow::Mode => {
                self.settings.trading_mode = self.settings.trading_mode.next();
                self.persist_settings();
                self.status = format!(
                    "Trading mode → {} ({}).",
                    self.settings.trading_mode,
                    self.settings.trading_mode.describe()
                );
            }
            SettingRow::Field(field) => {
                let input = self.setting_current_value(*field);
                self.settings_modal = Some(SettingsEditModal {
                    field: *field,
                    input,
                    error: None,
                });
            }
        }
    }

    /// The current value of an editable setting, pre-filled into the editor.
    pub(crate) fn setting_current_value(&self, field: SettingField) -> String {
        let s = &self.settings;
        let opt = |v: Option<Decimal>| v.map(|d| d.normalize().to_string()).unwrap_or_default();
        match field {
            SettingField::Threshold => s.confirm_threshold_usd.normalize().to_string(),
            SettingField::Quickbuy => join_decimals(&s.quickbuy_presets),
            SettingField::Quicksell => join_decimals(&s.quicksell_presets),
            SettingField::Slippage => s.slippage_pct.normalize().to_string(),
            SettingField::TakeProfit => opt(s.default_take_profit_pct),
            SettingField::StopLoss => opt(s.default_stop_loss_pct),
            SettingField::Trailing => opt(s.default_trailing_stop_pct),
        }
    }

    fn settings_modal_key(&mut self, key: KeyEvent) {
        let Some(m) = self.settings_modal.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.settings_modal = None,
            KeyCode::Backspace => {
                m.input.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' || c == ',' || c == ' ' => {
                m.input.push(c);
            }
            KeyCode::Enter => self.submit_setting(),
            _ => {}
        }
    }

    fn submit_setting(&mut self) {
        let (field, raw) = match self.settings_modal.as_ref() {
            Some(m) => (m.field, m.input.trim().to_string()),
            None => return,
        };
        // An optional percent: blank clears it.
        let parse_opt_pct = |raw: &str| -> Result<Option<Decimal>, String> {
            if raw.is_empty() {
                return Ok(None);
            }
            match Decimal::from_str(raw) {
                Ok(v) if v > Decimal::ZERO => Ok(Some(v)),
                Ok(_) => Err("Enter a positive percent, or blank to turn off.".into()),
                Err(_) => Err(format!("'{raw}' is not a number.")),
            }
        };
        let result: Result<(), String> = (|| {
            match field {
                SettingField::Threshold => {
                    let v = Decimal::from_str(&raw).map_err(|_| "Enter a dollar amount.".to_string())?;
                    if v < Decimal::ZERO {
                        return Err("Threshold cannot be negative.".into());
                    }
                    self.settings.confirm_threshold_usd = v;
                }
                SettingField::Quickbuy => {
                    self.settings.quickbuy_presets =
                        settings::parse_number_list(&raw).map_err(|e| e.to_string())?;
                }
                SettingField::Quicksell => {
                    self.settings.quicksell_presets =
                        settings::parse_number_list(&raw).map_err(|e| e.to_string())?;
                }
                SettingField::Slippage => {
                    let v = Decimal::from_str(&raw).map_err(|_| "Enter a percent.".to_string())?;
                    if v < Decimal::ZERO {
                        return Err("Slippage cannot be negative.".into());
                    }
                    self.settings.slippage_pct = v;
                }
                SettingField::TakeProfit => {
                    self.settings.default_take_profit_pct = parse_opt_pct(&raw)?;
                }
                SettingField::StopLoss => {
                    self.settings.default_stop_loss_pct = parse_opt_pct(&raw)?;
                }
                SettingField::Trailing => {
                    self.settings.default_trailing_stop_pct = parse_opt_pct(&raw)?;
                }
            }
            Ok(())
        })();

        match result {
            Ok(()) => {
                self.persist_settings();
                self.settings_modal = None;
                self.status = "Setting saved.".into();
            }
            Err(e) => {
                if let Some(m) = self.settings_modal.as_mut() {
                    m.error = Some(e);
                }
            }
        }
    }

    fn persist_settings(&self) {
        let _ = settings::save(&self.settings);
    }

    // --- Order modal -------------------------------------------------------

    fn open_modal(&mut self, side: TradeSide) {
        let Some(d) = &self.detail else { return };
        let Some(token_id) = d.token_ids.get(self.detail_token) else {
            return;
        };
        let token_id = token_id.clone();
        let outcome = d
            .outcomes
            .get(self.detail_token)
            .cloned()
            .unwrap_or_else(|| format!("Outcome {}", self.detail_token + 1));
        // Prefill TP/SL on buys from the configured defaults.
        let pct = |v: Option<Decimal>| v.map(|d| d.normalize().to_string()).unwrap_or_default();
        let (tp, sl) = if side == TradeSide::Buy {
            (
                pct(self.settings.default_take_profit_pct),
                pct(self.settings.default_stop_loss_pct),
            )
        } else {
            (String::new(), String::new())
        };
        let held = self
            .account
            .lock()
            .unwrap()
            .positions
            .get(&token_id)
            .map_or(Decimal::ZERO, |p| p.size);
        self.modal = Some(OrderModal {
            token_id,
            question: d.question.clone(),
            outcome,
            side,
            kind: OrderKind::Market,
            amount: String::new(),
            price: String::new(),
            size: String::new(),
            tp,
            sl,
            field: ModalField::Amount,
            error: None,
            preset_idx: 0,
            held,
            awaiting_confirm: false,
        });
    }

    fn modal_key(&mut self, key: KeyEvent) {
        // Enter at the confirmation gate sends; Esc anywhere cancels.
        match key.code {
            KeyCode::Esc => {
                self.modal = None;
                return;
            }
            KeyCode::Enter => {
                self.submit_modal();
                return;
            }
            KeyCode::Char('p') => {
                self.apply_preset();
                return;
            }
            _ => {}
        }
        let Some(m) = self.modal.as_mut() else { return };
        // Any edit invalidates a pending confirmation.
        match key.code {
            KeyCode::Char('m') => {
                m.kind = OrderKind::Market;
                m.field = ModalField::Amount;
                m.awaiting_confirm = false;
            }
            KeyCode::Char('L') => {
                m.kind = OrderKind::Limit;
                m.field = ModalField::Price;
                m.awaiting_confirm = false;
            }
            KeyCode::Tab => {
                m.field = next_field(m.kind, m.side, m.field);
            }
            KeyCode::Backspace => {
                field_mut(m).pop();
                m.awaiting_confirm = false;
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => {
                field_mut(m).push(c);
                m.awaiting_confirm = false;
            }
            _ => {}
        }
    }

    /// `p` quick-fill: cycle through quickbuy ($) presets on buys, or quicksell
    /// (% of the held position) presets on sells.
    fn apply_preset(&mut self) {
        let Some(m) = self.modal.as_mut() else { return };
        m.awaiting_confirm = false;
        match m.side {
            TradeSide::Buy => {
                let presets = &self.settings.quickbuy_presets;
                if presets.is_empty() {
                    return;
                }
                let v = presets[m.preset_idx % presets.len()];
                m.preset_idx += 1;
                let s = v.normalize().to_string();
                match m.kind {
                    OrderKind::Market => {
                        m.amount = s;
                        m.field = ModalField::Amount;
                    }
                    // For limit buys the preset seeds the size (shares).
                    OrderKind::Limit => {
                        m.size = s;
                        m.field = ModalField::Size;
                    }
                }
            }
            TradeSide::Sell => {
                let presets = &self.settings.quicksell_presets;
                if presets.is_empty() || m.held <= Decimal::ZERO {
                    return;
                }
                let pct = presets[m.preset_idx % presets.len()];
                m.preset_idx += 1;
                let shares = (m.held * pct / Decimal::ONE_HUNDRED).round_dp(2);
                let s = shares.normalize().to_string();
                match m.kind {
                    OrderKind::Market => {
                        m.amount = s;
                        m.field = ModalField::Amount;
                    }
                    OrderKind::Limit => {
                        m.size = s;
                        m.field = ModalField::Size;
                    }
                }
            }
        }
    }

    fn submit_modal(&mut self) {
        // Trading-mode confirmation gate (Cautious / Standard threshold).
        if self.confirm_gate() {
            return;
        }
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
        let slippage = self.settings.slippage_pct;
        let result: anyhow::Result<String> = (|| {
            let mut acct = self.account.lock().unwrap();
            match (m.kind, m.side) {
                (OrderKind::Market, TradeSide::Buy) => {
                    let usd = parse_dec(&m.amount)?;
                    paper_engine::check_slippage(&book.asks, TradeSide::Buy, usd, slippage)?;
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
                    paper_engine::check_slippage(&book.bids, TradeSide::Sell, shares, slippage)?;
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
                let exit = self.attach_exit_from_modal();
                self.status = match exit {
                    Some(note) => format!("[paper] {msg} · {note}"),
                    None => format!("[paper] {msg}"),
                };
                self.modal = None;
            }
            Err(e) => {
                if let Some(m) = self.modal.as_mut() {
                    m.error = Some(e.to_string());
                }
            }
        }
    }

    /// Whether the order needs confirmation and we've just asked for it (so the
    /// caller should stop and wait for the next Enter). Sets the prompt.
    fn confirm_gate(&mut self) -> bool {
        let already = self.modal.as_ref().is_some_and(|m| m.awaiting_confirm);
        if already {
            return false; // confirmed — proceed
        }
        let Some(notional) = self.order_notional() else {
            return false; // can't size it; let downstream validate
        };
        if !self.settings.requires_confirmation(notional) {
            return false;
        }
        let mode = self.settings.trading_mode;
        if let Some(m) = self.modal.as_mut() {
            m.awaiting_confirm = true;
            m.error = None;
            self.status = format!(
                "Confirm {} ${:.2} [{} mode] — press Enter to send, Esc to cancel.",
                m.side, notional, mode
            );
        }
        true
    }

    /// Best-effort notional (pUSD) of the order in the open modal, for the
    /// confirmation gate. `None` when it can't be sized yet.
    fn order_notional(&self) -> Option<Decimal> {
        let m = self.modal.as_ref()?;
        match m.kind {
            OrderKind::Market => {
                let amt = Decimal::from_str(m.amount.trim()).ok()?;
                match m.side {
                    TradeSide::Buy => Some(amt), // pUSD spent
                    TradeSide::Sell => {
                        // shares * best bid (fallback to mid).
                        let d = self.data.lock().unwrap();
                        let mark = d.book(&m.token_id).and_then(|b| b.best_bid.or(b.best_ask));
                        mark.map(|p| (p * amt).abs())
                    }
                }
            }
            OrderKind::Limit => {
                let price = Decimal::from_str(m.price.trim()).ok()?;
                let size = Decimal::from_str(m.size.trim()).ok()?;
                Some(price * size)
            }
        }
    }

    /// After a buy, attach (or replace) a take-profit/stop-loss guard on the
    /// token using the modal's TP/SL fields plus the default trailing stop.
    /// Returns a short note for the status line, or `None` if nothing attached.
    fn attach_exit_from_modal(&mut self) -> Option<String> {
        let m = self.modal.as_ref()?;
        if m.side != TradeSide::Buy {
            return None;
        }
        let token_id = m.token_id.clone();
        let tp = parse_opt_pct(&m.tp);
        let sl = parse_opt_pct(&m.sl);
        let trailing = self.settings.default_trailing_stop_pct.map(dec_to_f64);
        if tp.is_none() && sl.is_none() && trailing.is_none() {
            return None;
        }
        let params = serde_json::json!({
            "take_profit_pct": tp,
            "stop_loss_pct": sl,
            "trailing_stop_pct": trailing,
            "sell_fraction": 1.0,
        });
        let id = format!("exit-{}", &token_id[..token_id.len().min(6)]);
        // One guard per token: drop any existing one, then attach fresh.
        let _ = self.engine.remove(&id);
        match self.engine.add_with_params(&id, "tp_sl", vec![token_id], params) {
            Ok(()) => {
                let _ = self.engine.start(&id);
                let mut bits = Vec::new();
                if let Some(v) = tp {
                    bits.push(format!("TP +{v:.0}%"));
                }
                if let Some(v) = sl {
                    bits.push(format!("SL -{v:.0}%"));
                }
                if let Some(v) = trailing {
                    bits.push(format!("trail {v:.0}%"));
                }
                Some(format!("guard armed ({})", bits.join(", ")))
            }
            Err(_) => None,
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
                Ok(amount) => {
                    // Estimate slippage from the freshest cached book before
                    // sending the FOK order to the CLOB.
                    let levels = {
                        let d = self.data.lock().unwrap();
                        d.book(&token_id).map(|b| match side {
                            TradeSide::Buy => b.asks.clone(),
                            TradeSide::Sell => b.bids.clone(),
                        })
                    };
                    if let Some(levels) = levels
                        && let Err(e) = paper_engine::check_slippage(
                            &levels,
                            side,
                            amount,
                            self.settings.slippage_pct,
                        )
                    {
                        return self.set_modal_error(e.to_string());
                    }
                    LiveOrder::Market {
                        token_id,
                        side,
                        amount,
                    }
                }
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
        // Arm a TP/SL guard now; the engine watches the live position once the
        // refresher hydrates it.
        let exit = self.attach_exit_from_modal();
        self.status = match exit {
            Some(note) => format!("Submitting live order… · {note}"),
            None => "Submitting live order to the CLOB…".into(),
        };
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
            // Token IDs are decimal or 0x-hex strings.
            KeyCode::Char(c) if c.is_ascii_alphanumeric() || c == ',' => m.tokens.push(c),
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

    // --- Reset paper account ----------------------------------------------

    fn reset_modal_key(&mut self, key: KeyEvent) {
        let Some(m) = self.reset_modal.as_mut() else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.reset_modal = None,
            KeyCode::Backspace => {
                m.balance.pop();
            }
            KeyCode::Char(c) if c.is_ascii_digit() || c == '.' => m.balance.push(c),
            KeyCode::Enter => self.submit_reset(),
            _ => {}
        }
    }

    fn submit_reset(&mut self) {
        let bal_s = match self.reset_modal.as_ref() {
            Some(m) => m.balance.clone(),
            None => return,
        };
        let balance = if bal_s.trim().is_empty() {
            default_starting_balance()
        } else {
            match parse_dec(&bal_s) {
                Ok(b) => b,
                Err(e) => {
                    if let Some(m) = self.reset_modal.as_mut() {
                        m.error = Some(e.to_string());
                    }
                    return;
                }
            }
        };
        if balance <= Decimal::ZERO {
            if let Some(m) = self.reset_modal.as_mut() {
                m.error = Some("Balance must be positive.".into());
            }
            return;
        }

        // Wipe the account and start fresh; the engine shares this handle.
        {
            let mut acct = self.account.lock().unwrap();
            *acct = PaperAccount::new(balance, true);
            let _ = store::save(&acct);
        }
        self.positions_sel = 0;
        self.orders_sel = 0;
        self.history_scroll = 0;
        self.status = format!(
            "Paper account reset — fresh ${} balance, positions and history cleared.",
            balance.round_dp(2)
        );
        self.reset_modal = None;
    }

    // --- Orders ------------------------------------------------------------

    fn cancel_selected_order(&mut self) {
        if self.live {
            self.cancel_selected_live_order();
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

    /// Cancel the selected live order at the CLOB, in the background. The
    /// order disappears from the list optimistically; the next slow refresh
    /// re-syncs from the CLOB either way.
    fn cancel_selected_live_order(&mut self) {
        let order = {
            let d = self.data.lock().unwrap();
            d.live_orders.get(self.orders_sel).cloned()
        };
        let Some(order) = order else {
            self.status = "No live order selected.".into();
            return;
        };
        {
            let mut d = self.data.lock().unwrap();
            d.live_orders.retain(|o| o.id != order.id);
        }
        let shared = Arc::clone(&self.data);
        let engine = self.engine.clone();
        let id = order.id.clone();
        tokio::spawn(async move {
            let (level, msg) = match super::live::cancel_order(&id).await {
                Ok(s) => (LogLevel::Trade, s),
                Err(e) => (LogLevel::Error, format!("Cancel FAILED: {e}")),
            };
            engine.log(level, "live", &msg);
            shared.lock().unwrap().notices.push(msg);
        });
        self.status = format!(
            "Cancelling live order {}…",
            &order.id[..order.id.len().min(12)]
        );
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
        ModalField::TakeProfit => &mut m.tp,
        ModalField::StopLoss => &mut m.sl,
    }
}

/// The editable fields, in Tab order, for the given order kind and side.
pub(crate) fn modal_fields(kind: OrderKind, side: TradeSide) -> Vec<ModalField> {
    let mut fields = match kind {
        OrderKind::Market => vec![ModalField::Amount],
        OrderKind::Limit => vec![ModalField::Price, ModalField::Size],
    };
    // Take-profit / stop-loss apply to buys only (they exit a new position).
    if side == TradeSide::Buy {
        fields.push(ModalField::TakeProfit);
        fields.push(ModalField::StopLoss);
    }
    fields
}

fn next_field(kind: OrderKind, side: TradeSide, field: ModalField) -> ModalField {
    let fields = modal_fields(kind, side);
    let idx = fields.iter().position(|f| *f == field).unwrap_or(0);
    fields[(idx + 1) % fields.len()]
}

fn parse_dec(s: &str) -> anyhow::Result<Decimal> {
    Decimal::from_str(s.trim()).map_err(|_| anyhow::anyhow!("Enter a number (got '{s}')"))
}

/// Parse an optional percent field: blank → `None`, else `Some(value)`.
fn parse_opt_pct(s: &str) -> Option<f64> {
    let t = s.trim();
    if t.is_empty() {
        return None;
    }
    t.parse::<f64>().ok().filter(|v| *v > 0.0)
}

fn dec_to_f64(d: Decimal) -> f64 {
    use std::str::FromStr as _;
    f64::from_str(&d.to_string()).unwrap_or(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plain_queries_pass_through() {
        assert_eq!(normalize_search_query("bitcoin etf"), "bitcoin etf");
    }

    #[test]
    fn event_urls_reduce_to_slug() {
        assert_eq!(
            normalize_search_query("https://polymarket.com/event/will-x-happen?tid=99"),
            "will x happen"
        );
    }

    #[test]
    fn nested_market_urls_take_last_segment() {
        assert_eq!(
            normalize_search_query("https://polymarket.com/event/fed-rates/fed-cut-in-june"),
            "fed cut in june"
        );
    }

    #[test]
    fn modal_fields_include_tp_sl_on_buys_only() {
        let buy = modal_fields(OrderKind::Market, TradeSide::Buy);
        assert!(buy.contains(&ModalField::TakeProfit));
        assert!(buy.contains(&ModalField::StopLoss));
        let sell = modal_fields(OrderKind::Market, TradeSide::Sell);
        assert_eq!(sell, vec![ModalField::Amount]);
    }

    #[test]
    fn tab_cycles_through_buy_fields() {
        let f = ModalField::Amount;
        let f = next_field(OrderKind::Market, TradeSide::Buy, f);
        assert_eq!(f, ModalField::TakeProfit);
        let f = next_field(OrderKind::Market, TradeSide::Buy, f);
        assert_eq!(f, ModalField::StopLoss);
        let f = next_field(OrderKind::Market, TradeSide::Buy, f);
        assert_eq!(f, ModalField::Amount);
    }

    #[test]
    fn opt_pct_parses_blank_and_values() {
        assert_eq!(parse_opt_pct(""), None);
        assert_eq!(parse_opt_pct("  "), None);
        assert_eq!(parse_opt_pct("25"), Some(25.0));
        assert_eq!(parse_opt_pct("-5"), None);
    }
}
