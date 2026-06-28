//! Shared, live market data for the TUI.
//!
//! A background task ([`refresher`]) periodically pulls the markets list and
//! the order books for whatever tokens the UI is currently watching, writing
//! everything into a single [`SharedData`] behind a mutex. The render loop
//! only ever reads this struct, so drawing never blocks on the network. This
//! task also ticks the TP/SL [`crate::guard`]s against the fresh books.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use polymarket_client_sdk_v2::clob::types::request::OrderBookSummaryRequest;
use polymarket_client_sdk_v2::gamma;
use polymarket_client_sdk_v2::types::{Address, Decimal};

use super::live;
use crate::guard::{self, GuardAction};
use crate::paper::engine as paper_engine;
use crate::paper::quotes;
use crate::paper::types::{PaperAccount, TradeSide};

/// A market flattened to just what the UI needs.
#[derive(Clone, Debug)]
pub(crate) struct MarketRow {
    pub id: String,
    pub question: String,
    pub token_ids: Vec<String>,
    pub outcomes: Vec<String>,
    pub prices: Vec<Decimal>,
    pub volume: Option<Decimal>,
    pub liquidity: Option<Decimal>,
    pub closed: Option<bool>,
    pub active: Option<bool>,
    /// Market rules / resolution criteria (Gamma `description`).
    pub description: Option<String>,
    pub resolution_source: Option<String>,
    pub end_date: Option<DateTime<Utc>>,
}

/// A token's live book, summarized.
#[derive(Clone, Debug, Default)]
pub(crate) struct BookView {
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub bids: Vec<(Decimal, Decimal)>,
    pub asks: Vec<(Decimal, Decimal)>,
}

/// Resolution facts for a token whose market has resolved, keyed off Gamma's
/// final outcome prices.
#[derive(Clone, Debug)]
pub(crate) struct ResolutionInfo {
    /// pUSD paid per share at settlement: 1 (won) or 0 (lost).
    pub payout: Decimal,
    pub won: bool,
    /// On-chain condition ID (0x hex), needed for live redemption.
    pub condition_id: Option<String>,
    pub neg_risk: bool,
    /// This token's index among the market's outcomes (neg-risk redemption
    /// takes per-outcome amounts).
    pub outcome_index: usize,
    pub outcome_count: usize,
}

#[derive(Default)]
pub(crate) struct SharedData {
    pub markets: Vec<MarketRow>,
    pub markets_status: String,
    pub books: HashMap<String, BookView>,
    pub marks: HashMap<String, Decimal>,
    /// Resolved markets among the held tokens, refreshed on the slow cadence.
    pub resolutions: HashMap<String, ResolutionInfo>,
    /// Tokens the UI wants fresh books for (positions + open market).
    pub watch: Vec<String>,
    pub last_refresh: Option<DateTime<Utc>>,
    pub connected: bool,
    /// Transient one-line notices (e.g. live-order results) for the status bar.
    pub notices: Vec<String>,
    /// Open orders at the CLOB (live mode only), refreshed on the slow cadence.
    pub live_orders: Vec<live::LiveOpenOrder>,
    /// Results of the most recent market search, and the query they answer
    /// (so the UI can tell fresh results from a stale/in-flight query).
    pub search_results: Vec<MarketRow>,
    pub search_results_query: String,
}

impl SharedData {
    pub fn book(&self, token_id: &str) -> Option<&BookView> {
        self.books.get(token_id)
    }
}

pub(crate) type Shared = Arc<Mutex<SharedData>>;

pub(crate) fn new_shared() -> Shared {
    Arc::new(Mutex::new(SharedData {
        markets_status: "loading…".to_string(),
        ..SharedData::default()
    }))
}

