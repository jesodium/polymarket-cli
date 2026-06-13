# Polymarket CLI & Trading Terminal

> A local-first prediction-market **trading terminal** for Polymarket — browse markets, place market & limit orders, run autonomous strategies, and manage positions from a keyboard-driven TUI (or as a JSON API for scripts and agents).

<p align="center">
  <a href="https://github.com/jesodium/polymarket-cli/actions/workflows/ci.yml"><img alt="CI" src="https://github.com/jesodium/polymarket-cli/actions/workflows/ci.yml/badge.svg"></a>
  <img alt="Rust" src="https://img.shields.io/badge/rust-1.88%2B-orange?logo=rust&logoColor=white">
  <img alt="TUI" src="https://img.shields.io/badge/TUI-ratatui-00b3b3?logo=gnometerminal&logoColor=white">
  <img alt="Platform" src="https://img.shields.io/badge/platform-macOS%20%7C%20Linux-lightgrey">
  <img alt="License" src="https://img.shields.io/badge/license-MIT-yellow.svg">
  <img alt="Status: WIP" src="https://img.shields.io/badge/status-heavily%20WIP-red">
  <img alt="Paper trading" src="https://img.shields.io/badge/paper%20trading-yes-brightgreen">
</p>

> [!CAUTION]
> **❗ THIS IS HEAVILY WORK IN PROGRESS, DO NOT PUT REAL MONEY UNLESS YOU ARE WILLING TO LOSE IT ❗**
>
> APIs, commands, on-chain interactions, and live order routing are experimental and **not battle-tested**. Live mode signs and submits **real orders with real funds**, and autonomous strategies can trade on their own with no built-in risk caps yet. Start with `--paper`. Verify every transaction. You are solely responsible for any losses.

---

### ✨ Feature status

| Area | Status | Notes |
| --- | :---: | --- |
| 📈 Market & event browsing | ✅ | Gamma + CLOB data, JSON output |
| 🧾 Market & limit orders | ✅ | Buy/sell, open orders, cancel, history |
| 🧪 Paper trading | ✅ | $10k virtual account, realistic book-driven fills |
| 🖥️ Trading terminal (TUI) | ✅ | 10 views, live order book, modal order entry |
| 🤖 Local strategy engine | ✅ | Plugins: `momentum`, `mean_reversion`, `tp_sl`, live logs |
| 🎯 Take-profit / stop-loss | ✅ | Per-position exit guard: TP %, SL %, trailing stop |
| ⚙️ Trading settings | ✅ | Cautious/Standard/Expert modes, quickbuy/quicksell presets |
| 🔌 MCP server | ✅ | `polymarket mcp` — 37 tools for AI agents, paper + live |
| 💸 Live trading | 🚧 | Real CLOB orders wired — **untested with real funds** |
| 🛡️ Risk caps / kill-switch | ⏳ | Planned before autonomous live is safe |
| ☁️ Hosted agents | ⏳ | See [docs/ROADMAP.md](docs/ROADMAP.md) |

## Install

### Homebrew (macOS / Linux)

```bash
brew tap jesodium/polymarket-cli https://github.com/jesodium/polymarket-cli
brew install polymarket
```

### Shell script

```bash
curl -sSL https://raw.githubusercontent.com/jesodium/polymarket-cli/main/install.sh | sh
```

### Build from source

```bash
git clone https://github.com/jesodium/polymarket-cli
cd polymarket-cli
cargo install --path .
```

## Trading Terminal (TUI)

The primary interface is a keyboard-driven trading terminal — closer to a
Bloomberg Terminal than a traditional CLI.

```bash
polymarket tui            # LIVE mode — real wallet + CLOB (needs a wallet)
polymarket tui --paper    # PAPER mode — $10,000 simulated account, no wallet
```

The mode is shown in the top-left (red **⏺ LIVE** / green **◆ PAPER**) and
colors the whole frame. In live mode the terminal mirrors your real balance and
positions, and the order modal submits real signed orders to the CLOB; in paper
mode everything is simulated. Views (switch with `Tab` or `1`–`9`):

