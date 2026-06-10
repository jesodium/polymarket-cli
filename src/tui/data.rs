//! Shared, live market data for the TUI.
//!
//! A background task ([`refresher`]) periodically pulls the markets list and
//! the order books for whatever tokens the UI is currently watching, writing
//! everything into a single [`SharedData`] behind a mutex. The render loop
//! only ever reads this struct, so drawing never blocks on the network and the
//! interface stays responsive while strategies trade.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use polymarket_client_sdk_v2::gamma;
use polymarket_client_sdk_v2::types::{Address, Decimal};

use super::live;
use crate::paper::quotes;
use crate::paper::types::PaperAccount;

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
}

/// A token's live book, summarized.
#[derive(Clone, Debug, Default)]
pub(crate) struct BookView {
    pub best_bid: Option<Decimal>,
    pub best_ask: Option<Decimal>,
    pub bids: Vec<(Decimal, Decimal)>,
    pub asks: Vec<(Decimal, Decimal)>,
}

#[derive(Default)]
pub(crate) struct SharedData {
    pub markets: Vec<MarketRow>,
    pub markets_status: String,
    pub books: HashMap<String, BookView>,
    pub marks: HashMap<String, Decimal>,
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
    loop {
        // Live: mirror the real wallet into the account snapshot the views read.
        if let Some(user) = live_user {
            let live_acct = live::fetch_account(user, market_ticks == 0).await;
            hydrate(&account, live_acct);
            // Open orders need an authenticated call — slow cadence only.
            if market_ticks == 0
                && let Ok(orders) = live::fetch_open_orders().await
            {
                shared.lock().unwrap().live_orders = orders;
            }
        }

        // Refresh the markets list roughly every ~30s (every 6th 5s pass).
        if market_ticks == 0 {
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
        }
        market_ticks = (market_ticks + 1) % 6;

        // Refresh books for watched tokens every pass.
        let watch = shared.lock().unwrap().watch.clone();
        if !watch.is_empty()
            && let Ok(client) = crate::auth::unauthenticated_clob_client()
        {
            let mut books = HashMap::new();
            let mut marks = HashMap::new();
            for tid in &watch {
                if let Ok(token) = quotes::parse_token_id(tid)
                    && let Ok(levels) = quotes::fetch_book(&client, token).await
                {
                    let q = levels.quote();
                    let mid = match (q.best_bid, q.best_ask) {
                        (Some(b), Some(a)) => Some((b + a) / Decimal::from(2)),
                        (Some(b), None) => Some(b),
                        (None, Some(a)) => Some(a),
                        (None, None) => None,
                    };
                    if let Some(m) = mid {
                        marks.insert(tid.clone(), m);
                    }
                    books.insert(
                        tid.clone(),
                        BookView {
                            best_bid: q.best_bid,
                            best_ask: q.best_ask,
                            bids: levels.bids,
                            asks: levels.asks,
                        },
                    );
                }
            }
            let mut d = shared.lock().unwrap();
            for (k, v) in books {
                d.books.insert(k, v);
            }
            for (k, v) in marks {
                d.marks.insert(k, v);
            }
            d.last_refresh = Some(Utc::now());
        }

        tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    }
}

/// Overwrite the account snapshot with real wallet state (live mode only).
/// Open orders and trade history are left untouched — in live mode those are
/// managed through the CLOB directly (`clob orders` / `clob cancel`).
fn hydrate(account: &Arc<Mutex<PaperAccount>>, live: live::LiveAccount) {
    let mut a = account.lock().unwrap();
    a.positions.clear();
    for p in live.positions {
        a.positions.insert(p.token_id.clone(), p);
    }
    if let Some(cash) = live.cash {
        a.cash = cash;
        // Anchor ROI to first observed equity (cash + cost basis).
        if a.initial_balance == Decimal::ZERO {
            let cost: Decimal = a.positions.values().map(|p| p.size * p.avg_price).sum();
            a.initial_balance = cash + cost;
        }
    }
}

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
    })
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