/// Background loop: refresh markets occasionally and watched books each pass.
/// In live mode (`live_user` set) it also hydrates the account with the
/// wallet's real balance and positions.
pub(crate) async fn refresher(
    shared: Shared,
    account: Arc<Mutex<PaperAccount>>,
    live_user: Option<Address>,
) {
    let gamma = gamma::Client::default();
    let mut market_ticks = 0u32;
    // Highest mark seen per token, for trailing-stop guards.
    let mut guard_peaks: HashMap<String, Decimal> = HashMap::new();
    loop {
        // Slow cadence: market list, resolutions, balance, open orders (~15s).
        if market_ticks == 0 {
            if let Some(_user) = live_user {
                if let Ok(cash) = live::fetch_collateral().await {
                    account.lock().unwrap().cash = cash;
                }
                if let Ok(orders) = live::fetch_open_orders().await {
                    shared.lock().unwrap().live_orders = orders;
                }
            }
            match fetch_markets(&gamma).await {
                Ok(rows) => {
                    let mut d = shared.lock().unwrap();
                    d.markets_status = format!("{} markets", rows.len());
                    d.markets = rows;
                    d.connected = true;
                }
                Err(e) => {
                    let mut d = shared.lock().unwrap();
                    d.markets_status = format!("error: {e}");
                    d.connected = false;
                }
            }
            let held: Vec<String> = account.lock().unwrap().positions.keys().cloned().collect();
            if !held.is_empty()
                && let Ok(resolved) = fetch_resolutions(&gamma, &held).await
                && !resolved.is_empty()
            {
                let mut d = shared.lock().unwrap();
                for (token, info) in &resolved {
                    d.marks.insert(token.clone(), info.payout);
                }
                d.resolutions.extend(resolved);
            }
        }
        market_ticks = (market_ticks + 1) % 300;

        // Concurrent: live positions + batch book refresh.
        let watch = shared.lock().unwrap().watch.clone();

        let user = live_user;
        let (live_positions, (books, marks)) = tokio::join!(
            async {
                if let Some(u) = user {
                    live::fetch_positions(u).await.unwrap_or_default()
                } else {
                    Vec::new()
                }
            },
            async {
                let mut books: HashMap<String, BookView> = HashMap::new();
                let mut marks: HashMap<String, Decimal> = HashMap::new();
                if !watch.is_empty()
                    && let Ok(client) = crate::auth::unauthenticated_clob_client()
                {
                    let requests: Vec<_> = watch
                        .iter()
                        .filter_map(|tid| {
                            let token = quotes::parse_token_id(tid).ok()?;
                            Some(OrderBookSummaryRequest::builder().token_id(token).build())
                        })
                        .collect();
                    if !requests.is_empty()
                        && let Ok(responses) = client.order_books(&requests).await
                    {
                        for resp in responses {
                            let tid = resp.asset_id.to_string();
                            let mut bids: Vec<(Decimal, Decimal)> =
                                resp.bids.iter().map(|l| (l.price, l.size)).collect();
                            let mut asks: Vec<(Decimal, Decimal)> =
                                resp.asks.iter().map(|l| (l.price, l.size)).collect();
                            bids.sort_by_key(|&(price, _)| std::cmp::Reverse(price));
                            asks.sort_by_key(|&(price, _)| price);
                            let best_bid = bids.first().map(|&(p, _)| p);
                            let best_ask = asks.first().map(|&(p, _)| p);
                            let mid = match (best_bid, best_ask) {
                                (Some(b), Some(a)) => Some((b + a) / Decimal::from(2)),
                                (Some(b), None) => Some(b),
                                (None, Some(a)) => Some(a),
                                (None, None) => None,
                            };
                            if let Some(m) = mid {
                                marks.insert(tid.clone(), m);
                            }
                            books.insert(
                                tid,
                                BookView {
                                    best_bid,
                                    best_ask,
                                    bids,
                                    asks,
                                },
                            );
                        }
                    }
                }
                (books, marks)
            },
        );

        // Hydrate live positions every pass.
        if live_user.is_some() {
            let mut a = account.lock().unwrap();
            a.positions.clear();
            for p in live_positions {
                a.positions.insert(p.token_id.clone(), p);
            }
        }

        // Write fresh books and marks into shared state.
        {
            let mut d = shared.lock().unwrap();
            for (k, v) in &books {
                d.books.insert(k.clone(), v.clone());
            }
            for (k, v) in &marks {
                if !d.resolutions.contains_key(k) {
                    d.marks.insert(k.clone(), *v);
                }
            }
        }

        // Evaluate TP/SL guards against the fresh books.
        if !books.is_empty() {
            tick_guards(
                &shared,
                &account,
                &books,
                &marks,
                live_user.is_some(),
                &mut guard_peaks,
            );
        }

        shared.lock().unwrap().last_refresh = Some(Utc::now());
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

/// Evaluate every armed TP/SL guard against the freshest books and exit any
/// position whose threshold has been crossed. Paper sells settle locally; live
/// sells are submitted to the CLOB in the background. Triggered guards (and
/// guards whose position is gone) are cleared from the store.
fn tick_guards(
    shared: &Shared,
    account: &Arc<Mutex<PaperAccount>>,
    books: &HashMap<String, BookView>,
    marks: &HashMap<String, Decimal>,
    live: bool,
    peaks: &mut HashMap<String, Decimal>,
) {
    let book = guard::load().unwrap_or_default();
    if book.guards.is_empty() {
        return;
    }
    let now = Utc::now();
    for g in &book.guards {
        let (free, avg) = {
            let acct = account.lock().unwrap();
            match acct.positions.get(&g.token_id) {
                Some(p) => (
                    (p.size - acct.reserved_shares(&g.token_id)).max(Decimal::ZERO),
                    p.avg_price,
                ),
                None => (Decimal::ZERO, Decimal::ZERO),
            }
        };
        let mid = marks.get(&g.token_id).copied();
        let best_bid = books.get(&g.token_id).and_then(|b| b.best_bid);

        match guard::evaluate(g, free, avg, mid, best_bid, peaks) {
            GuardAction::Hold => {}
            GuardAction::Drop => {
                let _ = guard::clear(&g.token_id);
            }
            GuardAction::Sell { shares, reason } => {
                let bids = books
                    .get(&g.token_id)
                    .map(|b| b.bids.clone())
                    .unwrap_or_default();
                if live {
                    let order = crate::trade::LiveOrder::Market {
                        token_id: g.token_id.clone(),
                        side: TradeSide::Sell,
                        amount: shares,
                    };
                    let shared = Arc::clone(shared);
                    let label = reason.to_string();
                    tokio::spawn(async move {
                        let msg = match live::place(order).await {
                            Ok(s) => format!("{label} exit: {s}"),
                            Err(e) => format!("{label} exit FAILED: {e}"),
                        };
                        shared.lock().unwrap().notices.push(msg);
                    });
                } else {
                    let result = {
                        let mut acct = account.lock().unwrap();
                        paper_engine::market_sell(&mut acct, &g.token_id, &bids, shares, now)
                    };
                    match result {
                        Ok(t) => {
                            let _ = crate::paper::store::save(&account.lock().unwrap());
                            shared.lock().unwrap().notices.push(format!(
                                "{reason} exit: sold {} @ {} (pnl {})",
                                t.size.round_dp(2),
                                t.price.round_dp(4),
                                t.realized_pnl.unwrap_or_default().round_dp(2)
                            ));
                        }
                        Err(e) => {
                            shared
                                .lock()
                                .unwrap()
                                .notices
                                .push(format!("{reason} exit rejected: {e}"));
                        }
                    }
                }
                let _ = guard::clear(&g.token_id);
            }
        }
    }
}

/// Overwrite the account snapshot with real wallet state (live mode only).
/// Open orders and trade history are left untouched — in live mode those are
/// managed through the CLOB directly (`clob orders` / `clob cancel`).
/// Flatten a Gamma market into the row the UI needs, skipping ones without
/// CLOB tokens (not tradable).
fn to_market_row(m: gamma::types::response::Market) -> Option<MarketRow> {
    let token_ids: Vec<String> = m
        .clob_token_ids
        .as_ref()?
        .iter()
        .map(|id| id.to_string())
        .collect();
    if token_ids.is_empty() {
        return None;
    }
    Some(MarketRow {
        id: m.id,
        question: m.question.unwrap_or_else(|| "(untitled)".to_string()),
        token_ids,
        outcomes: m.outcomes.unwrap_or_default(),
        prices: m.outcome_prices.unwrap_or_default(),
        volume: m.volume_num,
        liquidity: m.liquidity_num,
        closed: m.closed,
        active: m.active,
        description: m.description,
        resolution_source: m.resolution_source,
        end_date: m.end_date,
    })
}

/// Whether a Gamma market has actually resolved (payouts fixed at 0/1), not
/// merely closed for trading at interim prices.
fn market_resolved(closed: Option<bool>, uma_status: Option<&str>, prices: &[Decimal]) -> bool {
    let finalized = closed == Some(true) || uma_status == Some("resolved");
    finalized
        && !prices.is_empty()
        && prices
            .iter()
            .all(|p| *p == Decimal::ZERO || *p == Decimal::ONE)
}

/// Look up the markets behind `token_ids` and report any that have resolved,
/// keyed by token with each token's payout (its final outcome price).
async fn fetch_resolutions(
    client: &gamma::Client,
    token_ids: &[String],
) -> anyhow::Result<HashMap<String, ResolutionInfo>> {
    let mut out = HashMap::new();
    let parsed: Vec<_> = token_ids
        .iter()
        .filter_map(|t| quotes::parse_token_id(t).ok())
        .collect();
    for chunk in parsed.chunks(20) {
        // Gamma omits closed markets by default — but a resolved market *is*
        // closed, so without this filter we'd never see the very markets we're
        // checking for resolution. `closed=true` returns the settled ones and
        // drops the still-open holdings (which aren't resolved anyway).
        let request = gamma::types::request::MarketsRequest::builder()
            .clob_token_ids(chunk.to_vec())
            .closed(true)
            .limit(chunk.len() as i32)
            .build();
        for m in client.markets(&request).await? {
            let prices = m.outcome_prices.unwrap_or_default();
            if !market_resolved(m.closed, m.uma_resolution_status.as_deref(), &prices) {
                continue;
            }
            let Some(tokens) = m.clob_token_ids else {
                continue;
            };
            let neg_risk = m.neg_risk.unwrap_or(false);
            let condition_id = m.condition_id.map(|b| b.to_string());
            for (i, token) in tokens.iter().enumerate() {
                let Some(payout) = prices.get(i).copied() else {
                    continue;
                };
                out.insert(
                    token.to_string(),
                    ResolutionInfo {
                        payout,
                        won: payout == Decimal::ONE,
                        condition_id: condition_id.clone(),
                        neg_risk,
                        outcome_index: i,
                        outcome_count: tokens.len(),
                    },
                );
            }
        }
    }
    Ok(out)
}

async fn fetch_markets(client: &gamma::Client) -> anyhow::Result<Vec<MarketRow>> {
    let request = gamma::types::request::MarketsRequest::builder()
        .closed(false)
        .limit(150)
        .build();
    let markets = client.markets(&request).await?;
    let mut rows: Vec<MarketRow> = markets.into_iter().filter_map(to_market_row).collect();
    rows.sort_by(|a, b| {
        b.volume
            .unwrap_or(Decimal::ZERO)
            .cmp(&a.volume.unwrap_or(Decimal::ZERO))
    });
    Ok(rows)
}

/// Search markets through the Gamma search API — the same endpoint the
/// `markets search` command uses, so the TUI finds every market, not just the
/// top-by-volume snapshot.
pub(crate) async fn search_markets(query: &str) -> Vec<MarketRow> {
    let client = gamma::Client::default();
    let request = gamma::types::request::SearchRequest::builder()
        .q(query.to_string())
        .limit_per_type(30)
        .build();
    let Ok(results) = client.search(&request).await else {
        return Vec::new();
    };
    results
        .events
        .unwrap_or_default()
        .into_iter()
        .flat_map(|e| e.markets.unwrap_or_default())
        .filter_map(to_market_row)
        .collect()
}

/// Kick off a background search; results land in `search_results` keyed by the
/// query so the UI shows them once they match the active query.
pub(crate) fn run_search(shared: Shared, query: String) {
    tokio::spawn(async move {
        let rows = search_markets(&query).await;
        let mut d = shared.lock().unwrap();
        d.search_results = rows;
        d.search_results_query = query;
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    #[test]
    fn resolved_needs_final_prices_and_a_close_signal() {
        let final_prices = [dec!(1), dec!(0)];
        assert!(market_resolved(Some(true), None, &final_prices));
        assert!(market_resolved(None, Some("resolved"), &final_prices));
        // Closed but trading prices not finalized — not resolved.
        assert!(!market_resolved(
            Some(true),
            None,
            &[dec!(0.97), dec!(0.03)]
        ));
        // Final-looking prices on an open market — not resolved.
        assert!(!market_resolved(Some(false), None, &final_prices));
        assert!(!market_resolved(None, None, &final_prices));
        assert!(!market_resolved(Some(true), None, &[]));
    }
}
