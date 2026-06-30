//! Pure simulation logic. No I/O: callers supply quotes/book levels and a
//! timestamp, which keeps every rule here unit-testable and reusable for
//! future backtesting.

use std::collections::BTreeMap;

use anyhow::{Result, bail};
use chrono::{DateTime, Utc};
use polymarket_client_sdk_v2::types::Decimal;

use super::types::{
    MarketMeta, OpenOrder, OrderKind, PaperAccount, PortfolioView, PositionView, Quote, Stats,
    Trade, TradeSide,
};

/// Outcome of placing a limit order: an immediate (marketable) fill, or a
/// resting order.
pub(crate) enum LimitOutcome {
    Filled(Trade),
    Resting(OpenOrder),
}

fn validate_price(price: Decimal) -> Result<()> {
    if price <= Decimal::ZERO || price >= Decimal::ONE {
        bail!("Price must be between 0 and 1 (exclusive), got {price}");
    }
    Ok(())
}

fn validate_positive(value: Decimal, label: &str) -> Result<()> {
    if value <= Decimal::ZERO {
        bail!("{label} must be positive, got {value}");
    }
    Ok(())
}

/// Pre-trade slippage guard: estimate the average fill price of a market
/// order against the current book and reject it when that price strays more
/// than `max_pct` percent from the touch (best ask for buys, best bid for
/// sells). Pure estimation — nothing is executed.
pub(crate) fn check_slippage(
    levels: &[(Decimal, Decimal)],
    side: TradeSide,
    amount: Decimal,
    max_pct: Decimal,
) -> Result<()> {
    if max_pct <= Decimal::ZERO || amount <= Decimal::ZERO {
        return Ok(()); // guard off or nothing to size
    }
    let Some(&(touch, _)) = levels.first() else {
        return Ok(()); // no book — let execution produce its own error
    };
    let avg = match side {
        // `amount` is a pUSD budget walked down the asks.
        TradeSide::Buy => {
            let mut remaining = amount;
            let mut shares = Decimal::ZERO;
            let mut cost = Decimal::ZERO;
            for &(price, size) in levels {
                if remaining <= Decimal::ZERO {
                    break;
                }
                let take_cost = remaining.min(price * size);
                shares += take_cost / price;
                cost += take_cost;
                remaining -= take_cost;
            }
            if shares <= Decimal::ZERO {
                return Ok(());
            }
            cost / shares
        }
        // `amount` is shares walked down the bids.
        TradeSide::Sell => {
            let mut remaining = amount;
            let mut proceeds = Decimal::ZERO;
            let mut sold = Decimal::ZERO;
            for &(price, size) in levels {
                if remaining <= Decimal::ZERO {
                    break;
                }
                let take = remaining.min(size);
                proceeds += take * price;
                sold += take;
                remaining -= take;
            }
            if sold <= Decimal::ZERO {
                return Ok(());
            }
            proceeds / sold
        }
    };
    let drift_pct = ((avg - touch) / touch * Decimal::ONE_HUNDRED).abs();
    if drift_pct > max_pct {
        bail!(
            "Slippage {:.2}% exceeds the {max_pct}% tolerance (avg fill {} vs touch {touch}). \
             Raise it in Settings or trade smaller.",
            drift_pct,
            avg.round_dp(4)
        );
    }
    Ok(())
}

/// Buy with a pUSD budget by walking the ask side of the book (best first).
/// Fails if the book can't absorb the full amount (FOK semantics).
pub(crate) fn market_buy(
    account: &mut PaperAccount,
    token_id: &str,
    meta: &MarketMeta,
    asks: &[(Decimal, Decimal)],
    bids: &[(Decimal, Decimal)],
    usd_amount: Decimal,
    now: DateTime<Utc>,
) -> Result<Trade> {
    validate_positive(usd_amount, "Amount")?;
    if usd_amount > account.cash {
        bail!(
            "Insufficient paper cash: have {} pUSD available, order needs {usd_amount} pUSD",
            account.cash
        );
    }

    let mut remaining = usd_amount;
    let mut shares = Decimal::ZERO;
    let mut cost = Decimal::ZERO;
    for &(price, size) in asks {
        if remaining <= Decimal::ZERO {
            break;
        }
        let level_cost = price * size;
        let take_cost = remaining.min(level_cost);
        shares += take_cost / price;
        cost += take_cost;
        remaining -= take_cost;
    }
    if remaining > Decimal::ZERO {
        bail!("Insufficient liquidity: order book can only absorb {cost} of {usd_amount} pUSD");
    }

    account.cash -= cost;
    let price = cost / shares;
    let entry_midpoint = midpoint(bids, asks, price);
    Ok(record_buy(
        account,
        token_id,
        meta,
        shares,
        price,
        entry_midpoint,
        OrderKind::Market,
        now,
    ))
}

