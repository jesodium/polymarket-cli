//! Live account glue for the TUI: derive the wallet address and read its real
//! balance + positions so the terminal's views show actual on-chain state when
//! running without `--paper`. Order placement itself lives in [`crate::trade`]
//! (shared with the copy-trade engine).

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use polymarket_client_sdk_v2::auth::LocalSigner;
use polymarket_client_sdk_v2::clob::types::AssetType;
use polymarket_client_sdk_v2::clob::types::request::BalanceAllowanceRequest;
use polymarket_client_sdk_v2::data;
use polymarket_client_sdk_v2::data::types::Side;
use polymarket_client_sdk_v2::data::types::request::{
    ClosedPositionsRequest, PositionsRequest, TradesRequest,
};
use polymarket_client_sdk_v2::gamma::{self, types::request::PublicProfileRequest};
use polymarket_client_sdk_v2::types::{Address, Decimal};
use polymarket_client_sdk_v2::{POLYGON, derive_proxy_wallet};
use std::str::FromStr;

use crate::auth;
use crate::commands::COLLATERAL_DECIMALS;
use crate::config;
use crate::paper::types::{OrderKind, Position, Trade, TradeSide};

pub(crate) use crate::trade::LiveOrder;
pub(crate) use crate::trade::place;

/// Read-only view of the configured wallet, for the Settings tab in live mode.
pub(crate) struct WalletInfo {
    /// Signer (EOA) address.
    pub eoa: String,
    /// Polymarket proxy wallet address, when the signer derives one.
    pub proxy: Option<String>,
    /// Address funds and orders actually flow through (proxy if set, else EOA).
    pub trading: String,
    pub signature_type: String,
    /// The raw private key, for the explicit "reveal/export" action.
    pub private_key: Option<String>,
    /// Where the wallet config lives on disk.
    pub config_path: String,
}

/// Gather wallet details from the resolved key + signature type. Returns `None`
/// when no wallet is configured.
pub(crate) fn wallet_info() -> Option<WalletInfo> {
    let (key, _) = config::resolve_key(None).ok()?;
    let key = key?;
    let signer = LocalSigner::from_str(&key).ok()?;
    let eoa = signer.address();
    let derived = derive_proxy_wallet(eoa, POLYGON).map(|a| a.to_string());
    let signature_type = config::resolve_signature_type(None).unwrap_or_else(|_| "proxy".into());
    // Proxy/gnosis sig types trade through a proxy; a manual override (set on
    // the Settings tab) wins over the CREATE2-derived address. EOA trades
    // directly from the signer.
    let is_proxy_sig = signature_type == "proxy" || signature_type == "gnosis-safe";
    let override_proxy = config::resolve_proxy_address().ok().flatten();
    let proxy = if is_proxy_sig {
        override_proxy.clone().or_else(|| derived.clone())
    } else {
        derived.clone()
    };
    let trading = match (is_proxy_sig, &proxy) {
        (true, Some(p)) => p.clone(),
        _ => eoa.to_string(),
    };
    Some(WalletInfo {
        eoa: eoa.to_string(),
        proxy,
        trading,
        signature_type,
        private_key: Some(key),
        config_path: config::config_path()
            .map(|p| p.display().to_string())
            .unwrap_or_default(),
    })
}

/// The wallet address whose state the terminal mirrors in live mode. Uses the
/// Polymarket proxy wallet when configured, otherwise the signer's EOA.
pub(crate) fn resolve_user_address() -> Result<Address> {
    if crate::commands::proxy::is_proxy_mode(None)? {
        crate::commands::proxy::derive_proxy_address(None)
    } else {
        let signer = auth::resolve_signer(None)?;
        Ok(polymarket_client_sdk_v2::auth::Signer::address(&signer))
    }
}

pub(crate) async fn fetch_positions(user: Address) -> Result<Vec<Position>> {
    let client = data::Client::default();
    let request = PositionsRequest::builder().user(user).build();
    let raw = client
        .positions(&request)
        .await
        .context("Failed to fetch positions")?;
    Ok(raw
        .into_iter()
        .map(|p| Position {
            token_id: p.asset.to_string(),
            question: p.title,
            outcome: p.outcome,
            size: p.size,
            avg_price: p.avg_price,
            realized_pnl: p.realized_pnl,
            entry_midpoint: Decimal::ZERO,
        })
        .collect())
}

/// Fetch the wallet's closed positions and synthesize one closing [`Trade`] per
/// market, so the dashboard's realized-PnL stats (win rate, avg win/loss, profit
/// factor) work in live mode — `account.trades` is otherwise never hydrated.
pub(crate) async fn fetch_closed_trades(user: Address) -> Result<Vec<Trade>> {
    let client = data::Client::default();
    // IMPORTANT NOTE: server caps this endpoint at 50 (rejects >50); add pagination if
    // a user ever closes more than 50 markets and needs the older ones.
    let request = ClosedPositionsRequest::builder()
        .user(user)
        .limit(50)?
        .build();
    let raw = client
        .closed_positions(&request)
        .await
        .context("Failed to fetch closed positions")?;
    let mut trades: Vec<Trade> = raw
        .into_iter()
        .enumerate()
        .map(|(i, p)| {
            // Entry cost = avg entry × size; exit notional = entry cost + realized,
            // so `notional − realized = entry cost` holds (cost-basis & history tab).
            let entry_cost = p.avg_price * p.total_bought;
            let exit_notional = entry_cost + p.realized_pnl;
            let exit_price = if p.total_bought > Decimal::ZERO {
                exit_notional / p.total_bought
            } else {
                p.cur_price
            };
            Trade {
                id: i as u64,
                timestamp: chrono::DateTime::from_timestamp(p.timestamp, 0).unwrap_or_default(),
                token_id: p.asset.to_string(),
                question: p.title,
                outcome: p.outcome,
                side: TradeSide::Sell,
                kind: OrderKind::Market,
                size: p.total_bought,
                price: exit_price,
                notional: exit_notional,
                realized_pnl: Some(p.realized_pnl),
            }
        })
        .collect();
    // Oldest first, matching the paper trade log (dashboard reverses for "recent").
    trades.sort_by_key(|t| t.timestamp);
    Ok(trades)
}

