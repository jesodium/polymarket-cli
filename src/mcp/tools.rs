//! MCP tool registry: definitions advertised via `tools/list`, and the
//! mapping from a tool call to the CLI argv that implements it.
//!
//! Keep [`definitions`] and [`build_argv`] in sync — every tool name returned
//! by the former must be handled by the latter.

use anyhow::{Result, bail};
use serde_json::{Value, json};

/// JSON schemas for every exposed tool, returned verbatim by `tools/list`.
pub(super) fn definitions() -> Vec<Value> {
    vec![
        // ── Discovery (Gamma) ────────────────────────────────────────────
        tool(
            "search_markets",
            "Full-text search across Polymarket markets. Returns markets from matching events.",
            obj(
                json!({
                    "query": str_prop("Search text, e.g. 'presidential election'"),
                    "limit": int_prop("Max results per type (default 10)"),
                }),
                &["query"],
            ),
        ),
        tool(
            "list_markets",
            "List markets with optional filters and sorting.",
            obj(
                json!({
                    "active": bool_prop("Only active (true) or inactive (false) markets"),
                    "closed": bool_prop("Only closed (true) or open (false) markets"),
                    "limit": int_prop("Max results (default 25)"),
                    "offset": int_prop("Pagination offset"),
                    "order": str_prop("Sort field, e.g. 'volume_num' or 'liquidity_num'"),
                }),
                &[],
            ),
        ),
        tool(
            "get_market",
            "Get a single market by numeric ID or slug, including its token IDs and prices.",
            obj(
                json!({"id": str_prop("Market numeric ID or slug")}),
                &["id"],
            ),
        ),
        tool(
            "list_events",
            "List events (groups of related markets) with optional filters.",
            obj(
                json!({
                    "active": bool_prop("Only active (true) or inactive (false) events"),
                    "closed": bool_prop("Only closed (true) or open (false) events"),
                    "limit": int_prop("Max results (default 25)"),
                    "offset": int_prop("Pagination offset"),
                    "order": str_prop("Sort field, e.g. 'volume' or 'liquidity'"),
                    "tag": str_prop("Filter by tag slug, e.g. 'politics' or 'crypto'"),
                }),
                &[],
            ),
        ),
        tool(
            "get_event",
            "Get a single event by numeric ID or slug, including its markets.",
            obj(json!({"id": str_prop("Event numeric ID or slug")}), &["id"]),
        ),
        // ── CLOB market data (unauthenticated) ───────────────────────────
        tool(
            "get_price",
            "Best bid/ask price for a token on a given side.",
            obj(
                json!({
                    "token_id": str_prop("Token ID (numeric string)"),
                    "side": enum_prop("Order side", &["buy", "sell"]),
                }),
                &["token_id", "side"],
            ),
        ),
        tool(
            "get_midpoint",
            "Midpoint price (between best bid and ask) for a token.",
            obj(
                json!({"token_id": str_prop("Token ID (numeric string)")}),
                &["token_id"],
            ),
        ),
        tool(
            "get_order_book",
            "Full order book (bids and asks) for a token.",
            obj(
                json!({"token_id": str_prop("Token ID (numeric string)")}),
                &["token_id"],
            ),
        ),
        tool(
            "get_spread",
            "Bid-ask spread for a token.",
            obj(
                json!({"token_id": str_prop("Token ID (numeric string)")}),
                &["token_id"],
            ),
        ),
        tool(
            "price_history",
            "Historical price series for a token.",
            obj(
                json!({
                    "token_id": str_prop("Token ID (numeric string)"),
                    "interval": enum_prop(
                        "Time interval",
                        &["1m", "1h", "6h", "1d", "1w", "max"],
                    ),
                    "fidelity": int_prop("Number of data points"),
                }),
                &["token_id", "interval"],
            ),
        ),
        // ── On-chain data ────────────────────────────────────────────────
        tool(
            "get_positions",
            "Open positions for a wallet address.",
            obj(
                json!({
                    "address": str_prop("Wallet address (0x...)"),
                    "limit": int_prop("Max results (default 25)"),
                    "offset": int_prop("Pagination offset"),
                }),
                &["address"],
            ),
        ),
        tool(
            "get_trades",
            "Trade history for a wallet address.",
            obj(
                json!({
                    "address": str_prop("Wallet address (0x...)"),
                    "limit": int_prop("Max results (default 25)"),
                    "offset": int_prop("Pagination offset"),
                }),
                &["address"],
            ),
        ),
        tool(
            "get_activity",
            "On-chain activity feed for a wallet address.",
            obj(
                json!({
                    "address": str_prop("Wallet address (0x...)"),
                    "limit": int_prop("Max results (default 25)"),
                    "offset": int_prop("Pagination offset"),
                }),
                &["address"],
            ),
        ),
        tool(
            "get_value",
            "Total position value for a wallet address.",
            obj(
                json!({"address": str_prop("Wallet address (0x...)")}),
                &["address"],
            ),
        ),
        tool(
            "leaderboard",
            "Trader leaderboard by PnL or volume.",
            obj(
                json!({
                    "period": enum_prop("Time period", &["day", "week", "month", "all"]),
                    "order_by": enum_prop("Rank by", &["pnl", "vol"]),
                    "limit": int_prop("Max results (default 25)"),
                    "offset": int_prop("Pagination offset"),
                }),
                &[],
            ),
        ),
        // ── Wallet / account (authenticated) ─────────────────────────────
        tool(
            "wallet_address",
            "Show the address of the configured wallet.",
            obj(json!({}), &[]),
        ),
        tool(
            "get_balance",
            "Balance and allowance for the configured wallet (authenticated).",
            obj(
                json!({
                    "asset_type": enum_prop("Asset type", &["collateral", "conditional"]),
                    "token": str_prop("Token ID — required for conditional"),
                }),
                &["asset_type"],
            ),
        ),
        tool(
            "account_status",
            "Account status (e.g. closed-only mode) for the configured wallet (authenticated).",
            obj(json!({}), &[]),
        ),
        tool(
            "list_orders",
            "List open live orders for the configured wallet (authenticated).",
            obj(
                json!({
                    "market": str_prop("Filter by market condition ID (0x...)"),
                    "asset": str_prop("Filter by asset/token ID"),
                    "cursor": str_prop("Pagination cursor"),
                }),
                &[],
            ),
        ),
        tool(
            "list_clob_trades",
            "List the configured wallet's CLOB trades (authenticated).",
            obj(
                json!({
                    "market": str_prop("Filter by market condition ID (0x...)"),
                    "asset": str_prop("Filter by asset/token ID"),
                }),
                &[],
            ),
        ),
        // ── Order placement (paper or live) ──────────────────────────────
        tool(
            "create_limit_order",
            "Place a limit order. Routes to the paper simulator when paper mode is on \
             or when `paper: true` is passed; otherwise signs and submits to the live CLOB.",
            obj(
                json!({
                    "token": str_prop("Token ID (numeric string)"),
                    "side": enum_prop("Order side", &["buy", "sell"]),
                    "price": str_prop("Limit price, e.g. '0.50'"),
                    "size": str_prop("Number of shares, e.g. '10'"),
                    "order_type": enum_prop(
                        "Order type (default GTC)",
                        &["GTC", "FOK", "GTD", "FAK"],
                    ),
                    "post_only": bool_prop("Reject if the order would take liquidity"),
                    "paper": bool_prop("Force the paper simulator regardless of global mode"),
                }),
                &["token", "side", "price", "size"],
            ),
        ),
        tool(
            "create_market_order",
            "Place a market order. Amount is pUSD for buys, shares for sells. Routes to the \
             paper simulator when paper mode is on or `paper: true`; otherwise live.",
            obj(
                json!({
                    "token": str_prop("Token ID (numeric string)"),
                    "side": enum_prop("Order side", &["buy", "sell"]),
                    "amount": str_prop("pUSD to spend (buy) or shares to sell (sell)"),
                    "order_type": enum_prop("Order type (default FOK)", &["FOK", "FAK"]),
                    "paper": bool_prop("Force the paper simulator regardless of global mode"),
                }),
                &["token", "side", "amount"],
            ),
        ),
        tool(
            "cancel_order",
            "Cancel a single live order by ID (authenticated).",
            obj(json!({"order_id": str_prop("Order ID")}), &["order_id"]),
        ),
        tool(
            "cancel_all_orders",
            "Cancel all open live orders for the configured wallet (authenticated).",
            obj(json!({}), &[]),
        ),
        // ── Paper trading ────────────────────────────────────────────────
        tool(
            "paper_status",
            "Show paper mode status and account summary.",
            obj(json!({}), &[]),
        ),
        tool(
            "paper_enable",
            "Turn on paper mode (creates a virtual account if none exists).",
            obj(json!({}), &[]),
        ),
        tool(
            "paper_disable",
            "Turn off paper mode (account data is kept).",
            obj(json!({}), &[]),
        ),
        tool(
            "paper_reset",
            "Start over with a fresh virtual account.",
            obj(
                json!({"balance": str_prop("Starting virtual balance in pUSD (default 10000)")}),
                &[],
            ),
        ),
        tool(
            "paper_buy",
            "Simulated buy: market (pass `amount`) or limit (pass `price` and `size`).",
            obj(
                json!({
                    "token_id": str_prop("Token ID (numeric string)"),
                    "amount": str_prop("pUSD to spend (market buy)"),
                    "price": str_prop("Limit price (with `size`)"),
                    "size": str_prop("Shares (limit buy, with `price`)"),
                }),
                &["token_id"],
            ),
        ),
        tool(
            "paper_sell",
            "Simulated sell: market by default, limit when `price` is given.",
            obj(
                json!({
                    "token_id": str_prop("Token ID (numeric string)"),
                    "size": str_prop("Number of shares to sell"),
                    "price": str_prop("Limit price (omit for a market sell)"),
                }),
                &["token_id", "size"],
            ),
        ),
        tool(
            "paper_portfolio",
            "Show the virtual portfolio: cash, positions, PnL, ROI.",
            obj(json!({}), &[]),
        ),
        tool(
            "paper_history",
            "Show the simulated trade log.",
            obj(
                json!({"limit": int_prop("Max trades, most recent first (default 50)")}),
                &[],
            ),
        ),
        tool(
            "paper_orders",
            "List resting paper limit orders.",
            obj(json!({}), &[]),
        ),
        tool(
            "paper_cancel",
            "Cancel a resting paper limit order.",
            obj(
                json!({"order_id": int_prop("Paper order ID")}),
                &["order_id"],
            ),
        ),
        tool(
            "paper_stats",
            "Paper performance analytics: win rate, best/worst trade, daily PnL.",
            obj(json!({}), &[]),
        ),
        // ── Misc ─────────────────────────────────────────────────────────
        tool(
            "status",
            "Check Polymarket API health.",
            obj(json!({}), &[]),
        ),
        tool(
            "run_cli",
            "Escape hatch: run any non-interactive subcommand directly with `--output json`. \
             Pass `args` as the argument vector, e.g. [\"clob\", \"tick-size\", \"<token>\"]. \
             Interactive and key-management commands are blocked.",
            obj(
                json!({
                    "args": {
                        "type": "array",
                        "items": {"type": "string"},
                        "description": "Subcommand and arguments, excluding the binary name and --output",
                    }
                }),
                &["args"],
            ),
        ),
    ]
}

