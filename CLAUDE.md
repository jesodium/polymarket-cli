# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Commands

```bash
cargo build                      # debug build
cargo build --release            # release (thin LTO, stripped)
cargo test                       # run all tests
cargo clippy -- -D warnings      # lint (CI standard)
cargo fmt --check                # format check (CI standard)
cargo fmt                        # auto-format
cargo install --path .           # install binary locally
```

Source cargo env if binary not found: `source ~/.cargo/env`

## Architecture

**polymarket-cli** is a Rust trading terminal for Polymarket. Four entry points share the same core (all dispatched from `main.rs`):

1. **TUI** (`tui/`) — primary interface; 9 tabs, async render loop, background refresh
2. **CLI** (`commands/`) — 20+ subcommands
3. **Shell** (`shell.rs`) — line-based interactive REPL that parses input into the same CLI subcommands
4. **MCP** (`mcp/`) — JSON-RPC 2.0 server over stdio (`mcp` subcommand) for AI agents; each tool call re-invokes the binary as a subcommand with `--output json`, so paper/live behaviour matches the CLI exactly

### Module Map

| Path | Role |
|------|------|
| `src/tui/app.rs` | TUI state machine, 11 views, input handling |
| `src/tui/ui.rs` | ratatui rendering |
| `src/tui/data.rs` | background data sync loop (polls Gamma/CLOB every N ms) |
| `src/tui/live.rs` | wallet/balance polling |
| `src/paper/engine.rs` | paper order fills against live quotes |
| `src/paper/store.rs` | JSON persistence for paper account |
| `src/paper/quotes.rs` | live order-book/quote feed for the simulator (unauthenticated CLOB/Gamma) |
| `src/paper/types.rs` | paper account, positions, trades (fills) |
| `src/trade.rs` | live order placement — shared by the TUI order modal and copy-trade engine |
| `src/copytrade/engine.rs` | mirrors trades from followed wallets (polls every 15s) |
| `src/copytrade/config.rs` | followed-trader roster + per-trader sizing/filter rules (`copytrades.json`) |
| `src/guard.rs` | per-token TP/SL/trailing-stop evaluation |
| `src/auth.rs` | signer factory (private key → alloy signer + provider) |
| `src/config.rs` | wallet config: path, sig type (EOA/proxy/Gnosis), key storage |
| `src/settings.rs` | trading mode presets, quickbuy/quicksell, slippage |
| `src/shell.rs` | line-based interactive REPL (`Commands::Shell`) |
| `src/mcp/mod.rs` | MCP stdio server: JSON-RPC loop, subprocess dispatch (`Commands::Mcp`) |
| `src/mcp/tools.rs` | MCP tool registry (schemas) + tool→CLI-argv mapping |
| `src/mcp/status.rs` | MCP liveness file (`mcp-status.json`) read by the TUI Settings tab |
| `src/updater.rs` | self-update check against GitHub releases (`upgrade` command) |
| `src/output/` | table (`tabled`) or JSON formatters controlled by `--output` flag |

### Data Flow (TUI)

Keyboard → `App` state → shared state (Arc<Mutex<_>>) read by:
- render loop (ratatui output)
- background data loop (Gamma/CLOB API polling)
- guard evaluator (TP/SL checks each tick)
- copy-trade engine (followed-wallet checks every 15s)

### Paper vs Live

Paper mode (`--paper` flag or TUI toggle) uses `paper/` engine with a local JSON store. Orders are simulated against live quote feeds. No wallet signer is needed. Live mode requires a configured private key and approved USDC/CTF allowances.

### Key Deps

- `polymarket_client_sdk_v2` — Gamma, CLOB, Data, Bridge, CTF APIs
- `alloy` — Ethereum signing, RPC
- `ratatui` + `crossterm` — TUI
- `clap` — CLI parsing
- `rust_decimal` — price/size precision
