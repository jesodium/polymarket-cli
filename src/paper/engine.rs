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

/// Buy with a pUSD budget by walking the ask side of the book (best first).
/// Fails if the book can't absorb the full amount (FOK semantics).
pub(crate) fn market_buy(
    account: &mut PaperAccount,
    token_id: &str,
    meta: &MarketMeta,
    asks: &[(Decimal, Decimal)],
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
    Ok(record_buy(
        account,
        token_id,
        meta,
        shares,
        price,
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
        let trade = record_buy(account, token_id, meta, size, ask, OrderKind::Limit, now);
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
                fills.push(record_buy(
                    account,
                    &order.token_id,
                    &meta,
                    order.size,
                    order.price,
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
    let roi_pct = if account.initial_balance > Decimal::ZERO {
        ((equity - account.initial_balance) / account.initial_balance * Decimal::ONE_HUNDRED)
            .round_dp(2)
    } else {
        Decimal::ZERO
    };

    PortfolioView {
        initial_balance: account.initial_balance,
        cash: account.cash,
        reserved_cash,
        positions_value,
        equity,
        realized_pnl: realized_pnl(account),
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

/// Record a buy fill. Caller must already have deducted the cash.
fn record_buy(
    account: &mut PaperAccount,
    token_id: &str,
    meta: &MarketMeta,
    shares: Decimal,
    price: Decimal,
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
        });
    let new_size = position.size + shares;
    position.avg_price = (position.avg_price * position.size + price * shares) / new_size;
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
    if position.size == Decimal::ZERO {
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
        let trade = market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(80), now()).unwrap();
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
        let err = market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(20_000), now()).unwrap_err();
        assert!(err.to_string().contains("Insufficient paper cash"));
    }

    #[test]
    fn market_buy_rejects_thin_book() {
        let mut acct = account();
        let asks = [(dec!(0.50), dec!(10))]; // only $5 of depth
        let err = market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(100), now()).unwrap_err();
        assert!(err.to_string().contains("Insufficient liquidity"));
        assert_eq!(acct.cash, dec!(10_000)); // nothing deducted
    }

    #[test]
    fn market_sell_realizes_pnl_and_closes_position() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(40), now()).unwrap();

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
        market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(40), now()).unwrap();
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
        market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(40), now()).unwrap();

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
    fn portfolio_view_computes_equity_and_roi() {
        let mut acct = account();
        let asks = [(dec!(0.40), dec!(100))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(40), now()).unwrap();

        let mut marks = BTreeMap::new();
        marks.insert(TOKEN.to_string(), dec!(0.50));
        let view = portfolio_view(&acct, &marks);
        assert_eq!(view.cash, dec!(9_960));
        assert_eq!(view.positions_value, dec!(50));
        assert_eq!(view.unrealized_pnl, dec!(10));
        assert_eq!(view.equity, dec!(10_010));
        assert_eq!(view.roi_pct, dec!(0.10));
    }

    #[test]
    fn stats_track_wins_losses_and_extremes() {
        let mut acct = account();
        // Buy 200 shares at 0.50.
        let asks = [(dec!(0.50), dec!(1_000))];
        market_buy(&mut acct, TOKEN, &meta(), &asks, dec!(100), now()).unwrap();
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
    fn rejects_out_of_range_limit_prices() {
        let mut acct = account();
        let quote = Quote::default();
        assert!(limit_buy(&mut acct, TOKEN, &meta(), quote, dec!(0), dec!(10), now()).is_err());
        assert!(limit_buy(&mut acct, TOKEN, &meta(), quote, dec!(1), dec!(10), now()).is_err());
        assert!(limit_buy(&mut acct, TOKEN, &meta(), quote, dec!(1.5), dec!(10), now()).is_err());
    }
}
