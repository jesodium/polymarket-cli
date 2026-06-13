use std::str::FromStr;

use anyhow::{Context, Result, bail};
use clap::{Args, Subcommand};
use polymarket_client_sdk_v2::auth::LocalSigner;
use polymarket_client_sdk_v2::auth::Signer as _;
use polymarket_client_sdk_v2::types::Address;
use polymarket_client_sdk_v2::{POLYGON, derive_proxy_wallet};

use crate::config;
use crate::output::OutputFormat;

#[derive(Args)]
pub struct WalletArgs {
    #[command(subcommand)]
    pub command: WalletCommand,
}

#[derive(Subcommand)]
pub enum WalletCommand {
    /// Generate a new random wallet and save to config
    Create {
        /// Overwrite existing wallet
        #[arg(long)]
        force: bool,
        /// Signature type: eoa, proxy (default), or gnosis-safe
        #[arg(long, default_value = "proxy")]
        signature_type: String,
    },
    /// Import an existing private key
    Import {
        /// Private key (hex, with or without 0x prefix)
        key: String,
        /// Overwrite existing wallet
        #[arg(long)]
        force: bool,
        /// Signature type: eoa, proxy (default), or gnosis-safe
        #[arg(long, default_value = "proxy")]
        signature_type: String,
    },
    /// Show the address of the configured wallet
    Address,
    /// Show wallet info (address, config path, key source)
    Show,
    /// Override the funder/proxy wallet address — for web/email (Magic)
    /// accounts whose server-assigned proxy differs from the derived one
    SetProxy {
        /// Proxy wallet address (0x...). Look it up with
        /// `polymarket profiles get <your-address>`.
        address: Option<String>,
        /// Clear the override and revert to the derived proxy
        #[arg(long)]
        clear: bool,
    },
    /// Delete all config and keys (fresh install)
    Reset {
        /// Skip confirmation prompt
        #[arg(long)]
        force: bool,
    },
}

pub fn execute(
    args: WalletArgs,
    output: OutputFormat,
    private_key_flag: Option<&str>,
) -> Result<()> {
    match args.command {
        WalletCommand::Create {
            force,
            signature_type,
        } => cmd_create(output, force, &signature_type),
        WalletCommand::Import {
            key,
            force,
            signature_type,
        } => cmd_import(&key, output, force, &signature_type),
        WalletCommand::Address => cmd_address(output, private_key_flag),
        WalletCommand::Show => cmd_show(output, private_key_flag),
        WalletCommand::SetProxy { address, clear } => cmd_set_proxy(output, address, clear),
        WalletCommand::Reset { force } => cmd_reset(output, force),
    }
}

fn guard_overwrite(force: bool) -> Result<()> {
    if !force && config::config_exists() {
        bail!(
            "A wallet already exists at {}. Use --force to overwrite.",
            config::config_path()?.display()
        );
    }
    Ok(())
}

fn cmd_create(output: OutputFormat, force: bool, signature_type: &str) -> Result<()> {
    guard_overwrite(force)?;

    let signer = LocalSigner::random().with_chain_id(Some(POLYGON));
    let address = signer.address();
    let key_hex = format!("{:#x}", signer.to_bytes());

    config::save_wallet(&key_hex, POLYGON, signature_type)?;
    let config_path = config::config_path()?;
    let proxy_addr = derive_proxy_wallet(address, POLYGON);

    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "address": address.to_string(),
                    "proxy_address": proxy_addr.map(|a| a.to_string()),
                    "signature_type": signature_type,
                    "config_path": config_path.display().to_string(),
                })
            );
        }
        OutputFormat::Table => {
            println!("Wallet created successfully!");
            println!("Address:        {address}");
            if let Some(proxy) = proxy_addr {
                println!("Proxy wallet:   {proxy}");
            }
            println!("Signature type: {signature_type}");
            println!("Config:         {}", config_path.display());
            println!();
            println!("IMPORTANT: Back up your private key from the config file.");
            println!("           If lost, your funds cannot be recovered.");
        }
    }
    Ok(())
}

fn cmd_import(key: &str, output: OutputFormat, force: bool, signature_type: &str) -> Result<()> {
    guard_overwrite(force)?;

    let signer = LocalSigner::from_str(key)
        .context("Invalid private key")?
        .with_chain_id(Some(POLYGON));
    let address = signer.address();
    let key_hex = format!("{:#x}", signer.to_bytes());

    config::save_wallet(&key_hex, POLYGON, signature_type)?;
    let config_path = config::config_path()?;
    let proxy_addr = derive_proxy_wallet(address, POLYGON);

    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "address": address.to_string(),
                    "proxy_address": proxy_addr.map(|a| a.to_string()),
                    "signature_type": signature_type,
                    "config_path": config_path.display().to_string(),
                })
            );
        }
        OutputFormat::Table => {
            println!("Wallet imported successfully!");
            println!("Address:        {address}");
            if let Some(proxy) = proxy_addr {
                println!("Proxy wallet:   {proxy}");
            }
            println!("Signature type: {signature_type}");
            println!("Config:         {}", config_path.display());
        }
    }
    Ok(())
}