/// Sell shares by walking the bid side of the book (best first).
/// Fails if the book can't absorb the full size (FOK semantics).
pub(crate) fn market_sell(
    account: &mut PaperAccount,
    token_id: &str,
    bids: &[(Decimal, Decimal)],
    shares: Decimal,
    now: DateTime<Utc>,
) -> Result<Trade> {
    validate_positive(shares, "Size")?;
    check_free_shares(account, token_id, shares)?;

    let mut remaining = shares;
    let mut proceeds = Decimal::ZERO;
    for &(price, size) in bids {
        if remaining <= Decimal::ZERO {
            break;
        }
        let take = remaining.min(size);
        proceeds += take * price;
        remaining -= take;
    }
    if remaining > Decimal::ZERO {
        bail!(
            "Insufficient liquidity: order book bids can only absorb {} of {shares} shares",
            shares - remaining
        );
    }

    let price = proceeds / shares;
    record_sell(account, token_id, shares, price, OrderKind::Market, now)
}

/// Place a limit buy. If marketable (best ask at or below the limit) it fills
/// immediately at the best ask; otherwise it rests and the limit cost is
/// reserved from cash.
pub(crate) fn limit_buy(
    account: &mut PaperAccount,
    token_id: &str,
    meta: &MarketMeta,
    quote: Quote,
    price: Decimal,
    size: Decimal,
    now: DateTime<Utc>,
) -> Result<LimitOutcome> {
    validate_price(price)?;
    validate_positive(size, "Size")?;
    let max_cost = price * size;
    if max_cost > account.cash {
        bail!(
            "Insufficient paper cash: have {} pUSD available, order reserves {max_cost} pUSD",
            account.cash
        );
    }

    if let Some(ask) = quote.best_ask
        && ask <= price
    {
        account.cash -= ask * size;
        let entry_midpoint = match (quote.best_bid, quote.best_ask) {
            (Some(b), Some(a)) => (b + a) / Decimal::from(2),
            _ => ask,
        };
        let trade = record_buy(
            account,
            token_id,
            meta,
            size,
            ask,
            entry_midpoint,
            OrderKind::Limit,
            now,
        );
        return Ok(LimitOutcome::Filled(trade));
    }

    account.cash -= max_cost;
    let order = OpenOrder {
        id: account.take_id(),
        created_at: now,
        token_id: token_id.to_string(),
        question: meta.question.clone(),
        outcome: meta.outcome.clone(),
        side: TradeSide::Buy,
        price,
        size,
    };
    account.open_orders.push(order.clone());
    Ok(LimitOutcome::Resting(order))
}

/// Place a limit sell. If marketable (best bid at or above the limit) it
/// fills immediately at the best bid; otherwise it rests and the shares are
/// reserved against further sells.
pub(crate) fn limit_sell(
    account: &mut PaperAccount,
    token_id: &str,
    quote: Quote,
    price: Decimal,
    size: Decimal,
    now: DateTime<Utc>,
) -> Result<LimitOutcome> {
    validate_price(price)?;
    validate_positive(size, "Size")?;
    check_free_shares(account, token_id, size)?;

    if let Some(bid) = quote.best_bid
        && bid >= price
    {
        let trade = record_sell(account, token_id, size, bid, OrderKind::Limit, now)?;
        return Ok(LimitOutcome::Filled(trade));
    }

    let position = &account.positions[token_id];
    let (question, outcome) = (position.question.clone(), position.outcome.clone());
    let order = OpenOrder {
        id: account.take_id(),
        created_at: now,
        token_id: token_id.to_string(),
        question,
        outcome,
        side: TradeSide::Sell,
        price,
        size,
    };
    account.open_orders.push(order.clone());
    Ok(LimitOutcome::Resting(order))
}