/// Map a tool call to the CLI argv that implements it. The caller prepends the
/// binary path and `--output json`.
pub(super) fn build_argv(name: &str, args: &Value) -> Result<Vec<String>> {
    let argv = match name {
        // ── Discovery ────────────────────────────────────────────────────
        "search_markets" => {
            let mut a = svec(&["markets", "search", &req_str(args, "query")?]);
            push_opt(&mut a, args, "limit", "--limit");
            a
        }
        "list_markets" => {
            let mut a = svec(&["markets", "list"]);
            push_opt(&mut a, args, "active", "--active");
            push_opt(&mut a, args, "closed", "--closed");
            push_opt(&mut a, args, "limit", "--limit");
            push_opt(&mut a, args, "offset", "--offset");
            push_opt(&mut a, args, "order", "--order");
            a
        }
        "get_market" => svec(&["markets", "get", &req_str(args, "id")?]),
        "list_events" => {
            let mut a = svec(&["events", "list"]);
            push_opt(&mut a, args, "active", "--active");
            push_opt(&mut a, args, "closed", "--closed");
            push_opt(&mut a, args, "limit", "--limit");
            push_opt(&mut a, args, "offset", "--offset");
            push_opt(&mut a, args, "order", "--order");
            push_opt(&mut a, args, "tag", "--tag");
            a
        }
        "get_event" => svec(&["events", "get", &req_str(args, "id")?]),

        // ── CLOB market data ─────────────────────────────────────────────
        "get_price" => svec(&[
            "clob",
            "price",
            &req_str(args, "token_id")?,
            "--side",
            &req_str(args, "side")?,
        ]),
        "get_midpoint" => svec(&["clob", "midpoint", &req_str(args, "token_id")?]),
        "get_order_book" => svec(&["clob", "book", &req_str(args, "token_id")?]),
        "get_spread" => svec(&["clob", "spread", &req_str(args, "token_id")?]),
        "price_history" => {
            let mut a = svec(&[
                "clob",
                "price-history",
                &req_str(args, "token_id")?,
                "--interval",
                &req_str(args, "interval")?,
            ]);
            push_opt(&mut a, args, "fidelity", "--fidelity");
            a
        }

        // ── On-chain data ────────────────────────────────────────────────
        "get_positions" => data_address(args, "positions")?,
        "get_trades" => data_address(args, "trades")?,
        "get_activity" => data_address(args, "activity")?,
        "get_value" => svec(&["data", "value", &req_str(args, "address")?]),
        "leaderboard" => {
            let mut a = svec(&["data", "leaderboard"]);
            push_opt(&mut a, args, "period", "--period");
            push_opt(&mut a, args, "order_by", "--order-by");
            push_opt(&mut a, args, "limit", "--limit");
            push_opt(&mut a, args, "offset", "--offset");
            a
        }

        // ── Wallet / account ─────────────────────────────────────────────
        "wallet_address" => svec(&["wallet", "address"]),
        "get_balance" => {
            let mut a = svec(&[
                "clob",
                "balance",
                "--asset-type",
                &req_str(args, "asset_type")?,
            ]);
            push_opt(&mut a, args, "token", "--token");
            a
        }
        "account_status" => svec(&["clob", "account-status"]),
        "list_orders" => {
            let mut a = svec(&["clob", "orders"]);
            push_opt(&mut a, args, "market", "--market");
            push_opt(&mut a, args, "asset", "--asset");
            push_opt(&mut a, args, "cursor", "--cursor");
            a
        }
        "list_clob_trades" => {
            let mut a = svec(&["clob", "trades"]);
            push_opt(&mut a, args, "market", "--market");
            push_opt(&mut a, args, "asset", "--asset");
            a
        }

        // ── Order placement (paper or live) ──────────────────────────────
        "create_limit_order" => {
            let mut a = svec(&[
                "clob",
                "create-order",
                "--token",
                &req_str(args, "token")?,
                "--side",
                &req_str(args, "side")?,
                "--price",
                &req_str(args, "price")?,
                "--size",
                &req_str(args, "size")?,
            ]);
            push_opt(&mut a, args, "order_type", "--order-type");
            push_flag(&mut a, args, "post_only", "--post-only");
            push_flag(&mut a, args, "paper", "--paper");
            a
        }
        "create_market_order" => {
            let mut a = svec(&[
                "clob",
                "market-order",
                "--token",
                &req_str(args, "token")?,
                "--side",
                &req_str(args, "side")?,
                "--amount",
                &req_str(args, "amount")?,
            ]);
            push_opt(&mut a, args, "order_type", "--order-type");
            push_flag(&mut a, args, "paper", "--paper");
            a
        }
        "cancel_order" => svec(&["clob", "cancel", &req_str(args, "order_id")?]),
        "cancel_all_orders" => svec(&["clob", "cancel-all"]),

        // ── Paper trading ────────────────────────────────────────────────
        "paper_status" => svec(&["paper", "status"]),
        "paper_enable" => svec(&["paper", "enable"]),
        "paper_disable" => svec(&["paper", "disable"]),
        "paper_reset" => {
            let mut a = svec(&["paper", "reset"]);
            push_opt(&mut a, args, "balance", "--balance");
            a
        }
        "paper_buy" => {
            let mut a = svec(&["paper", "buy", &req_str(args, "token_id")?]);
            push_opt(&mut a, args, "amount", "--amount");
            push_opt(&mut a, args, "price", "--price");
            push_opt(&mut a, args, "size", "--size");
            a
        }
        "paper_sell" => {
            let mut a = svec(&[
                "paper",
                "sell",
                &req_str(args, "token_id")?,
                "--size",
                &req_str(args, "size")?,
            ]);
            push_opt(&mut a, args, "price", "--price");
            a
        }
        "paper_portfolio" => svec(&["paper", "portfolio"]),
        "paper_history" => {
            let mut a = svec(&["paper", "history"]);
            push_opt(&mut a, args, "limit", "--limit");
            a
        }
        "paper_orders" => svec(&["paper", "orders"]),
        "paper_cancel" => svec(&["paper", "cancel", &req_str(args, "order_id")?]),
        "paper_stats" => svec(&["paper", "stats"]),

        // ── Misc ─────────────────────────────────────────────────────────
        "status" => svec(&["status"]),
        "run_cli" => return run_cli_argv(args),

        other => bail!("unknown tool: {other}"),
    };
    Ok(argv)
}

