# Hosted Agents — Recommended Architecture

This document describes the path from the **local** strategy engine (Phase 1)
to **hosted, remotely controlled agents** (Phase 3). It is a design target, not
a shipped feature. The goal is to reuse the local engine almost verbatim so that
"hosted" is a deployment choice, not a rewrite.

## Guiding idea

The local engine already separates the three concerns that hosting needs:

| Concern            | Local component                              | Hosted equivalent                         |
| ------------------ | -------------------------------------------- | ----------------------------------------- |
| Strategy logic     | `Strategy` trait + `strategies/` plugins     | **unchanged** — same crate, same plugins  |
| Execution          | `engine::ExecutionMode` (Paper / Live)       | same, selected per-agent on the worker    |
| Control & state    | `StrategyEngine` methods + JSON config files | exposed over an authenticated API         |

So the worker is "the same binary, headless, plus an API." The TUI becomes a
client that can talk to a local engine *or* a remote one through the same
control surface.

## Components

```
            ┌──────────────────────────────────────────────┐
            │                  Control Plane               │
            │  - Auth (API keys / OAuth, per user)         │
            │  - Agent registry & roster storage           │
            │  - Routing: client ⇆ correct worker          │
            │  - Audit log of every order/control action   │
            └───────────────┬──────────────────────────────┘
                            │  authenticated API (REST + WS/SSE)
        ┌───────────────────┼───────────────────────────────┐
        │                   │                               │
 ┌──────▼───────┐    ┌──────▼───────┐                ┌───────▼────────┐
 │  TUI client  │    │  CLI client  │                │  Web client    │
 │ (local/remote│    │  (scriptable)│                │  (optional)    │
 │  toggle)     │    │              │                │                │
 └──────────────┘    └──────────────┘                └────────────────┘

        ┌───────────────────────────── Workers ───────────────────────────┐
        │  Each worker = the existing engine, headless:                    │
        │   - runs N agents (StrategyEngine instances)                     │
        │   - holds delegated/limited trading credentials                  │
        │   - streams logs, fills, status over WS/SSE                      │
        │   - persists state to durable storage, not local JSON            │
        └──────────────────────────────────────────────────────────────────┘
```

### 1. Control Plane (stateless API + datastore)

- **Auth**: per-user API keys or OAuth; scoped tokens for agents.
- **Roster store**: the `StrategyBook` (today a local `strategies.json`) becomes
  a per-user row in Postgres/Dynamo. The on-disk schema is already
  serde-serializable and versionable.
- **Routing**: maps `agent_id → worker` and proxies control calls.
- **Audit**: every order and control action is appended immutably.

### 2. Workers (the engine, headless)

- Run the **unmodified** `StrategyEngine::run_forever` loop per agent.
- Choose `ExecutionMode::Live` with credentials injected at runtime (never baked
  into the image). Recommended: a **delegated trading key** or
  Polymarket proxy-wallet with on-chain allowance caps, plus engine-level risk
  limits (max notional, max position, kill-switch).
- Replace local persistence (`store::save`, `config::save`,
  `append_log_file`) with injected trait objects:

  ```rust
  trait AccountStore  { fn load(&self)->Account; fn save(&self, a:&Account); }
  trait RosterStore   { fn load(&self)->StrategyBook; fn save(&self, b:&StrategyBook); }
  trait LogSink       { fn emit(&self, line: &LogLine); }
  ```

  The engine already funnels all I/O through a few functions, so this is a
  localized change — the tick loop and strategies stay identical.

### 3. Transport

- **Control** (start/stop/enable/disable/add/remove, param edits): REST.
- **Live updates** (logs, fills, status, equity): WebSocket or SSE — the engine
  already produces a `LogLine` stream and `InstanceStatus` snapshots that map
  directly onto push messages.

### 4. Client (TUI/CLI)

- Introduce an `EngineClient` trait with two implementations:
  `LocalEngine(StrategyEngine)` and `RemoteEngine(http+ws)`. The TUI's
  Strategies/Logs views already call a small set of methods
  (`snapshot`, `recent_logs`, `start`, `stop`, `set_enabled`, `add`, `remove`)
  — make those the trait, and local vs. remote becomes a one-line swap.

## Security model

- **Least privilege**: workers hold scoped credentials with on-chain allowance
  limits; the control plane never holds raw keys.
- **Risk engine in the worker**: hard caps (per-order, per-agent notional,
  daily loss) enforced *below* strategy logic, so a misbehaving plugin cannot
  exceed limits. A global kill-switch halts all agents.
- **Auditability**: signed, append-only log of orders and control actions.
- **Isolation**: one sandbox/container per user (or per agent) so plugins cannot
  see each other's state or credentials.

## Migration checklist

1. Extract `AccountStore` / `RosterStore` / `LogSink` traits; keep the local
   file impls as the default. *(Engine already centralizes this I/O.)*
2. Add `EngineClient` trait; wrap today's `StrategyEngine` as `LocalEngine`.
3. Stand up the control-plane API + worker image (the headless engine).
4. Implement `RemoteEngine` and add a local/remote toggle in the TUI Settings
   view.
5. Layer in the risk engine and audit log before enabling `ExecutionMode::Live`
   on workers.

The deliberate consequence of Phase 1's design — plugins, a single execution
surface, read-only UI, centralized I/O — is that none of the above touches
strategy code or the tick loop.