/// Fill resting orders whose limit price the market has crossed. Maker
/// semantics: fills execute at the order's own limit price. Returns the
/// resulting trades.
pub(crate) fn settle_open_orders(
    account: &mut PaperAccount,
    quotes: &BTreeMap<String, Quote>,
    now: DateTime<Utc>,
) -> Vec<Trade> {
    let mut fills = Vec::new();
    let orders = std::mem::take(&mut account.open_orders);
    for order in orders {
        let quote = quotes.get(&order.token_id).copied().unwrap_or_default();
        let crossed = match order.side {
            TradeSide::Buy => quote.best_ask.is_some_and(|ask| ask <= order.price),
            TradeSide::Sell => quote.best_bid.is_some_and(|bid| bid >= order.price),
        };
        if !crossed {
            account.open_orders.push(order);
            continue;
        }
        match order.side {
            TradeSide::Buy => {
                // Reserved cash (price * size) was deducted at placement.
                let meta = MarketMeta {
                    question: order.question.clone(),
                    outcome: order.outcome.clone(),
                };
                let entry_midpoint = match (quote.best_bid, quote.best_ask) {
                    (Some(b), Some(a)) => (b + a) / Decimal::from(2),
                    _ => order.price,
                };
                fills.push(record_buy(
                    account,
                    &order.token_id,
                    &meta,
                    order.size,
                    order.price,
                    entry_midpoint,
                    OrderKind::Limit,
                    now,
                ));
            }
            TradeSide::Sell => {
                // The position always holds reserved sell shares, so this
                // cannot fail; if it somehow does, keep the order resting.
                match record_sell(
                    account,
                    &order.token_id,
                    order.size,
                    order.price,
                    OrderKind::Limit,
                    now,
                ) {
                    Ok(trade) => fills.push(trade),
                    Err(_) => account.open_orders.push(order),
                }
            }
        }
    }
    fills
}

/// Settle a position at market resolution: every share pays out `payout`
/// pUSD ($1 won, $0 lost). The token's resting orders are cancelled first
/// (refunding reserved buy cash, releasing reserved sell shares), then the
/// whole position closes at the payout price as a settlement trade.
pub(crate) fn settle_position(
    account: &mut PaperAccount,
    token_id: &str,
    payout: Decimal,
    now: DateTime<Utc>,
) -> Result<Trade> {
    if payout < Decimal::ZERO || payout > Decimal::ONE {
        bail!("Settlement payout must be between 0 and 1, got {payout}");
    }
    if !account.positions.contains_key(token_id) {
        bail!("No paper position in token {token_id}");
    }
    let orders = std::mem::take(&mut account.open_orders);
    for order in orders {
        if order.token_id == token_id {
            if order.side == TradeSide::Buy {
                account.cash += order.price * order.size;
            }
        } else {
            account.open_orders.push(order);
        }
    }
    let size = account.positions[token_id].size;
    record_sell(account, token_id, size, payout, OrderKind::Settlement, now)
}

/// Cancel a resting order, refunding reserved cash for buys.
pub(crate) fn cancel_order(account: &mut PaperAccount, order_id: u64) -> Result<OpenOrder> {
    let idx = account
        .open_orders
        .iter()
        .position(|o| o.id == order_id)
        .ok_or_else(|| anyhow::anyhow!("No open paper order with ID {order_id}"))?;
    let order = account.open_orders.remove(idx);
    if order.side == TradeSide::Buy {
        account.cash += order.price * order.size;
    }
    Ok(order)
}