/// `data <sub> <address> [--limit] [--offset]` — shared by the wallet data tools.
fn data_address(args: &Value, sub: &str) -> Result<Vec<String>> {
    let mut a = svec(&["data", sub, &req_str(args, "address")?]);
    push_opt(&mut a, args, "limit", "--limit");
    push_opt(&mut a, args, "offset", "--offset");
    Ok(a)
}

/// Validate and pass through a raw argv for the `run_cli` escape hatch,
/// blocking interactive and key-management subcommands.
fn run_cli_argv(args: &Value) -> Result<Vec<String>> {
    let arr = args
        .get("args")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow::anyhow!("`args` must be an array of strings"))?;
    let argv: Vec<String> = arr
        .iter()
        .map(|v| {
            value_to_string(Some(v))
                .ok_or_else(|| anyhow::anyhow!("all `args` entries must be strings"))
        })
        .collect::<Result<_>>()?;

    const BLOCKED: &[&str] = &["tui", "shell", "mcp", "upgrade"];
    let first = argv.first().map(String::as_str).unwrap_or_default();
    if first.is_empty() {
        bail!("`args` must not be empty");
    }
    if BLOCKED.contains(&first) {
        bail!("subcommand `{first}` is not allowed via run_cli");
    }
    if first == "wallet" {
        let sub = argv.get(1).map(String::as_str).unwrap_or_default();
        if matches!(sub, "create" | "import" | "reset") {
            bail!("`wallet {sub}` is not allowed via run_cli");
        }
    }
    Ok(argv)
}

