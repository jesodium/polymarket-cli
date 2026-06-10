//! Live account glue for the TUI: derive the wallet address and read its real
//! balance + positions so the terminal's views show actual on-chain state when
//! running without `--paper`. Order placement itself lives in [`crate::trade`]
//! (shared with the strategy engine).

use anyhow::{Context, Result};
use polymarket_client_sdk_v2::clob::types::AssetType;
use polymarket_client_sdk_v2::clob::types::request::BalanceAllowanceRequest;
use polymarket_client_sdk_v2::data;
use polymarket_client_sdk_v2::data::types::request::PositionsRequest;
use polymarket_client_sdk_v2::types::{Address, Decimal};

use crate::auth;
use crate::commands::COLLATERAL_DECIMALS;
use crate::paper::types::Position;

pub(crate) use crate::trade::LiveOrder;
pub(crate) use crate::trade::place;

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

/// A snapshot of real account state for hydrating the terminal's views.
pub(crate) struct LiveAccount {
    pub cash: Option<Decimal>,
    pub positions: Vec<Position>,
}

/// Read the wallet's real collateral balance and open positions.
///
/// `with_balance` is expensive (it authenticates), so the refresher only sets
/// it on the slower cadence; position reads are public and run every pass.
pub(crate) async fn fetch_account(user: Address, with_balance: bool) -> LiveAccount {
    let positions = fetch_positions(user).await.unwrap_or_default();
    let cash = if with_balance {
        fetch_collateral().await.ok()
    } else {
        None
    };
    LiveAccount { cash, positions }
}

async fn fetch_positions(user: Address) -> Result<Vec<Position>> {
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
        })
        .collect())
}

async fn fetch_collateral() -> Result<Decimal> {
    let client = auth::authenticated_clob_client(None, None).await?;
    let request = BalanceAllowanceRequest::builder()
        .asset_type(AssetType::Collateral)
        .build();
    let result = client.balance_allowance(request).await?;
    let divisor = Decimal::from(10u64.pow(COLLATERAL_DECIMALS));
    Ok(result.balance / divisor)
}