/// Build the portfolio summary, marking positions with the supplied prices
/// (typically midpoints keyed by token ID).
pub(crate) fn portfolio_view(
    account: &PaperAccount,
    marks: &BTreeMap<String, Decimal>,
) -> PortfolioView {
    let mut positions_value = Decimal::ZERO;
    let mut unrealized = Decimal::ZERO;
    let positions: Vec<PositionView> = account
        .positions
        .values()
        .map(|p| {
            let mark = marks.get(&p.token_id).copied();
            let value = mark.map(|m| m * p.size);
            // Basis is actual cost (avg fill). With the mark at the bid, this
            // makes unrealized PnL equal what selling now would realize.
            let upnl = mark.map(|m| (m - p.avg_price) * p.size);
            positions_value += value.unwrap_or(Decimal::ZERO);
            unrealized += upnl.unwrap_or(Decimal::ZERO);
            PositionView {
                position: p.clone(),
                mark_price: mark,
                market_value: value,
                unrealized_pnl: upnl,
            }
        })
        .collect();

    let reserved_cash = account.reserved_cash();
    let equity = account.cash + reserved_cash + positions_value;
    let realized = realized_pnl(account);
    let roi_pct = if account.initial_balance > Decimal::ZERO {
        // Paper: equity vs the fixed starting bankroll.
        ((equity - account.initial_balance) / account.initial_balance * Decimal::ONE_HUNDRED)
            .round_dp(2)
    } else {
        // Live: no starting bankroll, so ROI = total PnL / total cost basis.
        // Closed-trade entry cost = exit notional − realized; open = avg × size.
        let closed_basis: Decimal = account
            .trades
            .iter()
            .filter_map(|t| t.realized_pnl.map(|r| t.notional - r))
            .sum();
        let open_basis: Decimal = account
            .positions
            .values()
            .map(|p| p.avg_price * p.size)
            .sum();
        let cost_basis = closed_basis + open_basis;
        if cost_basis > Decimal::ZERO {
            ((realized + unrealized) / cost_basis * Decimal::ONE_HUNDRED).round_dp(2)
        } else {
            Decimal::ZERO
        }
    };

    PortfolioView {
        initial_balance: account.initial_balance,
        cash: account.cash,
        reserved_cash,
        positions_value,
        equity,
        realized_pnl: realized,
        unrealized_pnl: unrealized,
        roi_pct,
        open_orders: account.open_orders.len(),
        positions,
    }
}

/// Total realized PnL across the trade log.
pub(crate) fn realized_pnl(account: &PaperAccount) -> Decimal {
    account.trades.iter().filter_map(|t| t.realized_pnl).sum()
}

/// Performance statistics derived from the trade log.
pub(crate) fn compute_stats(account: &PaperAccount) -> Stats {
    let trades = &account.trades;
    let buys = trades.iter().filter(|t| t.side == TradeSide::Buy).count();
    let sells = trades.len() - buys;

    let closed: Vec<&Trade> = trades.iter().filter(|t| t.realized_pnl.is_some()).collect();
    let wins = closed
        .iter()
        .filter(|t| t.realized_pnl.unwrap_or_default() > Decimal::ZERO)
        .count();
    let losses = closed
        .iter()
        .filter(|t| t.realized_pnl.unwrap_or_default() < Decimal::ZERO)
        .count();
    let win_rate_pct = (!closed.is_empty()).then(|| {
        (Decimal::from(wins as u64) / Decimal::from(closed.len() as u64) * Decimal::ONE_HUNDRED)
            .round_dp(2)
    });

    let best_trade = closed
        .iter()
        .max_by_key(|t| t.realized_pnl.unwrap_or_default())
        .map(|t| (*t).clone());
    let worst_trade = closed
        .iter()
        .min_by_key(|t| t.realized_pnl.unwrap_or_default())
        .map(|t| (*t).clone());

    let mut daily: BTreeMap<chrono::NaiveDate, Decimal> = BTreeMap::new();
    for t in &closed {
        *daily.entry(t.timestamp.date_naive()).or_default() += t.realized_pnl.unwrap_or_default();
    }
    let daily_pnl: Vec<_> = daily.into_iter().collect();

    let mut cumulative = account.initial_balance;
    let equity_curve: Vec<_> = daily_pnl
        .iter()
        .map(|&(date, pnl)| {
            cumulative += pnl;
            (date, cumulative)
        })
        .collect();

    Stats {
        total_trades: trades.len(),
        buys,
        sells,
        wins,
        losses,
        win_rate_pct,
        realized_pnl: realized_pnl(account),
        volume: trades.iter().map(|t| t.notional).sum(),
        best_trade,
        worst_trade,
        daily_pnl,
        equity_curve,
    }
}

/// Compute the bid-ask midpoint from order book levels. Falls back to
/// `fallback` when only one side (or neither) is available.
fn midpoint(
    bids: &[(Decimal, Decimal)],
    asks: &[(Decimal, Decimal)],
    fallback: Decimal,
) -> Decimal {
    match (bids.first(), asks.first()) {
        (Some(&(b, _)), Some(&(a, _))) => (b + a) / Decimal::from(2),
        _ => fallback,
    }
}