// ── argv helpers ─────────────────────────────────────────────────────────

fn svec(parts: &[&str]) -> Vec<String> {
    parts.iter().map(ToString::to_string).collect()
}

fn req_str(args: &Value, key: &str) -> Result<String> {
    value_to_string(args.get(key))
        .ok_or_else(|| anyhow::anyhow!("missing required argument: {key}"))
}

/// Coerce a JSON scalar (string, number, or bool) to its string form.
fn value_to_string(v: Option<&Value>) -> Option<String> {
    match v {
        Some(Value::String(s)) => Some(s.clone()),
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::Bool(b)) => Some(b.to_string()),
        _ => None,
    }
}

/// Append `flag <value>` when the argument is present.
fn push_opt(argv: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if let Some(v) = value_to_string(args.get(key)) {
        argv.push(flag.to_string());
        argv.push(v);
    }
}

/// Append a boolean flag (no value) only when the argument is `true`.
fn push_flag(argv: &mut Vec<String>, args: &Value, key: &str, flag: &str) {
    if args.get(key).and_then(Value::as_bool) == Some(true) {
        argv.push(flag.to_string());
    }
}

// ── schema helpers ───────────────────────────────────────────────────────

fn tool(name: &str, description: &str, schema: Value) -> Value {
    json!({"name": name, "description": description, "inputSchema": schema})
}

