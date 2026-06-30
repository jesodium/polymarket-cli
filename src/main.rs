mod auth;
mod commands;
mod config;
mod copytrade;
mod guard;
mod mcp;
mod output;
mod paper;
mod settings;
mod shell;
mod trade;
mod tui;
mod updater;

use std::process::ExitCode;

use clap::{CommandFactory, Parser, Subcommand};
use clap_complete::Shell;
use output::OutputFormat;

#[derive(Parser)]
#[command(
    name = "fiberglass",
    about = "Fiberglass — a trading terminal for Polymarket",
    version
)]
pub(crate) struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// Output format: table or json
    #[arg(short, long, global = true, default_value = "table")]
    pub(crate) output: OutputFormat,

    /// Private key (overrides env var and config file)
    #[arg(long, global = true)]
    private_key: Option<String>,

    /// Signature type: eoa, proxy, or gnosis-safe
    #[arg(long, global = true)]
    signature_type: Option<String>,
}

#[derive(Subcommand)]
enum Commands {
    /// Launch the interactive trading terminal (TUI) — the primary interface
    Tui {
        /// Trade against the paper account (simulated). Without this flag the
        /// terminal runs in LIVE mode against your real wallet and the CLOB.
        #[arg(long)]
        paper: bool,
    },
    /// Launch the line-based interactive shell
    Shell,
    /// Run as an MCP server over stdio (for AI agents / LLM tooling)
    Mcp,
    /// Copy-trading: follow wallets and mirror their trades
    Copytrade(commands::copytrade::CopyTradeArgs),
    /// View and edit trading settings (mode, presets, slippage, TP/SL)
    Settings(commands::settings::SettingsArgs),
    /// Interact with markets
    Markets(commands::markets::MarketsArgs),
    /// Interact with events
    Events(commands::events::EventsArgs),
    /// Interact with tags
    Tags(commands::tags::TagsArgs),
    /// Interact with series
    Series(commands::series::SeriesArgs),
    /// Interact with comments
    Comments(commands::comments::CommentsArgs),
    /// Look up public profiles
    Profiles(commands::profiles::ProfilesArgs),
    /// Sports metadata and teams
    Sports(commands::sports::SportsArgs),
    /// Check and set contract approvals for trading
    Approve(commands::approve::ApproveArgs),
    /// Interact with the CLOB (order book, trading, balances)
    Clob(commands::clob::ClobArgs),
    /// Paper trading: simulate orders with a virtual balance
    Paper(commands::paper::PaperArgs),
    /// CTF operations: split, merge, redeem positions
    Ctf(commands::ctf::CtfArgs),
    /// Query on-chain data (positions, trades, leaderboards)
    Data(commands::data::DataArgs),
    /// Bridge assets from other chains to Polymarket
    Bridge(commands::bridge::BridgeArgs),
    /// Manage wallet and authentication
    Wallet(commands::wallet::WalletArgs),
    /// Check API health status
    Status,
    /// Update to the latest version
    Upgrade,
    /// Generate a shell completion script (bash, zsh, fish, powershell, elvish)
    ///
    /// Example: `fiberglass completion zsh > ~/.zfunc/_fiberglass`
    Completion {
        /// Target shell
        shell: Shell,
    },
}

#[tokio::main]
async fn main() -> ExitCode {
    let cli = Cli::parse();
    let output = cli.output;
    let is_tui = matches!(
        cli.command,
        Commands::Tui { .. } | Commands::Shell | Commands::Mcp
    );
    // Skip the update check for `upgrade` (it does its own) and for
    // `completion` (its stdout must stay a clean, sourceable script).
    let skip_update = matches!(cli.command, Commands::Upgrade | Commands::Completion { .. });

    if !skip_update {
        updater::refresh_cache_if_stale();
    }

    if let Err(e) = run(cli).await {
        output::print_error(&e, output);
        return ExitCode::FAILURE;
    }

    if !is_tui
        && !skip_update
        && let Some(tag) = updater::check_update()
    {
        eprintln!("\nUpdate {tag} available — run `fiberglass upgrade` to install.");
    }

    ExitCode::SUCCESS
}

#[allow(clippy::too_many_lines)]
pub(crate) async fn run(cli: Cli) -> anyhow::Result<()> {
    // Lazy-init so we only pay for the client we actually use.
    let gamma = std::cell::LazyCell::new(polymarket_client_sdk_v2::gamma::Client::default);
    let data = std::cell::LazyCell::new(polymarket_client_sdk_v2::data::Client::default);
    let bridge = std::cell::LazyCell::new(polymarket_client_sdk_v2::bridge::Client::default);

    match cli.command {
        Commands::Tui { paper } => Box::pin(tui::run(paper)).await,
        Commands::Shell => Box::pin(shell::run_shell()).await,
        Commands::Mcp => mcp::run(),
        Commands::Copytrade(args) => commands::copytrade::execute(args, cli.output).await,
        Commands::Settings(args) => commands::settings::execute(args, cli.output),
        Commands::Markets(args) => commands::markets::execute(&gamma, args, cli.output).await,
        Commands::Events(args) => commands::events::execute(&gamma, args, cli.output).await,
        Commands::Tags(args) => commands::tags::execute(&gamma, args, cli.output).await,
        Commands::Series(args) => commands::series::execute(&gamma, args, cli.output).await,
        Commands::Comments(args) => commands::comments::execute(&gamma, args, cli.output).await,
        Commands::Profiles(args) => commands::profiles::execute(&gamma, args, cli.output).await,
        Commands::Sports(args) => commands::sports::execute(&gamma, args, cli.output).await,
        Commands::Approve(args) => {
            commands::approve::execute(
                args,
                cli.output,
                cli.private_key.as_deref(),
                cli.signature_type.as_deref(),
            )
            .await
        }
        Commands::Clob(args) => {
            commands::clob::execute(
                args,
                cli.output,
                cli.private_key.as_deref(),
                cli.signature_type.as_deref(),
            )
            .await
        }
        Commands::Ctf(args) => {
            commands::ctf::execute(
                args,
                cli.output,
                cli.private_key.as_deref(),
                cli.signature_type.as_deref(),
            )
            .await
        }
        Commands::Paper(args) => commands::paper::execute(args, cli.output).await,
        Commands::Data(args) => commands::data::execute(&data, args, cli.output).await,
        Commands::Bridge(args) => commands::bridge::execute(&bridge, args, cli.output).await,
        Commands::Wallet(args) => {
            commands::wallet::execute(args, cli.output, cli.private_key.as_deref())
        }
        Commands::Upgrade => commands::upgrade::execute(),
        Commands::Completion { shell } => {
            let mut cmd = Cli::command();
            let name = cmd.get_name().to_string();
            clap_complete::generate(shell, &mut cmd, name, &mut std::io::stdout());
            Ok(())
        }
        Commands::Status => {
            let status = gamma.status().await?;
            match cli.output {
                OutputFormat::Json => {
                    println!("{}", serde_json::json!({"status": status}));
                }
                OutputFormat::Table => {
                    println!("API Status: {status}");
                }
            }
            Ok(())
        }
    }
}
