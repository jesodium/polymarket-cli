# changelog

all notable changes to this project are documented here. the format loosely
follows [keep a changelog](https://keepachangelog.com/); versions match the git
tags. the section matching a release tag is published as that release's notes
(see `.github/workflows/release.yml`).

entries are written all lowercase for readability (code spans, paths, and
version numbers keep their original case).

## [unreleased]

## [0.1.17] - 2026-07-01

### changed
- removed the user-facing `guard` command and replaced it with a simpler risk
  command flow:
  - set/remove take profit: `tp add/remove <token_id> --pct <n> [--live]`
  - set/remove stop loss: `sl add/remove <token_id> --pct <n> [--live]`
  - set/remove trailing stop: `trail add/remove <token_id> --pct <n> [--live]`
  - list/manage risk state: `risk list|remove|status|events|autostart`
- runtime control is now explicit at the top level: `start` runs the full
  background daemon (mcp + tp/sl/trail + copy trading), and `stop` halts it.

### removed
- mcp tools `guard_status` and `guard_events`. the underlying `risk
  status`/`risk events` commands still exist and remain reachable via the
  `run_cli` escape hatch.

## [0.1.16] - 2026-07-01

### added
- `guard` command — the TP/SL exit worker is now a first-class cli surface, not
  just a tui background task. arm a guard on a token you hold (`guard arm
  <token> --tp <pct> --sl <pct> --trail <pct>`, `--live` for the wallet
  position instead of paper), `guard clear`/`guard list` to manage them, and
  `guard run`/`guard start`/`guard stop` to control the evaluation worker
  (foreground or detached). `guard status` reports worker liveness and armed
  guard count; `guard autostart on|off|show` wires the worker to start at login
  on macos.
- top-level `stop` command (aliases `die`, `end`) to kill the background guard
  worker from anywhere.
- notification events. guard exits and failed exits are appended to
  `events.jsonl` in the config dir and popped as os notifications. `guard
  events [--limit n]` prints the recent log.
- mcp tools `guard_status` and `guard_events` so an agent can poll worker health
  and relay guard fills/alerts to external channels.

### added
- paper snapshots (`paper snapshot save/restore/list`). save named copies of
  the paper account and restore them later; snapshots live in
  `paper_snapshots/` next to the account file. a restore validates the json
  before overwriting the live account.
- paper csv export (`paper export trades|positions`). dumps the trade log or
  open positions as csv to stdout for redirect into a file.

### changed
- `wallet show` now notes that trading uses usdc.e, not native usdc.

## [0.1.14] - 2026-06-30

> note: the `v0.1.13` release build failed and was never published, so this
> release also carries the wallet, onboarding, and tui work that landed after
> the `v0.1.13` tag.

### added
- windows builds. release artifacts now include `x86_64-pc-windows-msvc`
  (`fiberglass-<tag>-x86_64-pc-windows-msvc.tar.gz`) alongside macos and linux,
  and ci runs the test suite on windows as well. the self-updater (`upgrade`)
  remains macos/linux-only; update on windows by downloading the release
  archive.
- private key is now stored in the os keychain (macos keychain / windows
  credential manager / linux secret service), with the plaintext config file
  as a fallback.
- import-only onboarding: log into your own polymarket account, with an
  overwrite confirmation and key validation on import.
- tui live-mode dashboard stats, history fills, and a debug panel.
- log-out button with a two-step, timed confirmation.

### changed
- active sidebar item now has a breathing fill and glowing edge bar.

### fixed
- accurate sort help text and a clearer empty-bridge status message.

## [0.1.13] - 2026-06-30

### changed
- renamed the binary from `polymarket` to `fiberglass`. the homebrew formula
  is now `fiberglass` (`brew install fiberglass`), release assets are
  `fiberglass-<tag>-<target>.tar.gz`, and all commands run as `fiberglass …`.
  config still lives in `~/.config/polymarket/` — existing wallets, settings,
  and paper accounts are untouched.
- rewrote the readme: shorter, lowercase, plain.

## [0.1.12] - 2026-06-30

### changed
- rebranded the project to **fiberglass** (crate, repo, mcp server name, and
  tui chrome). the installed binary, homebrew formula, and cli invocation are
  unchanged (`polymarket`).
- market list now requests the top 500 markets ordered server-side by 24h
  volume and caps each parent event to 2 markets, so a single multi-outcome
  event can no longer flood the list.
- dashboard pnl/roi/expectancy and avg win/loss are now color-tinted
  (green/red), and the win-rate line shows w/l counts in green/red.

## [0.1.11] - 2026-06-29

### added
- market-detail price-history view: stacked tug-of-war bars showing the
  focused outcome vs. the opposing side over time, with a selectable timeframe
  (`t` cycles 5m / 30m / 1h / 1d). `←→` switches which outcome is charted.
- tui wallet configuration on the settings tab (live mode): set a proxy/funder
  address override (`x`) and cycle the signature type (`y`, eoa → proxy →
  gnosis-safe). these fix the clob "maker address not allowed" error for
  accounts created on polymarket.com whose proxy differs from the derived one.

### changed
- wallet panel now shows the *effective* trading address, honoring a proxy
  override and the gnosis-safe signature type instead of only the derived
  proxy.

## [0.1.10] - 2026-06-28

### added
- persisted mark-to-market equity curve; dashboard now derives sharpe ratio and
  max drawdown from it.
- quant metrics block and market resolution rules in the detail view.
- configurable copy-trade poll interval (`copy_poll_secs`) and a paper pnl
  summary.
- shell completion generation and a `ctf convert` command.
- dashboard win rate now breaks down as wins/losses (`75% (3W 1L)`).

### fixed
- tui no longer shows misleading $0.00 pnl / zeroed equity while quotes are
  still loading. marks-dependent figures (positions upnl/value, dashboard and
  portfolio equity/upnl/roi) render "loading…" until the first quote refresh
  completes.
- order-book spread calculation.
- market/event listings default to open; order book sorts best-price first.
- allow overriding the funder/proxy address (#40).
- paper store guards against stale writes.

## [0.1.9] - 2026-06-13

### added
- **mcp server** (`polymarket mcp`): a json-rpc 2.0 server over stdio that
  exposes 37 tools for ai agents — market/event discovery, clob and on-chain
  data, wallet/account, order placement (paper or live), full paper trading,
  and a guarded `run_cli` escape hatch. each tool re-invokes the cli with
  `--output json`, so paper/live behaviour matches the cli exactly.
- **mcp status panel** in the tui settings tab: shows whether an ai client is
  connected, the client name/version, tool-call count, and last activity.

### fixed
- tui marked held positions at the best bid instead of the bid-ask midpoint, so
  marks and unrealized pnl disagreed with the `paper portfolio` command. the
  tui now marks at the midpoint to match.

## [0.1.8] - 2026-06-12

### added
- resolution detection, a redeemable positions section, and position history.

### changed
- cost-basis calculations and position history display.