/// Record a buy fill. Caller must already have deducted the cash.
/// `entry_midpoint` is the bid-ask midpoint at fill time, used so that
/// unrealized PnL tracks market movement rather than the spread.
#[allow(clippy::too_many_arguments)]
fn record_buy(
    account: &mut PaperAccount,
    token_id: &str,
    meta: &MarketMeta,
    shares: Decimal,
    price: Decimal,
    entry_midpoint: Decimal,
    kind: OrderKind,
    now: DateTime<Utc>,
) -> Trade {
    let position = account
        .positions
        .entry(token_id.to_string())
        .or_insert_with(|| super::types::Position {
            token_id: token_id.to_string(),
            question: meta.question.clone(),
            outcome: meta.outcome.clone(),
            size: Decimal::ZERO,
            avg_price: Decimal::ZERO,
            realized_pnl: Decimal::ZERO,
            entry_midpoint: Decimal::ZERO,
        });
    let new_size = position.size + shares;
    position.avg_price = (position.avg_price * position.size + price * shares) / new_size;
    position.entry_midpoint =
        (position.entry_midpoint * position.size + entry_midpoint * shares) / new_size;
    position.size = new_size;

    let trade = Trade {
        id: account.take_id(),
        timestamp: now,
        token_id: token_id.to_string(),
        question: meta.question.clone(),
        outcome: meta.outcome.clone(),
        side: TradeSide::Buy,
        kind,
        size: shares,
        price,
        notional: shares * price,
        realized_pnl: None,
    };
    account.trades.push(trade.clone());
    trade
}

/// Record a sell fill: credits cash, realizes PnL against the average entry,
/// and removes the position when fully closed.
fn record_sell(
    account: &mut PaperAccount,
    token_id: &str,
    shares: Decimal,
    price: Decimal,
    kind: OrderKind,
    now: DateTime<Utc>,
) -> Result<Trade> {
    let trade_id = account.take_id();
    let position = account
        .positions
        .get_mut(token_id)
        .ok_or_else(|| anyhow::anyhow!("No paper position in token {token_id}"))?;
    if position.size < shares {
        bail!(
            "Insufficient paper shares: hold {}, tried to sell {shares}",
            position.size
        );
    }

    let realized = (price - position.avg_price) * shares;
    position.size -= shares;
    position.realized_pnl += realized;
    account.cash += price * shares;

    let trade = Trade {
        id: trade_id,
        timestamp: now,
        token_id: token_id.to_string(),
        question: position.question.clone(),
        outcome: position.outcome.clone(),
        side: TradeSide::Sell,
        kind,
        size: shares,
        price,
        notional: shares * price,
        realized_pnl: Some(realized),
    };
    // Close on exact zero or sub-penny dust (rounded sell sizes can leave a
    // residual like 0.003 that would otherwise linger showing 0.0 shares).
    if position.size <= Decimal::new(1, 2) {
        account.positions.remove(token_id);
    }
    account.trades.push(trade.clone());
    Ok(trade)
}

