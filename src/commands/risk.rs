//! `tp` / `sl` / `trail` / `risk` — user-facing risk rule commands.
//!
//! These are thin CLI wrappers over the shared guard engine (`crate::guard`).

use anyhow::{Result, bail};
use clap::{Args, Subcommand};
use polymarket_client_sdk_v2::types::Decimal;

use crate::guard;
use crate::output::OutputFormat;

#[derive(Args)]
pub struct TpArgs {
    #[command(subcommand)]
    pub command: RiskRuleCommand,
}

#[derive(Args)]
pub struct SlArgs {
    #[command(subcommand)]
    pub command: RiskRuleCommand,
}

#[derive(Args)]
pub struct TrailArgs {
    #[command(subcommand)]
    pub command: RiskRuleCommand,
}

#[derive(Subcommand)]
pub enum RiskRuleCommand {
    /// Add or replace a take-profit/stop-loss/trailing-stop rule for a token
    Add {
        /// CLOB token ID of the position to watch
        token_id: String,
        /// Rule threshold as a positive percent
        #[arg(long)]
        pct: Decimal,
        /// Guard the live wallet position (default: paper account)
        #[arg(long)]
        live: bool,
    },
    /// Remove this rule type from a token
    Remove {
        /// CLOB token ID
        token_id: String,
    },
}

#[derive(Args)]
pub struct RiskArgs {
    #[command(subcommand)]
    pub command: RiskCommand,
}

#[derive(Subcommand)]
pub enum RiskCommand {
    /// List armed risk rules
    List,
    /// Remove all risk rules for a token
    Remove {
        /// CLOB token ID
        token_id: String,
    },
    /// Show worker liveness and armed risk rule count
    Status,
    /// Show recent notification events (exits, failures)
    Events {
        /// Max events to show (most recent)
        #[arg(long, default_value = "20")]
        limit: usize,
    },
    /// Start the worker automatically at login (macOS): on | off | show
    Autostart { state: String },
}

pub async fn execute_tp(args: TpArgs, output: OutputFormat) -> Result<()> {
    match args.command {
        RiskRuleCommand::Add {
            token_id,
            pct,
            live,
        } => add_rule(token_id, pct, live, RuleKind::TakeProfit),
        RiskRuleCommand::Remove { token_id } => remove_rule(token_id, RuleKind::TakeProfit, output),
    }
}

pub async fn execute_sl(args: SlArgs, output: OutputFormat) -> Result<()> {
    match args.command {
        RiskRuleCommand::Add {
            token_id,
            pct,
            live,
        } => add_rule(token_id, pct, live, RuleKind::StopLoss),
        RiskRuleCommand::Remove { token_id } => remove_rule(token_id, RuleKind::StopLoss, output),
    }
}

pub async fn execute_trail(args: TrailArgs, output: OutputFormat) -> Result<()> {
    match args.command {
        RiskRuleCommand::Add {
            token_id,
            pct,
            live,
        } => add_rule(token_id, pct, live, RuleKind::TrailingStop),
        RiskRuleCommand::Remove { token_id } => {
            remove_rule(token_id, RuleKind::TrailingStop, output)
        }
    }
}

pub async fn execute_risk(args: RiskArgs, output: OutputFormat) -> Result<()> {
    match args.command {
        RiskCommand::List => list(output),
        RiskCommand::Remove { token_id } => {
            let token_id = parse_token_id(&token_id)?;
            guard::clear(&token_id)?;
            println!("Removed all risk rules for {token_id}.");
            Ok(())
        }
        RiskCommand::Status => crate::commands::guard::print_status(output),
        RiskCommand::Events { limit } => crate::commands::guard::print_events(limit, output),
        RiskCommand::Autostart { state } => match state.as_str() {
            "on" => {
                crate::commands::guard::autostart_on()?;
                println!("Risk worker will start at login.");
                Ok(())
            }
            "off" => {
                crate::commands::guard::autostart_off()?;
                println!("Autostart disabled.");
                Ok(())
            }
            "show" | "status" => {
                println!(
                    "Autostart at login: {}",
                    if crate::commands::guard::autostart_enabled() {
                        "on"
                    } else {
                        "off"
                    }
                );
                Ok(())
            }
            other => bail!("Expected on | off | show, got '{other}'"),
        },
    }
}

