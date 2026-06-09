//! Live market data for the simulator, fetched through the same
//! unauthenticated CLOB / Gamma clients the rest of the CLI uses.

use std::collections::BTreeMap;
use std::str::FromStr;

use anyhow::{Context, Result};
use polymarket_client_sdk_v2::clob;
use polymarket_client_sdk_v2::clob::types::request::{MidpointRequest, OrderBookSummaryRequest};
use polymarket_client_sdk_v2::gamma;
use polymarket_client_sdk_v2::types::{Decimal, U256};

use super::types::{MarketMeta, Quote};

pub(crate) fn parse_token_id(s: &str) -> Result<U256> {
    U256::from_str(s).map_err(|_| anyhow::anyhow!("Invalid token ID: {s}"))
}

/// Order book sides as `(price, size)` levels, best price first.
pub(crate) struct BookLevels {
    pub bids: Vec<(Decimal, Decimal)>,
    pub asks: Vec<(Decimal, Decimal)>,
}

impl BookLevels {
    pub fn quote(&self) -> Quote {
        Quote {
            best_bid: self.bids.first().map(|&(p, _)| p),
            best_ask: self.asks.first().map(|&(p, _)| p),
        }
    }
}

/// Fetch the live order book for a token, sorted best-first on both sides.
pub(crate) async fn fetch_book(client: &clob::Client, token_id: U256) -> Result<BookLevels> {
    let request = OrderBookSummaryRequest::builder()
        .token_id(token_id)
        .build();
    let book = client
        .order_book(&request)
        .await
        .context("Failed to fetch order book")?;

    let mut bids: Vec<(Decimal, Decimal)> = book.bids.iter().map(|l| (l.price, l.size)).collect();
    let mut asks: Vec<(Decimal, Decimal)> = book.asks.iter().map(|l| (l.price, l.size)).collect();
    bids.sort_by_key(|&(price, _)| std::cmp::Reverse(price)); // highest bid first
    asks.sort_by_key(|&(price, _)| price); // lowest ask first
    Ok(BookLevels { bids, asks })
}

/// Fetch best bid/ask for a set of tokens, keyed by token ID string.
pub(crate) async fn fetch_quotes(
    client: &clob::Client,
    token_ids: &[String],
) -> Result<BTreeMap<String, Quote>> {
    let mut quotes = BTreeMap::new();
    if token_ids.is_empty() {
        return Ok(quotes);
    }
    let requests: Vec<_> = token_ids
        .iter()
        .map(|id| {
            Ok(OrderBookSummaryRequest::builder()
                .token_id(parse_token_id(id)?)
                .build())
        })
        .collect::<Result<_>>()?;
    let books = client
        .order_books(&requests)
        .await
        .context("Failed to fetch order books")?;
    for book in books {
        let best_bid = book.bids.iter().map(|l| l.price).max();
        let best_ask = book.asks.iter().map(|l| l.price).min();
        quotes.insert(book.asset_id.to_string(), Quote { best_bid, best_ask });
    }
    Ok(quotes)
}

/// Fetch midpoint marks for a set of tokens, keyed by token ID string.
/// Tokens without a midpoint (e.g. empty books) are skipped.
pub(crate) async fn fetch_marks(
    client: &clob::Client,
    token_ids: &[String],
) -> Result<BTreeMap<String, Decimal>> {
    let mut marks = BTreeMap::new();
    for id in token_ids {
        let request = MidpointRequest::builder()
            .token_id(parse_token_id(id)?)
            .build();
        if let Ok(result) = client.midpoint(&request).await {
            marks.insert(id.clone(), result.mid);
        }
    }
    Ok(marks)
}

/// Look up the market question and outcome name for a token via Gamma.
/// Falls back to placeholders if the market can't be resolved, so a metadata
/// hiccup never blocks a simulated trade.
pub(crate) async fn fetch_meta(client: &gamma::Client, token_id: U256) -> MarketMeta {
    let request = gamma::types::request::MarketsRequest::builder()
        .clob_token_ids(vec![token_id])
        .limit(1)
        .build();
    let market = match client.markets(&request).await {
        Ok(mut markets) if !markets.is_empty() => markets.remove(0),
        _ => {
            return MarketMeta {
                question: "(unknown market)".to_string(),
                outcome: "(unknown)".to_string(),
            };
        }
    };

    let outcome = market
        .clob_token_ids
        .as_deref()
        .and_then(|ids| ids.iter().position(|id| *id == token_id))
        .and_then(|i| market.outcomes.as_deref().and_then(|o| o.get(i)))
        .cloned()
        .unwrap_or_else(|| "(unknown)".to_string());

    MarketMeta {
        question: market
            .question
            .unwrap_or_else(|| "(unknown market)".to_string()),
        outcome,
    }
}

/// Fill any resting limit orders the market has crossed since the last
/// command, persisting if anything changed. Returns the fills.
pub(crate) async fn settle_resting_orders(
    account: &mut super::types::PaperAccount,
    client: &clob::Client,
) -> Result<Vec<super::types::Trade>> {
    if account.open_orders.is_empty() {
        return Ok(Vec::new());
    }
    let mut tokens: Vec<String> = account
        .open_orders
        .iter()
        .map(|o| o.token_id.clone())
        .collect();
    tokens.sort();
    tokens.dedup();
    let quotes = fetch_quotes(client, &tokens).await?;
    let fills = super::engine::settle_open_orders(account, &quotes, chrono::Utc::now());
    if !fills.is_empty() {
        super::store::save(account)?;
    }
    Ok(fills)
}
