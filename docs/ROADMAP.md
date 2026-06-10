# Roadmap

This project is evolving from a command-driven CLI into a **local-first
prediction-market trading terminal** — and, eventually, a platform for hosted
autonomous agents. The guiding principle is *local-first*: everything runs on
your machine, against your keys and your data, with no required backend. Hosted
features are additive, never mandatory.

---

## Phase 1 — Local Trading Terminal ✅ (current)

The foundation: trade, simulate, and automate entirely on your own machine.

- **Paper Trading** — a virtual account with realistic, order-book-driven fills,
  PnL, ROI, and performance stats. Fully isolated from real funds.
- **Limit Orders** — buy/sell limit orders with resting-order placement,
  marketable-fill detection, cancellation, open-order tracking, and fill
  history. Resting orders settle against live quotes. Works in paper today and
  shares the same execution surface as live trading.
- **Local Strategy Engine** — a plugin architecture for autonomous strategies
  that receive live market data, emit signals, and place orders. Strategies run
  independently of the UI, with start / stop / enable / disable / status / logs
  controls. Ships with `momentum` and `mean_reversion` examples.
- **Trading Terminal TUI** — a keyboard-driven, Bloomberg-style terminal that is
  the primary interface: dashboard, markets, market detail with live order
  books, portfolio, positions, orders, trade history, strategies, logs, and
  settings — all without typing commands.

## Phase 2 — Data & Backtesting

Turn the local terminal into a research workstation.

- **Historical Market Database** — local capture and storage of order books,
  trades, and price history (SQLite/Parquet) for offline analysis.
- **Backtesting Engine** — replay historical data through the *same* `Strategy`
  trait and paper engine used live, so a strategy backtests and trades with
  identical code.
- **Strategy Analytics** — Sharpe, drawdown, exposure, per-strategy attribution,
  and equity curves surfaced in the TUI.

## Phase 3 — Hosted Agents

Lift strategies off the laptop without giving up local control. See
[HOSTED_AGENTS.md](./HOSTED_AGENTS.md) for the recommended architecture.

- **Hosted Agents** — run strategies 24/7 on a remote worker.
- **Remote Strategy Execution** — the existing engine, containerized and managed.
- **CLI/TUI Control of Remote Agents** — the same TUI views drive remote agents
  over an authenticated API; local and remote agents appear side by side.
- **Multi-Device Synchronization** — roster, parameters, and logs sync across
  devices.

## Phase 4 — Collaboration & Marketplace

- **Team Accounts** — shared rosters and role-based controls.
- **Shared Strategies** — publish and subscribe to strategy configurations.
- **Strategy Marketplace** — discover, rate, and (optionally) monetize
  strategies.
- **Advanced Analytics** — cross-strategy, cross-market portfolio analytics.

---

## Design invariants

These hold across every phase:

1. **One execution surface.** Paper, backtest, and live all flow through the
   same `Signal` → execution path. A strategy never knows or cares which mode it
   is in. (See `src/strategy/engine.rs::ExecutionMode`.)
2. **Strategies are plugins.** Core code never hardcodes strategy logic; new
   strategies are added under `src/strategy/strategies/` and registered in one
   place (`registry.rs`).
3. **The UI only reads.** Rendering never blocks on the network — background
   tasks keep shared state fresh, so the terminal stays responsive while
   strategies trade.
4. **Local-first.** No hosted component is ever required to trade, simulate, or
   automate.
