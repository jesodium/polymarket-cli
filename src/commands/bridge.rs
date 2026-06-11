use anyhow::Result;
use clap::{Args, Subcommand};
use polymarket_client_sdk_v2::bridge::{
    self,
    types::{DepositRequest, StatusRequest},
};

use crate::output::OutputFormat;
use crate::output::bridge::{print_deposit, print_status, print_supported_assets};

#[derive(Args)]
pub struct BridgeArgs {
    #[command(subcommand)]
    pub command: BridgeCommand,
}

#[derive(Subcommand)]
pub enum BridgeCommand {
    /// Get deposit addresses for a wallet (EVM, Solana, Bitcoin)
    Deposit {
        /// Polymarket wallet address (0x...)
        address: polymarket_client_sdk_v2::types::Address,
    },

    /// List supported chains and tokens for deposits
    SupportedAssets,

    /// Check deposit transaction status for an address
    Status {
        /// Deposit address (EVM, Solana, or Bitcoin)
        address: String,
    },
}

pub async fn execute(
    client: &bridge::Client,
    args: BridgeArgs,
    output: OutputFormat,
) -> Result<()> {
    match args.command {
        BridgeCommand::Deposit { address } => {
            let request = DepositRequest::builder().address(address).build();

            let response = client.deposit(&request).await?;
            print_deposit(&response, &output)?;
        }

        BridgeCommand::SupportedAssets => {
            let response = client.supported_assets().await?;
            print_supported_assets(&response, &output)?;
        }

        BridgeCommand::Status { address } => {
            anyhow::ensure!(!address.trim().is_empty(), "Address cannot be empty");
            let request = StatusRequest::builder().address(&address).build();

            let response = client.status(&request).await?;
            print_status(&response, &output)?;
        }
    }

    Ok(())
}

/// TUI-friendly deposit address lookup — returns the EVM deposit address string.
pub(crate) async fn tui_deposit_address() -> Result<String> {
    let client = bridge::Client::default();
    let address = {
        let signer = crate::auth::resolve_signer(None)?;
        polymarket_client_sdk_v2::auth::Signer::address(&signer)
    };
    let request = DepositRequest::builder().address(address).build();
    let response = client.deposit(&request).await?;
    Ok(format!("Deposit USDC.e to: {} (EVM)", response.address.evm))
}

/// TUI-friendly deposit status check — returns a one-line summary.
pub(crate) async fn tui_deposit_status() -> Result<String> {
    let client = bridge::Client::default();
    let address = {
        let signer = crate::auth::resolve_signer(None)?;
        polymarket_client_sdk_v2::auth::Signer::address(&signer)
    };
    let request = StatusRequest::builder().address(&address.to_string()).build();
    let response = client.status(&request).await?;
    let pending: Vec<_> = response
        .transactions
        .iter()
        .filter(|t| !matches!(t.status, polymarket_client_sdk_v2::bridge::types::DepositTransactionStatus::Completed))
        .collect();
    if pending.is_empty() {
        Ok("No pending deposits.".into())
    } else {
        Ok(format!("{} pending deposit(s). Run `polymarket bridge status {}` for details.", pending.len(), address))
    }
}