```
 1·Dashboard  2·Markets  3·Portfolio  4·Positions  5·Orders  6·History  7·Strategies  8·Logs  9·Settings
┌─ POLYMARKET TERMINAL   ● live   142 markets ─────────────────────────────────┐
├──────────────────────────────────────────────────────────────────────────────┤
│ Portfolio Value │ Cash Balance │ Daily PnL  │ Total PnL                        │
│   $10,240.00    │  $9,120.00   │  +$140.00  │  +$240.00                        │
├───────────────────────────┬──────────────────────────────────────────────────┤
│ Open Positions        3   │ Time      Side  Market               Size  Price  │
│ Open Orders           2   │ 14:02:11  BUY   Will BTC top $100k…  100   0.612  │
│ Running Strategies    1   │ 14:01:55  SELL  Fed cuts in March…    50   0.480  │
│ ROI                +2.4%  │ …                                                  │
└───────────────────────────┴──────────────────────────────────────────────────┘
 ↑↓ move · Enter open · / search · b buy · s sell · g attach strategy · q quit
```

- **Markets** — browse / search / sort, `Enter` to open a market. Paste a
  `polymarket.com` link into the search box to jump straight to that market.
- **Market detail** — live order book, place market **and** limit orders from a
  modal (`b`/`s`), no commands typed; `g` attaches an autonomous strategy.
  The buy ticket has take-profit / stop-loss fields and `p` cycles your
  quickbuy ($) / quicksell (%) presets.
- **Orders** — review open orders and cancel with `c`, in paper **and** live
  mode (live orders sync from the CLOB).
- **Strategies** — create a strategy in-terminal (`n` → pick plugin, enter
  tokens), then start / stop / enable / disable it and watch its signals,
  orders, and logs update live.
- **Settings** — edit trading settings in place (`Enter` to edit / cycle):
  trading mode (Cautious / Standard / Expert confirmation), confirmation
  threshold, quickbuy/quicksell presets, slippage, and default TP/SL/trailing
  levels. In live mode it also shows your wallet (EOA, proxy, balance) and can
  reveal the private key for export (`w`).

## Local Strategy Engine

Run autonomous strategies locally. They receive live market data, generate
signals, and place orders against the paper (or, in future, live) account —
independently of the UI.

```bash
polymarket strategy list                              # available plugins + roster
polymarket strategy add momentum --tokens <TOKEN_ID>  # configure an instance
polymarket strategy run                               # run the engine (Ctrl-C to stop)
polymarket strategy status                            # roster + runtime stats
polymarket strategy logs                              # tail engine log
```

Strategies are plugins under `src/strategy/strategies/` (ships with
`momentum`, `mean_reversion`, and `tp_sl` — a take-profit / stop-loss /
trailing-stop exit guard). See [docs/ROADMAP.md](docs/ROADMAP.md) and
[docs/HOSTED_AGENTS.md](docs/HOSTED_AGENTS.md).

### Take-profit / stop-loss

Attach an exit guard to any position — it market-sells when the mark hits your
profit target, loss limit, or trails off its peak:

```bash
polymarket strategy add tp_sl --tokens <TOKEN_ID>   # +30% TP / -20% SL defaults
polymarket strategy run
```

Or set defaults once and every buy from the TUI auto-arms a guard:

```bash
polymarket settings take-profit 40    # +40% take profit
polymarket settings stop-loss 25      # -25% stop loss
polymarket settings trailing 10       # 10% off peak (optional)
```

## Trading Settings

Execution settings, shared by the TUI and CLI:

```bash
polymarket settings                   # show all
polymarket settings mode expert       # cautious | standard | expert
polymarket settings threshold 250     # Standard-mode confirm threshold ($)
polymarket settings quickbuy 10,25,50,100
polymarket settings quicksell 25,50,100
polymarket settings slippage 2
```

- **Cautious** — every order asks for confirmation.
- **Standard** (default) — only orders at/above the threshold confirm.
- **Expert** — instant execution, no confirmation.

## Quick Start