/// Ensure `shares` of `token_id` are held and not reserved by open sell
/// orders.
fn check_free_shares(account: &PaperAccount, token_id: &str, shares: Decimal) -> Result<()> {
    let held = account
        .positions
        .get(token_id)
        .map_or(Decimal::ZERO, |p| p.size);
    let free = held - account.reserved_shares(token_id);
    if shares > free {
        bail!(
            "Insufficient paper shares: hold {held}, {} reserved by open orders, tried to sell {shares}",
            held - free
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use rust_decimal_macros::dec;

    fn meta() -> MarketMeta {
        MarketMeta {
            question: "Will it rain tomorrow?".into(),
            outcome: "Yes".into(),
        }
    }

    fn account() -> PaperAccount {
        PaperAccount::new(dec!(10_000), true)
    }

    fn now() -> DateTime<Utc> {
        Utc::now()
    }

    const TOKEN: &str = "123";

    #[test]
    fn market_buy_walks_book_and_averages_price() {
        let mut acct = account();
        // $50 at 0.50 ($25 worth... no: level cost = 0.50*100 = $50), then 0.60.
        let asks = [(dec!(0.50), dec!(100)), (dec!(0.60), dec!(100))];
        let bids = [(dec!(0.48), dec!(100))];
        let trade = market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(80), now()).unwrap();
        // $50 buys 100 shares at 0.50, remaining $30 buys 50 shares at 0.60.
        assert_eq!(trade.size, dec!(150));
        assert_eq!(trade.notional, dec!(80));
        assert_eq!(acct.cash, dec!(9_920));
        let pos = &acct.positions[TOKEN];
        assert_eq!(pos.size, dec!(150));
        assert_eq!(pos.avg_price.round_dp(4), dec!(0.5333));
    }

    #[test]
    fn market_buy_rejects_insufficient_cash() {
        let mut acct = account();
        let asks = [(dec!(0.50), dec!(1_000_000))];
        let bids = [(dec!(0.48), dec!(1_000_000))];
        let err =
            market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(20_000), now()).unwrap_err();
        assert!(err.to_string().contains("Insufficient paper cash"));
    }

    #[test]
    fn market_buy_rejects_thin_book() {
        let mut acct = account();
        let asks = [(dec!(0.50), dec!(10))]; // only $5 of depth
        let bids = [(dec!(0.48), dec!(10))];
        let err =
            market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(100), now()).unwrap_err();
        assert!(err.to_string().contains("Insufficient liquidity"));
        assert_eq!(acct.cash, dec!(10_000)); // nothing deducted
    }

    #[test]
    fn market_sell_realizes_pnl_and_closes_position() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();

        let bids = [(dec!(0.70), dec!(100))];
        let trade = market_sell(&mut acct, TOKEN, &bids, dec!(100), now()).unwrap();
        assert_eq!(trade.realized_pnl, Some(dec!(30))); // (0.70-0.40)*100
        assert_eq!(acct.cash, dec!(10_030));
        assert!(acct.positions.is_empty());
    }

    #[test]
    fn market_sell_rejects_overselling() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();
        let bids = [(dec!(0.70), dec!(500))];
        assert!(market_sell(&mut acct, TOKEN, &bids, dec!(200), now()).is_err());
    }

    #[test]
    fn limit_buy_marketable_fills_at_ask() {
        let mut acct = account();
        let quote = Quote {
            best_bid: Some(dec!(0.48)),
            best_ask: Some(dec!(0.50)),
        };
        let outcome = limit_buy(
            &mut acct,
            TOKEN,
            &meta(),
            quote,
            dec!(0.55),
            dec!(100),
            now(),
        )
        .unwrap();
        let LimitOutcome::Filled(trade) = outcome else {
            panic!("expected immediate fill");
        };
        assert_eq!(trade.price, dec!(0.50)); // price improvement vs 0.55 limit
        assert_eq!(acct.cash, dec!(9_950));
    }

    #[test]
    fn limit_buy_rests_and_reserves_cash() {
        let mut acct = account();
        let quote = Quote {
            best_bid: Some(dec!(0.48)),
            best_ask: Some(dec!(0.50)),
        };
        let outcome = limit_buy(
            &mut acct,
            TOKEN,
            &meta(),
            quote,
            dec!(0.45),
            dec!(100),
            now(),
        )
        .unwrap();
        assert!(matches!(outcome, LimitOutcome::Resting(_)));
        assert_eq!(acct.cash, dec!(9_955)); // 0.45*100 reserved
        assert_eq!(acct.reserved_cash(), dec!(45));
    }

    #[test]
    fn resting_buy_settles_at_limit_when_crossed() {
        let mut acct = account();
        let quote = Quote {
            best_bid: None,
            best_ask: Some(dec!(0.50)),
        };
        limit_buy(
            &mut acct,
            TOKEN,
            &meta(),
            quote,
            dec!(0.45),
            dec!(100),
            now(),
        )
        .unwrap();

        let mut quotes = BTreeMap::new();
        quotes.insert(
            TOKEN.to_string(),
            Quote {
                best_bid: None,
                best_ask: Some(dec!(0.44)),
            },
        );
        let fills = settle_open_orders(&mut acct, &quotes, now());
        assert_eq!(fills.len(), 1);
        assert_eq!(fills[0].price, dec!(0.45)); // maker fill at own limit
        assert!(acct.open_orders.is_empty());
        assert_eq!(acct.positions[TOKEN].size, dec!(100));
        assert_eq!(acct.cash, dec!(9_955)); // reservation consumed exactly
    }

    #[test]
    fn resting_order_stays_open_when_not_crossed() {
        let mut acct = account();
        let quote = Quote {
            best_bid: None,
            best_ask: Some(dec!(0.50)),
        };
        limit_buy(
            &mut acct,
            TOKEN,
            &meta(),
            quote,
            dec!(0.45),
            dec!(100),
            now(),
        )
        .unwrap();

        let mut quotes = BTreeMap::new();
        quotes.insert(
            TOKEN.to_string(),
            Quote {
                best_bid: None,
                best_ask: Some(dec!(0.49)),
            },
        );
        let fills = settle_open_orders(&mut acct, &quotes, now());
        assert!(fills.is_empty());
        assert_eq!(acct.open_orders.len(), 1);
    }

    #[test]
    fn limit_sell_reserves_shares_against_double_sell() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();

        let quote = Quote {
            best_bid: Some(dec!(0.45)),
            best_ask: Some(dec!(0.50)),
        };
        // Rest a sell of 80 shares above the market.
        let outcome = limit_sell(&mut acct, TOKEN, quote, dec!(0.60), dec!(80), now()).unwrap();
        assert!(matches!(outcome, LimitOutcome::Resting(_)));
        // Only 20 free shares remain.
        let bids = [(dec!(0.45), dec!(1_000))];
        assert!(market_sell(&mut acct, TOKEN, &bids, dec!(50), now()).is_err());
        assert!(market_sell(&mut acct, TOKEN, &bids, dec!(20), now()).is_ok());
    }

    #[test]
    fn cancel_buy_refunds_reserved_cash() {
        let mut acct = account();
        let quote = Quote {
            best_bid: None,
            best_ask: Some(dec!(0.50)),
        };
        let LimitOutcome::Resting(order) = limit_buy(
            &mut acct,
            TOKEN,
            &meta(),
            quote,
            dec!(0.45),
            dec!(100),
            now(),
        )
        .unwrap() else {
            panic!("expected resting order");
        };
        assert_eq!(acct.cash, dec!(9_955));
        cancel_order(&mut acct, order.id).unwrap();
        assert_eq!(acct.cash, dec!(10_000));
        assert!(acct.open_orders.is_empty());
    }

    #[test]
    fn cancel_unknown_order_errors() {
        let mut acct = account();
        assert!(cancel_order(&mut acct, 42).is_err());
    }

    #[test]
    fn settle_won_pays_dollar_per_share_and_closes() {
        let mut acct = account();
        // 100 shares at 0.40 ($40).
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();

        let trade = settle_position(&mut acct, TOKEN, dec!(1), now()).unwrap();
        assert_eq!(trade.kind, OrderKind::Settlement);
        assert_eq!(trade.price, dec!(1));
        assert_eq!(trade.realized_pnl, Some(dec!(60))); // (1 - 0.40) * 100
        assert_eq!(acct.cash, dec!(10_060));
        assert!(acct.positions.is_empty());
    }

    #[test]
    fn settle_lost_pays_zero() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();

        let trade = settle_position(&mut acct, TOKEN, dec!(0), now()).unwrap();
        assert_eq!(trade.realized_pnl, Some(dec!(-40))); // (0 - 0.40) * 100
        assert_eq!(acct.cash, dec!(9_960));
        assert!(acct.positions.is_empty());
    }

    #[test]
    fn settle_cancels_open_orders_and_refunds_reserved_cash() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();
        let quote = Quote {
            best_bid: Some(dec!(0.38)),
            best_ask: Some(dec!(0.40)),
        };
        // Resting buy reserves $30; resting sell reserves 50 of the 100 shares.
        limit_buy(
            &mut acct,
            TOKEN,
            &meta(),
            quote,
            dec!(0.30),
            dec!(100),
            now(),
        )
        .unwrap();
        limit_sell(&mut acct, TOKEN, quote, dec!(0.90), dec!(50), now()).unwrap();
        assert_eq!(acct.open_orders.len(), 2);

        settle_position(&mut acct, TOKEN, dec!(1), now()).unwrap();
        // Reserved $30 refunded + 100 shares paid $1 each.
        assert_eq!(acct.cash, dec!(10_060));
        assert!(acct.open_orders.is_empty());
        assert!(acct.positions.is_empty());
    }

    #[test]
    fn settle_rejects_unknown_token_and_bad_payout() {
        let mut acct = account();
        assert!(settle_position(&mut acct, TOKEN, dec!(1), now()).is_err());
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();
        assert!(settle_position(&mut acct, TOKEN, dec!(1.5), now()).is_err());
        assert!(settle_position(&mut acct, TOKEN, dec!(-0.1), now()).is_err());
    }

    #[test]
    fn position_view_roi_uses_cost_basis() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();
        let mut marks = BTreeMap::new();
        marks.insert(TOKEN.to_string(), dec!(0.50));
        let view = portfolio_view(&acct, &marks);
        // avg fill 0.40, basis $40, upnl $10 → ROI 10/40 = 25%
        let roi = view.positions[0].roi().unwrap();
        assert_eq!(roi.round_dp(4), dec!(0.25));
        // No mark → no ROI.
        let view = portfolio_view(&acct, &BTreeMap::new());
        assert!(view.positions[0].roi().is_none());
    }

    #[test]
    fn portfolio_view_computes_equity_and_roi() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        let bids = [(dec!(0.38), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(40), now()).unwrap();

        let mut marks = BTreeMap::new();
        marks.insert(TOKEN.to_string(), dec!(0.50));
        let view = portfolio_view(&acct, &marks);
        assert_eq!(view.cash, dec!(9_960));
        assert_eq!(view.positions_value, dec!(50));
        // avg fill 0.40 → upnl = (0.50 - 0.40) * 100 = 10
        assert_eq!(view.unrealized_pnl, dec!(10));
        assert_eq!(view.equity, dec!(10_010));
        assert_eq!(view.roi_pct, dec!(0.10));
    }

    #[test]
    fn stats_track_wins_losses_and_extremes() {
        let mut acct = account();
        // Buy 200 shares at 0.50.
        let asks = [(dec!(0.50), dec!(1_000))];
        let bids = [(dec!(0.48), dec!(1_000))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, &bids, dec!(100), now()).unwrap();
        // Winning sell of 100 at 0.60: +$10.
        let bids = [(dec!(0.60), dec!(1_000))];
        market_sell(&mut acct, TOKEN, &bids, dec!(100), now()).unwrap();
        // Losing sell of 100 at 0.40: -$10.
        let bids = [(dec!(0.40), dec!(1_000))];
        market_sell(&mut acct, TOKEN, &bids, dec!(100), now()).unwrap();

        let stats = compute_stats(&acct);
        assert_eq!(stats.total_trades, 3);
        assert_eq!(stats.buys, 1);
        assert_eq!(stats.sells, 2);
        assert_eq!(stats.wins, 1);
        assert_eq!(stats.losses, 1);
        assert_eq!(stats.win_rate_pct, Some(dec!(50)));
        assert_eq!(stats.realized_pnl, dec!(0));
        assert_eq!(stats.best_trade.unwrap().realized_pnl, Some(dec!(10)));
        assert_eq!(stats.worst_trade.unwrap().realized_pnl, Some(dec!(-10)));
        assert_eq!(stats.daily_pnl.len(), 1);
        assert_eq!(stats.equity_curve.last().unwrap().1, dec!(10_000));
    }

    #[test]
    fn slippage_passes_at_the_touch() {
        // Whole order fills at the best ask — zero drift.
        let asks = [(dec!(0.50), dec!(1_000))];
        assert!(check_slippage(&asks, TradeSide::Buy, dec!(100), dec!(2)).is_ok());
    }

    #[test]
    fn slippage_rejects_deep_walks() {
        // $50 at 0.50, then the book jumps to 0.90: avg well past 2%.
        let asks = [(dec!(0.50), dec!(100)), (dec!(0.90), dec!(1_000))];
        let err = check_slippage(&asks, TradeSide::Buy, dec!(500), dec!(2)).unwrap_err();
        assert!(err.to_string().contains("Slippage"));
    }

    #[test]
    fn slippage_checks_sells_against_best_bid() {
        let bids = [(dec!(0.50), dec!(10)), (dec!(0.30), dec!(1_000))];
        assert!(check_slippage(&bids, TradeSide::Sell, dec!(10), dec!(2)).is_ok());
        assert!(check_slippage(&bids, TradeSide::Sell, dec!(500), dec!(2)).is_err());
    }

    #[test]
    fn slippage_guard_off_when_zero() {
        let asks = [(dec!(0.10), dec!(1)), (dec!(0.99), dec!(10_000))];
        assert!(check_slippage(&asks, TradeSide::Buy, dec!(1_000), dec!(0)).is_ok());
    }

    #[test]
    fn rejects_out_of_range_limit_prices() {
        let mut acct = account();
        let quote = Quote::default();
        assert!(limit_buy(&mut acct, TOKEN, &meta(), quote, dec!(0), dec!(10), now()).is_err());
        assert!(limit_buy(&mut acct, TOKEN, &meta(), quote, dec!(1), dec!(10), now()).is_err());
        assert!(limit_buy(&mut acct, TOKEN, &meta(), quote, dec!(1.5), dec!(10), now()).is_err());
    }
}