/// Fetch the wallet's actual fills (buys + sells) from the data `/trades`
/// endpoint — the full activity log shown on the Polymarket site, for the live
/// History tab. These carry no realized PnL (that's per-closed-position).
pub(crate) async fn fetch_fills(user: Address) -> Result<Vec<Trade>> {
    let client = data::Client::default();
    let request = TradesRequest::builder().user(user).limit(500)?.build();
    let raw = client
        .trades(&request)
        .await
        .context("Failed to fetch trade fills")?;
    let mut trades: Vec<Trade> = raw
        .into_iter()
        .enumerate()
        .map(|(i, t)| Trade {
            id: i as u64,
            timestamp: DateTime::from_timestamp(t.timestamp, 0).unwrap_or_default(),
            token_id: t.asset.to_string(),
            question: t.title,
            outcome: t.outcome,
            side: match t.side {
                Side::Sell => TradeSide::Sell,
                _ => TradeSide::Buy,
            },
            kind: OrderKind::Market,
            size: t.size,
            price: t.price,
            notional: t.size * t.price,
            realized_pnl: None,
        })
        .collect();
    trades.sort_by_key(|t| t.timestamp);
    Ok(trades)
}

/// Public Polymarket profile (username, bio, X, etc.) for the Settings → Debug
/// panel. Mirrors what `polymarket profiles get <addr>` returns.
#[derive(Clone, Default)]
pub(crate) struct LiveProfile {
    pub name: String,
    pub pseudonym: String,
    pub bio: String,
    pub x_username: String,
    pub verified: bool,
    pub created_at: Option<DateTime<Utc>>,
}

pub(crate) async fn fetch_profile(user: Address) -> Result<LiveProfile> {
    let client = gamma::Client::default();
    let req = PublicProfileRequest::builder().address(user).build();
    let p = client
        .public_profile(&req)
        .await
        .context("Failed to fetch profile")?;
    Ok(LiveProfile {
        name: p.name.unwrap_or_default(),
        pseudonym: p.pseudonym.unwrap_or_default(),
        bio: p.bio.unwrap_or_default(),
        x_username: p.x_username.unwrap_or_default(),
        verified: p.verified_badge.unwrap_or(false),
        created_at: p.created_at,
    })
}

/// A live open order at the CLOB, flattened for the Orders view.
#[derive(Clone, Debug)]
pub(crate) struct LiveOpenOrder {
    pub id: String,
    pub side: String,
    pub outcome: String,
    pub price: String,
    pub size: String,
    pub matched: String,
    pub created_at: String,
}

/// List the wallet's open orders at the CLOB (authenticates per call, same
/// pattern as [`crate::trade::place`]).
pub(crate) async fn fetch_open_orders() -> Result<Vec<LiveOpenOrder>> {
    use polymarket_client_sdk_v2::clob::types::request::OrdersRequest;
    let client = auth::authenticated_clob_client(None, None).await?;
    let request = OrdersRequest::builder().build();
    let page = client.orders(&request, None).await?;
    Ok(page
        .data
        .into_iter()
        .map(|o| LiveOpenOrder {
            id: o.id,
            side: o.side.to_string(),
            outcome: o.outcome,
            price: o.price.to_string(),
            size: o.original_size.to_string(),
            matched: o.size_matched.to_string(),
            created_at: o.created_at.format("%m-%d %H:%M").to_string(),
        })
        .collect())
}

/// Cancel one live order by ID. Returns a short status string.
pub(crate) async fn cancel_order(order_id: &str) -> Result<String> {
    let client = auth::authenticated_clob_client(None, None).await?;
    let result = client.cancel_order(order_id).await?;
    if result.not_canceled.is_empty() {
        Ok(format!(
            "Cancelled live order {}",
            &order_id[..order_id.len().min(12)]
        ))
    } else {
        let reasons: Vec<String> = result
            .not_canceled
            .iter()
            .map(|(id, why)| format!("{}: {why}", &id[..id.len().min(10)]))
            .collect();
        anyhow::bail!("Cancel failed — {}", reasons.join("; "))
    }
}

pub(crate) async fn fetch_collateral() -> Result<Decimal> {
    let client = auth::authenticated_clob_client(None, None).await?;
    let request = BalanceAllowanceRequest::builder()
        .asset_type(AssetType::Collateral)
        .build();
    let result = client.balance_allowance(request).await?;
    let divisor = Decimal::from(10u64.pow(COLLATERAL_DECIMALS));
    Ok(result.balance / divisor)
}