```bash
# No wallet needed — browse markets immediately
polymarket markets list --limit 5
polymarket markets search "election"
polymarket events list --tag politics

# Check a specific market
polymarket markets get will-trump-win-the-2024-election

# JSON output for scripts
polymarket -o json markets list --limit 3
```

To trade, set up a wallet:

```bash
polymarket setup
# Or manually:
polymarket wallet create
polymarket approve set
```

## Configuration

### Wallet Setup

The CLI needs a private key to sign orders and on-chain transactions. Three ways to provide it (checked in this order):

1. **CLI flag**: `--private-key 0xabc...`
2. **Environment variable**: `POLYMARKET_PRIVATE_KEY=0xabc...`
3. **Config file**: `~/.config/polymarket/config.json`

```bash
# Create a new wallet (generates random key, saves to config)
polymarket wallet create

# Import an existing key
polymarket wallet import 0xabc123...

# Check what's configured
polymarket wallet show
```

The config file (`~/.config/polymarket/config.json`):

```json
{
  "private_key": "0x...",
  "chain_id": 137,
  "signature_type": "proxy"
}
```

### Signature Types

- `proxy` (default) — uses Polymarket's proxy wallet system
- `eoa` — signs directly with your key
- `gnosis-safe` — for multisig wallets

Override per-command with `--signature-type eoa` or via `POLYMARKET_SIGNATURE_TYPE`.

### What Needs a Wallet

Most commands work without a wallet — browsing markets, viewing order books, checking prices. You only need a wallet for:

- Placing and canceling orders (`clob create-order`, `clob market-order`, `clob cancel-*`)
- Checking your balances and trades (`clob balance`, `clob trades`, `clob orders`)
- On-chain operations (`approve set`, `ctf split/merge/redeem`)
- Reward and API key management (`clob rewards`, `clob create-api-key`)

