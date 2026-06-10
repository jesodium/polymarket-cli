//! `polymarket settings` — view and edit the PolyGun-style trading settings
//! (trading mode, confirmation threshold, quickbuy/quicksell presets,
//! slippage, default take-profit / stop-loss). The TUI Settings tab edits the
//! same file; this is the headless equivalent.

use anyhow::Result;
use clap::{Args, Subcommand};
use polymarket_client_sdk_v2::types::Decimal;

use crate::output::OutputFormat;
use crate::settings::{self, Settings, TradingMode};

#[derive(Args)]
pub struct SettingsArgs {
    #[command(subcommand)]
    pub command: Option<SettingsCommand>,
}

#[derive(Subcommand)]
pub enum SettingsCommand {
    /// Show all current settings (default)
    Show,
    /// Set the trading mode: cautious | standard | expert
    Mode { mode: String },
    /// Set the Standard-mode confirmation threshold (pUSD)
    Threshold { usd: Decimal },
    /// Set quickbuy preset amounts, e.g. `10,25,50,100`
    Quickbuy { amounts: String },
    /// Set quicksell preset percentages, e.g. `25,50,100`
    Quicksell { percents: String },
    /// Set the market-order slippage tolerance (percent)
    Slippage { pct: Decimal },
    /// Set the default take-profit percent for new positions (omit to clear)
    TakeProfit { pct: Option<Decimal> },
    /// Set the default stop-loss percent for new positions (omit to clear)
    StopLoss { pct: Option<Decimal> },
    /// Set the default trailing-stop percent for new positions (omit to clear)
    Trailing { pct: Option<Decimal> },
}

pub fn execute(args: SettingsArgs, output: OutputFormat) -> Result<()> {
    let mut s = settings::load();
    let cmd = args.command.unwrap_or(SettingsCommand::Show);

    let changed = match cmd {
        SettingsCommand::Show => false,
        SettingsCommand::Mode { mode } => {
            s.trading_mode = mode.parse::<TradingMode>()?;
            true
        }
        SettingsCommand::Threshold { usd } => {
            s.confirm_threshold_usd = usd;
            true
        }
        SettingsCommand::Quickbuy { amounts } => {
            s.quickbuy_presets = settings::parse_number_list(&amounts)?;
            true
        }
        SettingsCommand::Quicksell { percents } => {
            s.quicksell_presets = settings::parse_number_list(&percents)?;
            true
        }
        SettingsCommand::Slippage { pct } => {
            s.slippage_pct = pct;
            true
        }
        SettingsCommand::TakeProfit { pct } => {
            s.default_take_profit_pct = pct.filter(|p| *p > Decimal::ZERO);
            true
        }
        SettingsCommand::StopLoss { pct } => {
            s.default_stop_loss_pct = pct.filter(|p| *p > Decimal::ZERO);
            true
        }
        SettingsCommand::Trailing { pct } => {
            s.default_trailing_stop_pct = pct.filter(|p| *p > Decimal::ZERO);
            true
        }
    };

    if changed {
        settings::save(&s)?;
    }
    print_settings(&s, output, changed);
    Ok(())
}

fn print_settings(s: &Settings, output: OutputFormat, changed: bool) {
    match output {
        OutputFormat::Json => {
            let _ = crate::output::print_json(&serde_json::json!({
                "trading_mode": s.trading_mode.label(),
                "confirm_threshold_usd": s.confirm_threshold_usd,
                "quickbuy_presets": s.quickbuy_presets,
                "quicksell_presets": s.quicksell_presets,
                "slippage_pct": s.slippage_pct,
                "default_take_profit_pct": s.default_take_profit_pct,
                "default_stop_loss_pct": s.default_stop_loss_pct,
                "default_trailing_stop_pct": s.default_trailing_stop_pct,
            }));
        }
        OutputFormat::Table => {
            if changed {
                println!("Settings updated.\n");
            }
            let opt = |v: Option<Decimal>| v.map(|p| format!("{p}%")).unwrap_or_else(|| "off".into());
            println!("Trading mode        {} — {}", s.trading_mode, s.trading_mode.describe());
            println!("Confirm threshold   ${}", s.confirm_threshold_usd.normalize());
            println!("Quickbuy presets    {}", settings::fmt_money_list(&s.quickbuy_presets));
            println!("Quicksell presets   {}", settings::fmt_pct_list(&s.quicksell_presets));
            println!("Slippage tolerance  {}%", s.slippage_pct.normalize());
            println!("Default take-profit {}", opt(s.default_take_profit_pct));
            println!("Default stop-loss   {}", opt(s.default_stop_loss_pct));
            println!("Default trailing    {}", opt(s.default_trailing_stop_pct));
            if s.has_default_exit() {
                println!("\nNew buys from the TUI auto-arm a tp_sl exit guard with these defaults.");
            }
            if let Ok(p) = settings::config_path() {
                println!("\nFile: {}", p.display());
            }
        }
    }
}