#[derive(Clone, Copy)]
enum RuleKind {
    TakeProfit,
    StopLoss,
    TrailingStop,
}

fn add_rule(token_id: String, pct: Decimal, live: bool, kind: RuleKind) -> Result<()> {
    if pct <= Decimal::ZERO {
        bail!("--pct must be > 0");
    }
    let token_id = parse_token_id(&token_id)?;
    let existing = guard::load()
        .unwrap_or_default()
        .guards
        .into_iter()
        .find(|g| g.token_id == token_id);
    let (mut tp, mut sl, mut trail, mut mode_live) = existing
        .map(|g| {
            (
                g.take_profit_pct,
                g.stop_loss_pct,
                g.trailing_stop_pct,
                g.live,
            )
        })
        .unwrap_or((None, None, None, false));
    if live {
        mode_live = true;
    }
    match kind {
        RuleKind::TakeProfit => tp = Some(pct),
        RuleKind::StopLoss => sl = Some(pct),
        RuleKind::TrailingStop => trail = Some(pct),
    }
    guard::arm(&token_id, mode_live, tp, sl, trail)?;
    let mode = if mode_live { "live" } else { "paper" };
    let label = match kind {
        RuleKind::TakeProfit => "TP",
        RuleKind::StopLoss => "SL",
        RuleKind::TrailingStop => "trailing stop",
    };
    println!(
        "{label} armed on {token_id} ({mode}) at {}%.",
        pct.normalize()
    );
    crate::commands::guard::ensure_worker(false);
    Ok(())
}

fn remove_rule(token_id: String, kind: RuleKind, output: OutputFormat) -> Result<()> {
    let token_id = parse_token_id(&token_id)?;
    let existing = guard::load()
        .unwrap_or_default()
        .guards
        .into_iter()
        .find(|g| g.token_id == token_id);
    let Some(g) = existing else {
        println!("No risk rules set for {token_id}.");
        return Ok(());
    };
    let (mut tp, mut sl, mut trail) = (g.take_profit_pct, g.stop_loss_pct, g.trailing_stop_pct);
    match kind {
        RuleKind::TakeProfit => tp = None,
        RuleKind::StopLoss => sl = None,
        RuleKind::TrailingStop => trail = None,
    }
    guard::arm(&token_id, g.live, tp, sl, trail)?;
    let label = match kind {
        RuleKind::TakeProfit => "TP",
        RuleKind::StopLoss => "SL",
        RuleKind::TrailingStop => "trailing stop",
    };
    let updated = guard::load()
        .unwrap_or_default()
        .guards
        .into_iter()
        .find(|x| x.token_id == token_id);
    match output {
        OutputFormat::Json => {
            crate::output::print_json(&serde_json::json!({
                "token_id": token_id,
                "removed": label,
                "guard": updated,
            }))?;
        }
        OutputFormat::Table => match updated {
            Some(next) => println!(
                "{label} removed from {}. Remaining: {}.",
                token_id,
                next.describe()
            ),
            None => println!("{label} removed from {token_id}. No rules remain."),
        },
    }
    Ok(())
}

fn list(output: OutputFormat) -> Result<()> {
    let book = guard::load().unwrap_or_default();
    if let OutputFormat::Json = output {
        crate::output::print_json(&book)?;
        return Ok(());
    }
    if book.guards.is_empty() {
        println!("No risk rules armed.");
        return Ok(());
    }
    for g in &book.guards {
        let mode = if g.live { "live " } else { "paper" };
        println!("{mode}  {}  {}", g.token_id, g.describe());
    }
    Ok(())
}

fn parse_token_id(token_id: &str) -> Result<String> {
    Ok(crate::paper::quotes::parse_token_id(token_id)?.to_string())
}