fn cmd_address(output: OutputFormat, private_key_flag: Option<&str>) -> Result<()> {
    let (key, _) = config::resolve_key(private_key_flag)?;
    let key = key.ok_or_else(|| anyhow::anyhow!("{}", config::NO_WALLET_MSG))?;

    let signer = LocalSigner::from_str(&key).context("Invalid private key")?;
    let address = signer.address();

    match output {
        OutputFormat::Json => {
            println!("{}", serde_json::json!({"address": address.to_string()}));
        }
        OutputFormat::Table => {
            println!("{address}");
        }
    }
    Ok(())
}

fn cmd_show(output: OutputFormat, private_key_flag: Option<&str>) -> Result<()> {
    let (key, source) = config::resolve_key(private_key_flag)?;
    let signer = key.as_deref().and_then(|k| LocalSigner::from_str(k).ok());
    let address = signer.as_ref().map(|s| s.address().to_string());

    // Prefer a configured override; fall back to the CREATE2-derived proxy.
    let configured_proxy = config::resolve_proxy_address()?;
    let proxy_addr = match &configured_proxy {
        Some(p) => Some(p.clone()),
        None => signer
            .as_ref()
            .and_then(|s| derive_proxy_wallet(s.address(), POLYGON))
            .map(|a| a.to_string()),
    };
    let proxy_source = if configured_proxy.is_some() {
        "override"
    } else {
        "derived"
    };

    let sig_type = config::resolve_signature_type(None)?;
    let config_path = config::config_path()?;

    match output {
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "address": address,
                    "proxy_address": proxy_addr,
                    "proxy_source": proxy_source,
                    "signature_type": sig_type,
                    "config_path": config_path.display().to_string(),
                    "source": source.label(),
                    "configured": address.is_some(),
                })
            );
        }
        OutputFormat::Table => {
            match &address {
                Some(addr) => println!("Address:        {addr}"),
                None => println!("Address:        (not configured)"),
            }
            if let Some(proxy) = &proxy_addr {
                println!("Proxy wallet:   {proxy} ({proxy_source})");
            }
            println!("Signature type: {sig_type}");
            println!("Config path:    {}", config_path.display());
            println!("Key source:     {}", source.label());
        }
    }
    Ok(())
}

fn cmd_set_proxy(output: OutputFormat, address: Option<String>, clear: bool) -> Result<()> {
    if clear {
        config::set_proxy_address(None)?;
        match output {
            OutputFormat::Json => println!(
                "{}",
                serde_json::json!({"proxy_address": serde_json::Value::Null, "cleared": true})
            ),
            OutputFormat::Table => {
                println!("Proxy override cleared. Reverting to the derived proxy address.");
            }
        }
        return Ok(());
    }

    let address = address.ok_or_else(|| {
        anyhow::anyhow!("Provide a proxy address, or use --clear to remove the override")
    })?;
    // Parse to validate and normalize to a checksummed address.
    let checksummed = Address::from_str(address.trim())
        .context("Invalid proxy address")?
        .to_string();
    config::set_proxy_address(Some(&checksummed))?;

    match output {
        OutputFormat::Json => println!(
            "{}",
            serde_json::json!({"proxy_address": checksummed, "cleared": false})
        ),
        OutputFormat::Table => {
            println!("Proxy override set: {checksummed}");
            println!("Used as the funder for proxy/gnosis-safe trading and balance queries.");
        }
    }
    Ok(())
}

fn cmd_reset(output: OutputFormat, force: bool) -> Result<()> {
    if !config::config_exists() {
        match output {
            OutputFormat::Table => println!("Nothing to reset. No config found."),
            OutputFormat::Json => {
                println!(
                    "{}",
                    serde_json::json!({"reset": false, "reason": "no config found"})
                );
            }
        }
        return Ok(());
    }

    if !force {
        use std::io::{self, BufRead, Write};
        print!("This will delete all keys and config. Are you sure? [y/N] ");
        io::stdout().flush()?;
        let mut input = String::new();
        io::stdin().lock().read_line(&mut input)?;
        if !matches!(input.trim().to_lowercase().as_str(), "y" | "yes") {
            println!("Aborted.");
            return Ok(());
        }
    }

    let path = config::config_path()?;
    config::delete_config()?;

    match output {
        OutputFormat::Table => {
            println!("Config deleted: {}", path.display());
            println!("All keys and settings have been removed.");
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::json!({
                    "reset": true,
                    "deleted": path.display().to_string(),
                })
            );
        }
    }
    Ok(())
}
