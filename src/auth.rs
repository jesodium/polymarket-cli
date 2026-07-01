use std::str::FromStr;

use alloy::providers::ProviderBuilder;
use anyhow::{Context, Result};
use polymarket_client_sdk_v2::auth::state::Authenticated;
use polymarket_client_sdk_v2::auth::{LocalSigner, Normal, Signer as _};
use polymarket_client_sdk_v2::clob::types::SignatureType;
use polymarket_client_sdk_v2::types::Address;
use polymarket_client_sdk_v2::{POLYGON, clob};

use crate::config;

const DEFAULT_CLOB_HOST: &str = "https://clob.polymarket.com";
const DEFAULT_RPC_URL: &str = "https://polygon.drpc.org";

fn clob_host() -> String {
    std::env::var("POLYMARKET_CLOB_HOST").unwrap_or_else(|_| DEFAULT_CLOB_HOST.to_string())
}

fn rpc_url() -> String {
    std::env::var("POLYMARKET_RPC_URL").unwrap_or_else(|_| DEFAULT_RPC_URL.to_string())
}

fn parse_signature_type(s: &str) -> SignatureType {
    match s {
        config::DEFAULT_SIGNATURE_TYPE => SignatureType::Proxy,
        "gnosis-safe" => SignatureType::GnosisSafe,
        _ => SignatureType::Eoa,
    }
}

/// Resolve the configured wallet's own address (EOA from the private key).
pub fn my_address() -> Result<Address> {
    Ok(resolve_signer(None)?.address())
}

/// clap value parser: a `0x…` address, or `@` / `me` / `self` for the configured wallet.
pub fn parse_address_or_me(s: &str) -> Result<Address, String> {
    match s.trim() {
        "@" | "me" | "self" => my_address().map_err(|e| e.to_string()),
        other => Address::from_str(other).map_err(|e| format!("invalid address: {e}")),
    }
}

pub fn resolve_signer(
    private_key: Option<&str>,
) -> Result<impl polymarket_client_sdk_v2::auth::Signer> {
    let (key, _) = config::resolve_key(private_key)?;
    let key = key.ok_or_else(|| anyhow::anyhow!("{}", config::NO_WALLET_MSG))?;
    LocalSigner::from_str(&key)
        .context("Invalid private key")
        .map(|s| s.with_chain_id(Some(POLYGON)))
}

pub async fn authenticated_clob_client(
    private_key: Option<&str>,
    signature_type_flag: Option<&str>,
) -> Result<clob::Client<Authenticated<Normal>>> {
    let signer = resolve_signer(private_key)?;
    authenticate_with_signer(&signer, signature_type_flag).await
}

pub async fn authenticate_with_signer(
    signer: &(impl polymarket_client_sdk_v2::auth::Signer + Sync),
    signature_type_flag: Option<&str>,
) -> Result<clob::Client<Authenticated<Normal>>> {
    let sig_type = parse_signature_type(&config::resolve_signature_type(signature_type_flag)?);

    let mut builder = unauthenticated_clob_client()?
        .authentication_builder(signer)
        .signature_type(sig_type);

    // Honor a manual funder/proxy override (issue #40). Accounts registered via
    // the Polymarket web UI get a server-assigned proxy that differs from the
    // CREATE2-derived address, so the SDK's auto-derivation would target the
    // wrong wallet. Only proxy/gnosis sig types accept a funder; EOA must not.
    if matches!(sig_type, SignatureType::Proxy | SignatureType::GnosisSafe)
        && let Some(proxy) = config::resolve_proxy_address()?
    {
        let funder = Address::from_str(proxy.trim()).context(
            "Invalid proxy address override (POLYMARKET_PROXY_ADDRESS or config.json proxy_address)",
        )?;
        builder = builder.funder(funder);
    }

    builder
        .authenticate()
        .await
        .context("Failed to authenticate with Polymarket CLOB")
}

pub fn unauthenticated_clob_client() -> Result<clob::Client> {
    clob::Client::new(&clob_host(), clob::Config::default())
        .context("Failed to create Polymarket CLOB client")
}

pub async fn create_readonly_provider() -> Result<impl alloy::providers::Provider + Clone> {
    ProviderBuilder::new()
        .connect(&rpc_url())
        .await
        .context("Failed to connect to Polygon RPC")
}

pub async fn create_provider(
    private_key: Option<&str>,
) -> Result<impl alloy::providers::Provider + Clone> {
    let (key, _) = config::resolve_key(private_key)?;
    let key = key.ok_or_else(|| anyhow::anyhow!("{}", config::NO_WALLET_MSG))?;
    let signer = LocalSigner::from_str(&key)
        .context("Invalid private key")?
        .with_chain_id(Some(POLYGON));
    ProviderBuilder::new()
        .wallet(signer)
        .connect(&rpc_url())
        .await
        .context("Failed to connect to Polygon RPC with wallet")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_address_or_me_literal_and_garbage() {
        let addr = "0x0000000000000000000000000000000000000001";
        assert_eq!(parse_address_or_me(addr).unwrap().to_string(), addr);
        assert!(parse_address_or_me("not-an-address").is_err());
        // IMPORTANT NOTE: `@`/`me` needs a configured wallet — covered manually, not here.
    }

    #[test]
    fn parse_signature_type_proxy() {
        assert_eq!(parse_signature_type("proxy"), SignatureType::Proxy);
    }

    #[test]
    fn parse_signature_type_gnosis_safe() {
        assert_eq!(
            parse_signature_type("gnosis-safe"),
            SignatureType::GnosisSafe
        );
    }

    #[test]
    fn parse_signature_type_eoa() {
        assert_eq!(parse_signature_type("eoa"), SignatureType::Eoa);
    }

    #[test]
    fn parse_signature_type_unknown_defaults_to_eoa() {
        assert_eq!(parse_signature_type("unknown"), SignatureType::Eoa);
    }
}
