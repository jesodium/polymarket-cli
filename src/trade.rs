//! Live order placement — one entry point used by both the TUI order modal
//! and the copy-trade engine.
//!
//! This is exactly the path the `clob create-order` / `clob market-order`
//! commands use: resolve the signer, authenticate, build, sign, and
//! `post_orders`. Re-authenticating per call keeps the non-`Send` authenticated
//! client out of long-lived async state.

use anyhow::Result;
use polymarket_client_sdk_v2::clob::types::{Amount, OrderType, Side};
use polymarket_client_sdk_v2::types::Decimal;

use crate::auth;
use crate::paper::quotes::parse_token_id;
use crate::paper::types::TradeSide;

/// A real order to submit to the CLOB.
pub(crate) enum LiveOrder {
    Market {
        token_id: String,
        side: TradeSide,
        /// pUSD for buys, shares for sells.
        amount: Decimal,
    },
    Limit {
        token_id: String,
        side: TradeSide,
        price: Decimal,
        size: Decimal,
    },
}

fn sdk_side(side: TradeSide) -> Side {
    match side {
        TradeSide::Buy => Side::Buy,
        TradeSide::Sell => Side::Sell,
    }
}

/// Submit a real signed order to the CLOB. Returns a short status string.
pub(crate) async fn place(order: LiveOrder) -> Result<String> {
    let signer = auth::resolve_signer(None)?;
    let client = auth::authenticate_with_signer(&signer, None).await?;

    let result = match order {
        LiveOrder::Limit {
            token_id,
            side,
            price,
            size,
        } => {
            let built = client
                .limit_order()
                .token_id(parse_token_id(&token_id)?)
                .side(sdk_side(side))
                .price(price)
                .size(size)
                .order_type(OrderType::GTC)
                .build()
                .await?;
            let signed = client.sign(&signer, built).await?;
            client.post_orders(vec![signed]).await?
        }
        LiveOrder::Market {
            token_id,
            side,
            amount,
        } => {
            let parsed = if matches!(side, TradeSide::Sell) {
                Amount::shares(amount)?
            } else {
                Amount::usdc(amount)?
            };
            let built = client
                .market_order()
                .token_id(parse_token_id(&token_id)?)
                .side(sdk_side(side))
                .amount(parsed)
                .order_type(OrderType::FOK)
                .build()
                .await?;
            let signed = client.sign(&signer, built).await?;
            client.post_orders(vec![signed]).await?
        }
    };

    let first = result
        .into_iter()
        .next()
        .ok_or_else(|| anyhow::anyhow!("Order submission returned no result"))?;
    Ok(format!("Live order submitted: {first:?}"))
}