Paper trading (`polymarket paper ...`) needs no wallet at all — see
[Paper Trading](#paper-trading-simulated-no-wallet-needed).

## Output Formats

Every command supports `--output table` (default) and `--output json`.

```bash
# Human-readable table (default)
polymarket markets list --limit 2
```

```
 Question                            Price (Yes)  Volume   Liquidity  Status
 Will Trump win the 2024 election?   52.00¢       $145.2M  $1.2M      Active
 Will BTC hit $100k by Dec 2024?     67.30¢       $89.4M   $430.5K    Active
```

```bash
# Machine-readable JSON
polymarket -o json markets list --limit 2
```

```json
[
  { "id": "12345", "question": "Will Trump win the 2024 election?", "outcomePrices": ["0.52", "0.48"], ... },
  { "id": "67890", "question": "Will BTC hit $100k by Dec 2024?", ... }
]
```

Short form: `-o json` or `-o table`.

Errors follow the same pattern — table mode prints `Error: ...` to stderr, JSON mode prints `{"error": "..."}` to stdout. Non-zero exit code either way.

## Commands

### Markets

```bash
# List markets with filters
polymarket markets list --limit 10
polymarket markets list --active true --order volume_num
polymarket markets list --closed false --limit 50 --offset 25

# Get a single market by ID or slug
polymarket markets get 12345
polymarket markets get will-trump-win

# Search
polymarket markets search "bitcoin" --limit 5

# Get tags for a market
polymarket markets tags 12345
```

**Flags for `markets list`**: `--limit`, `--offset`, `--order`, `--ascending`, `--active`, `--closed`

### Events

Events group related markets (e.g. "2024 Election" contains multiple yes/no markets).

```bash
polymarket events list --limit 10
polymarket events list --tag politics --active true
polymarket events get 500
polymarket events tags 500
```

**Flags for `events list`**: `--limit`, `--offset`, `--order`, `--ascending`, `--active`, `--closed`, `--tag`

### Tags, Series, Comments, Profiles, Sports

```bash
# Tags
polymarket tags list
polymarket tags get politics
polymarket tags related politics
polymarket tags related-tags politics

# Series (recurring events)
polymarket series list --limit 10
polymarket series get 42

# Comments on an entity
polymarket comments list --entity-type event --entity-id 500
polymarket comments get abc123
polymarket comments by-user 0xf5E6...

# Public profiles
polymarket profiles get 0xf5E6...

# Sports metadata
polymarket sports list
polymarket sports market-types
polymarket sports teams --league NFL --limit 32
```

### Order Book & Prices (CLOB)

All read-only — no wallet needed.

```bash
# Check API health
polymarket clob ok

# Prices
polymarket clob price 48331043336612883... --side buy
polymarket clob midpoint 48331043336612883...
polymarket clob spread 48331043336612883...

# Batch queries (comma-separated token IDs)
polymarket clob batch-prices "TOKEN1,TOKEN2" --side buy
polymarket clob midpoints "TOKEN1,TOKEN2"
polymarket clob spreads "TOKEN1,TOKEN2"

# Order book
polymarket clob book 48331043336612883...
polymarket clob books "TOKEN1,TOKEN2"

# Last trade
polymarket clob last-trade 48331043336612883...

# Market info
polymarket clob market 0xABC123...  # by condition ID
polymarket clob markets             # list all

# Price history
polymarket clob price-history 48331043336612883... --interval 1d --fidelity 30

# Metadata
polymarket clob tick-size 48331043336612883...
polymarket clob fee-rate 48331043336612883...
polymarket clob neg-risk 48331043336612883...
polymarket clob time
polymarket clob geoblock
```

**Interval options for `price-history`**: `1m`, `1h`, `6h`, `1d`, `1w`, `max`

### Trading (CLOB, authenticated)

Requires a configured wallet.

```bash
# Place a limit order (buy 10 shares at $0.50)
polymarket clob create-order \
  --token 48331043336612883... \
  --side buy --price 0.50 --size 10

# Place a market order (buy $5 worth)
polymarket clob market-order \
  --token 48331043336612883... \
  --side buy --amount 5

# Post multiple orders at once
polymarket clob post-orders \
  --tokens "TOKEN1,TOKEN2" \
  --side buy \
  --prices "0.40,0.60" \
  --sizes "10,10"

# Cancel
polymarket clob cancel ORDER_ID
polymarket clob cancel-orders "ORDER1,ORDER2"
polymarket clob cancel-market --market 0xCONDITION...
polymarket clob cancel-all

# View your orders and trades
polymarket clob orders
polymarket clob orders --market 0xCONDITION...
polymarket clob order ORDER_ID
polymarket clob trades

# Check balances
polymarket clob balance --asset-type collateral
polymarket clob balance --asset-type conditional --token 48331043336612883...
polymarket clob update-balance --asset-type collateral
```

**Order types**: `GTC` (default), `FOK`, `GTD`, `FAK`. Add `--post-only` for limit orders.

### Paper Trading (simulated, no wallet needed)

Practice trading with a virtual balance against live Polymarket prices. Paper
trading is fully isolated from your wallet and the live exchange — no keys,
no signing, no real funds.

```bash
# Turn on paper mode (creates a $10,000 virtual account the first time)
polymarket paper enable

# Simulated market buy: spend $100 at the current best asks
polymarket paper buy 48331043336612883... --amount 100

# Simulated limit buy: rests until the market crosses your price
polymarket paper buy 48331043336612883... --price 0.45 --size 50

# Simulated sells: market by default, limit with --price
polymarket paper sell 48331043336612883... --size 50
polymarket paper sell 48331043336612883... --size 50 --price 0.80

# Portfolio: cash, positions, realized/unrealized PnL, ROI
polymarket paper portfolio

# Resting limit orders (filled automatically when the market crosses)
polymarket paper orders
polymarket paper cancel ORDER_ID

# Trade log and performance analytics
polymarket paper history
polymarket paper stats          # win rate, best/worst trade, daily PnL, equity curve

# Manage the account
polymarket paper status
polymarket paper reset --balance 25000   # start over with a custom balance
polymarket paper disable                 # back to live trading (data is kept)
```

While paper mode is enabled, `clob create-order` and `clob market-order`
route to the simulator automatically (a `[paper]` notice is printed). You can
also force a single simulated order without toggling the mode:

```bash
polymarket clob market-order --token 48331043336612883... --side buy --amount 5 --paper
polymarket clob create-order --token 48331043336612883... --side buy --price 0.50 --size 10 --paper
```

**How fills are simulated**

- Market orders walk the live order book level by level, so large orders pay
  realistic slippage. If the book can't absorb the full size, the order is
  rejected (fill-or-kill).
- Limit orders that cross the market fill immediately at the touch (with
  price improvement if your limit is better). Otherwise they rest, reserving
  cash (buys) or shares (sells), and fill at your limit price once any paper
  command observes the market crossing it.
- Positions track average entry price; realized PnL is computed per sell
  against that average.

Paper data persists in `~/.config/polymarket/paper_account.json` (override
with `POLYMARKET_PAPER_FILE`). Wallet commands never touch it.

### Rewards & API Keys (CLOB, authenticated)

```bash
polymarket clob rewards --date 2024-06-15
polymarket clob earnings --date 2024-06-15
polymarket clob earnings-markets --date 2024-06-15
polymarket clob reward-percentages
polymarket clob current-rewards
polymarket clob market-reward 0xCONDITION...

# Check if orders are scoring rewards
polymarket clob order-scoring ORDER_ID
polymarket clob orders-scoring "ORDER1,ORDER2"

# API key management
polymarket clob api-keys
polymarket clob create-api-key
polymarket clob delete-api-key

# Account status
polymarket clob account-status
polymarket clob notifications
polymarket clob delete-notifications "NOTIF1,NOTIF2"
```

### On-Chain Data

Public data — no wallet needed.

```bash
# Portfolio
polymarket data positions 0xWALLET_ADDRESS
polymarket data closed-positions 0xWALLET_ADDRESS
polymarket data value 0xWALLET_ADDRESS
polymarket data traded 0xWALLET_ADDRESS

# Trade history
polymarket data trades 0xWALLET_ADDRESS --limit 50

# Activity
polymarket data activity 0xWALLET_ADDRESS

# Market data
polymarket data holders 0xCONDITION_ID
polymarket data open-interest 0xCONDITION_ID
polymarket data volume 12345  # event ID

# Leaderboards
polymarket data leaderboard --period month --order-by pnl --limit 10
polymarket data builder-leaderboard --period week
polymarket data builder-volume --period month
```

### Contract Approvals

Before trading, Polymarket contracts need ERC-20 (pUSD) and ERC-1155 (CTF token) approvals.

```bash
# Check current approvals (read-only)
polymarket approve check
polymarket approve check 0xSOME_ADDRESS

# Approve all contracts (sends on-chain transactions, needs MATIC for gas)
polymarket approve set
```

### CTF Operations

Split, merge, and redeem conditional tokens directly on-chain.

```bash
# Split $10 pUSD into YES/NO tokens
polymarket ctf split --condition 0xCONDITION... --amount 10

# Merge tokens back to pUSD
polymarket ctf merge --condition 0xCONDITION... --amount 10

# Redeem winning tokens after resolution
polymarket ctf redeem --condition 0xCONDITION...

# Redeem neg-risk positions
polymarket ctf redeem-neg-risk --condition 0xCONDITION... --amounts "10,5"

# Calculate IDs (read-only, no wallet needed)
polymarket ctf condition-id --oracle 0xORACLE... --question 0xQUESTION... --outcomes 2
polymarket ctf collection-id --condition 0xCONDITION... --index-set 1
polymarket ctf position-id --collection 0xCOLLECTION...
```

`--amount` is in pUSD (e.g., `10` = $10). The `--partition` flag defaults to binary (`1,2`). On-chain operations require MATIC for gas on Polygon.

### Bridge

Deposit assets from other chains into Polymarket.

```bash
# Get deposit addresses (EVM, Solana, Bitcoin)
polymarket bridge deposit 0xWALLET_ADDRESS

# List supported chains and tokens
polymarket bridge supported-assets

# Check deposit status
polymarket bridge status 0xDEPOSIT_ADDRESS
```

### Wallet Management

```bash
polymarket wallet create               # Generate new random wallet
polymarket wallet create --force       # Overwrite existing
polymarket wallet import 0xKEY...      # Import existing key
polymarket wallet address              # Print wallet address
polymarket wallet show                 # Full wallet info (address, source, config path)
polymarket wallet reset                # Delete config (prompts for confirmation)
polymarket wallet reset --force        # Delete without confirmation
```

### Interactive Shell

```bash
polymarket shell
# polymarket> markets list --limit 3
# polymarket> clob book 48331043336612883...
# polymarket> exit
```

Supports command history. All commands work the same as the CLI, just without the `polymarket` prefix.

### MCP Server (AI agents)

`polymarket mcp` runs a [Model Context Protocol](https://modelcontextprotocol.io)
server over stdio, exposing the CLI's capabilities as tools an AI agent can call.
Register it with any MCP client:

```json
{
  "mcpServers": {
    "polymarket": { "command": "polymarket", "args": ["mcp"] }
  }
}
```

For Claude Code: `claude mcp add polymarket -- polymarket mcp`.

It exposes 37 tools — market/event discovery, CLOB and on-chain data,
wallet/account, order placement, and full paper trading — plus a guarded
`run_cli` escape hatch for any other subcommand. Each tool re-invokes the CLI
with `--output json`, so behaviour matches the CLI exactly:

- **Paper vs live** is honoured automatically. With paper mode on (`polymarket
  paper enable`) the order tools simulate fills; otherwise they sign and submit
  to the live CLOB using your configured wallet. Order tools also accept a
  per-call `paper` argument.
- **Live trading moves real funds** — the same caveat as the CLI applies.

The TUI **Settings** tab shows a live MCP panel: whether a client is connected,
its name, tool-call count, and last activity.

### Other

```bash
polymarket status     # API health check
polymarket setup      # Guided first-time setup wizard
polymarket upgrade    # Update to the latest version
polymarket --version
polymarket --help
```

## Common Workflows

### Browse and research markets

```bash
polymarket markets search "bitcoin" --limit 5
polymarket markets get bitcoin-above-100k
polymarket clob book 48331043336612883...
polymarket clob price-history 48331043336612883... --interval 1d
```

### Set up a new wallet and start trading

```bash
polymarket wallet create
polymarket approve set                    # needs MATIC for gas
polymarket clob balance --asset-type collateral
polymarket clob market-order --token TOKEN_ID --side buy --amount 5
```

### Monitor your portfolio

```bash
polymarket data positions 0xYOUR_ADDRESS
polymarket data value 0xYOUR_ADDRESS
polymarket clob orders
polymarket clob trades
```

### Place and manage limit orders

```bash
# Place order
polymarket clob create-order --token TOKEN_ID --side buy --price 0.45 --size 20

# Check it
polymarket clob orders

# Cancel if needed
polymarket clob cancel ORDER_ID

# Or cancel everything
polymarket clob cancel-all
```

### Script with JSON output

```bash
# Pipe market data to jq
polymarket -o json markets list --limit 100 | jq '.[].question'

# Check prices programmatically
polymarket -o json clob midpoint TOKEN_ID | jq '.mid'

# Error handling in scripts
if ! result=$(polymarket -o json clob balance --asset-type collateral 2>/dev/null); then
  echo "Failed to fetch balance"
fi
```

## Architecture

```
src/
  main.rs        -- CLI entry point, clap parsing, error handling
  auth.rs        -- Wallet resolution, RPC provider, CLOB authentication
  config.rs      -- Config file (~/.config/polymarket/config.json)
  shell.rs       -- Interactive REPL
  mcp/           -- MCP stdio server (JSON-RPC) for AI agents
  commands/      -- One module per command group
  output/        -- Table and JSON rendering per command group
```

See [CHANGELOG.md](CHANGELOG.md) for release notes.

## License

MIT