fn obj(properties: Value, required: &[&str]) -> Value {
    json!({"type": "object", "properties": properties, "required": required})
}

fn str_prop(description: &str) -> Value {
    json!({"type": "string", "description": description})
}

fn int_prop(description: &str) -> Value {
    json!({"type": "integer", "description": description})
}

fn bool_prop(description: &str) -> Value {
    json!({"type": "boolean", "description": description})
}

fn enum_prop(description: &str, variants: &[&str]) -> Value {
    json!({"type": "string", "description": description, "enum": variants})
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_definition_builds() {
        // Build each tool with its required args filled so the registry and
        // the dispatcher can't drift apart.
        for def in definitions() {
            let name = def["name"].as_str().unwrap();
            let required = def["inputSchema"]["required"].as_array().unwrap();
            let mut args = serde_json::Map::new();
            for r in required {
                let key = r.as_str().unwrap();
                if key == "args" {
                    args.insert(key.into(), json!(["status"]));
                } else {
                    args.insert(key.into(), json!("1"));
                }
            }
            build_argv(name, &Value::Object(args))
                .unwrap_or_else(|e| panic!("tool {name} failed to build: {e}"));
        }
    }

    #[test]
    fn run_cli_blocks_interactive() {
        assert!(run_cli_argv(&json!({"args": ["tui"]})).is_err());
        assert!(run_cli_argv(&json!({"args": ["wallet", "import", "0xabc"]})).is_err());
        assert!(run_cli_argv(&json!({"args": ["markets", "list"]})).is_ok());
        assert!(run_cli_argv(&json!({"args": []})).is_err());
    }

    #[test]
    fn paper_flag_only_when_true() {
        let a = build_argv(
            "create_market_order",
            &json!({"token": "1", "side": "buy", "amount": "5", "paper": true}),
        )
        .unwrap();
        assert!(a.contains(&"--paper".to_string()));

        let b = build_argv(
            "create_market_order",
            &json!({"token": "1", "side": "buy", "amount": "5", "paper": false}),
        )
        .unwrap();
        assert!(!b.contains(&"--paper".to_string()));
    }
}
