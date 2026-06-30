//! Live account glue for the TUI: derive the wallet address and read its real
//! balance + positions so the terminal's views show actual on-chain state when
//! running without `--paper`. Order placement itself lives in [`crate::trade`]
//! (shared with the copy-trade engine).

use anyhow::{Context, Result};
use polymarket_client_sdk_v2::auth::LocalSigner;
use polymarket_client_sdk_v2::clob::types::AssetType;
use polymarket_client_sdk_v2::clob::types::request::BalanceAllowanceRequest;
use polymarket_client_sdk_v2::data;
use polymarket_client_sdk_v2::data::types::request::PositionsRequest;
use polymarket_client_sdk_v2::types::{Address, Decimal};
use polymarket_client_sdk_v2::{POLYGON, derive_proxy_wallet};
use std::str::FromStr;

use crate::auth;
use crate::commands::COLLATERAL_DECIMALS;
use crate::config;
use crate::paper::types::Position;

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
