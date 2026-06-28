# Changelog

All notable changes to this project are documented here. The format loosely
follows [Keep a Changelog](https://keepachangelog.com/); versions match the git
tags. The section matching a release tag is published as that release's notes
(see `.github/workflows/release.yml`).

## [Unreleased]

## [0.1.10] - 2026-06-28

### Added
- Persisted mark-to-market equity curve; dashboard now derives Sharpe ratio and
  max drawdown from it.
- Quant metrics block and market resolution rules in the detail view.
- Configurable copy-trade poll interval (`copy_poll_secs`) and a paper PnL
  summary.
- Shell completion generation and a `ctf convert` command.
- Dashboard win rate now breaks down as wins/losses (`75% (3W 1L)`).

### Fixed
- TUI no longer shows misleading $0.00 PnL / zeroed equity while quotes are
  still loading. Marks-dependent figures (positions uPnL/value, dashboard and
  portfolio equity/uPnL/ROI) render "loading…" until the first quote refresh
  completes.
- Order-book spread calculation.
- Market/event listings default to open; order book sorts best-price first.
- Allow overriding the funder/proxy address (#40).
- Paper store guards against stale writes.

## [0.1.9] - 2026-06-13

### Added
- **MCP server** (`polymarket mcp`): a JSON-RPC 2.0 server over stdio that
  exposes 37 tools for AI agents — market/event discovery, CLOB and on-chain
  data, wallet/account, order placement (paper or live), full paper trading,
  and a guarded `run_cli` escape hatch. Each tool re-invokes the CLI with
  `--output json`, so paper/live behaviour matches the CLI exactly.
- **MCP status panel** in the TUI Settings tab: shows whether an AI client is
  connected, the client name/version, tool-call count, and last activity.

### Fixed
- TUI marked held positions at the best bid instead of the bid-ask midpoint, so
  marks and unrealized PnL disagreed with the `paper portfolio` command. The
  TUI now marks at the midpoint to match.

## [0.1.8] - 2026-06-12

### Added
- Resolution detection, a redeemable positions section, and position history.

### Changed
- Cost-basis calculations and position history display.
