//! All TUI rendering. Reads `App` + shared data and paints the current view.

use std::collections::BTreeMap;

use polymarket_client_sdk_v2::types::Decimal;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, Borders, Cell, Clear, List, ListItem, Paragraph, Row, Table, TableState, Wrap,
};

use super::app::{App, ModalField, OrderModal, View};
use crate::paper::engine;
use crate::paper::types::{OrderKind, TradeSide};

const ACCENT: Color = Color::Cyan;
const GOOD: Color = Color::Rgb(63, 185, 80); // green
const BAD: Color = Color::Rgb(248, 81, 73); // red
const DIM: Color = Color::Rgb(120, 130, 145);
const GOLD: Color = Color::Rgb(210, 168, 60);
const HEADER: Color = Color::Rgb(88, 166, 255); // blue
const PANEL: Color = Color::Rgb(70, 78, 92);
const SELECT_BG: Color = Color::Rgb(30, 60, 90);
const ZEBRA_BG: Color = Color::Rgb(26, 28, 34);
const LIVE: Color = Color::Rgb(248, 81, 73);
const PAPER: Color = Color::Rgb(63, 185, 80);

/// The headline colour for the current trading mode.
fn mode_color(app: &App) -> Color {
    if app.live { LIVE } else { PAPER }
}

fn mode_label(app: &App) -> &'static str {
    if app.live {
        " ⏺ LIVE "
    } else {
        " ◆ PAPER "
    }
}

pub(crate) fn render(f: &mut Frame, app: &App) {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(3), // tab bar
            Constraint::Min(5),    // body
            Constraint::Length(3), // status
        ])
        .split(f.area());

    render_tabs(f, app, chunks[0]);
    match app.view {
        View::Dashboard => dashboard(f, app, chunks[1]),
        View::Markets => markets(f, app, chunks[1]),
        View::MarketDetail => market_detail(f, app, chunks[1]),
        View::Portfolio => portfolio(f, app, chunks[1]),
        View::Positions => positions(f, app, chunks[1]),
        View::Orders => orders(f, app, chunks[1]),
        View::History => history(f, app, chunks[1]),
        View::Strategies => strategies(f, app, chunks[1]),
        View::Logs => logs(f, app, chunks[1]),
        View::Settings => settings(f, app, chunks[1]),
    }
    render_status(f, app, chunks[2]);

    if let Some(modal) = &app.modal {
        render_modal(f, app, modal);
    }
    if let Some(sm) = &app.strat_modal {
        render_strat_modal(f, sm);
    }
}

fn render_tabs(f: &mut Frame, app: &App, area: Rect) {
    let mc = mode_color(app);
    let mut spans = vec![
        Span::styled(
            mode_label(app),
            Style::default().fg(Color::Black).bg(mc).bold(),
        ),
        Span::raw(" "),
    ];
    for (i, v) in View::TABS.iter().enumerate() {
        let active = *v == app.view || (app.view == View::MarketDetail && *v == View::Markets);
        let style = if active {
            Style::default().fg(Color::Black).bg(ACCENT).bold()
        } else {
            Style::default().fg(DIM)
        };
        spans.push(Span::styled(format!(" {}·{} ", i + 1, v.title()), style));
    }

    let d = app.data.lock().unwrap();
    let conn = if d.connected {
        Span::styled("● connected", Style::default().fg(GOOD))
    } else {
        Span::styled("○ offline", Style::default().fg(GOLD))
    };
    let markets_status = d.markets_status.clone();
    drop(d);

    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(mc))
        .title(Span::styled(
            " POLYMARKET TERMINAL ",
            Style::default().fg(mc).bold(),
        ))
        .title_top(
            Line::from(vec![
                Span::raw(" "),
                conn,
                Span::styled(format!("  {markets_status} "), Style::default().fg(DIM)),
            ])
            .right_aligned(),
        );
    f.render_widget(Paragraph::new(Line::from(spans)).block(block), area);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let help = match app.view {
        View::Markets => "↑↓/jk move · Enter open · / search · Tab views · q quit",
        View::MarketDetail => "←→ outcome · b buy · s sell · g attach strategy · Esc back",
        View::Orders => "↑↓ move · c cancel · Tab views",
        View::Strategies => "n new · s start · x stop · e enable · d disable · ↑↓ move",
        View::Settings => "Mode is fixed at launch (--paper) · Tab views",
        _ => "Tab/1-9 switch views · ↑↓ move · ? help · q or Ctrl+C quit",
    };
    let mc = mode_color(app);
    let lines = vec![
        Line::from(Span::styled(
            app.status.clone(),
            Style::default().fg(Color::White).bold(),
        )),
        Line::from(Span::styled(help, Style::default().fg(DIM))),
    ];
    let p = Paragraph::new(lines).block(
        Block::default()
            .borders(Borders::ALL)
            .border_style(Style::default().fg(mc)),
    );
    f.render_widget(p, area);
}

// --- Dashboard -------------------------------------------------------------

fn dashboard(f: &mut Frame, app: &App, area: Rect) {
    let marks = marks_snapshot(app);
    let acct = app.account.lock().unwrap();
    let view = engine::portfolio_view(&acct, &marks);
    let daily = daily_pnl(&acct);
    let recent: Vec<_> = acct.trades.iter().rev().take(8).cloned().collect();
    let positions = acct.positions.len();
    let open_orders = acct.open_orders.len();
    drop(acct);
    let running = app.engine.running_count();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(5)])
        .split(area);

    // Metric cards.
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(rows[0]);

    metric_card(f, cards[0], "Portfolio Value", &money(view.equity), ACCENT);
    metric_card(f, cards[1], "Cash Balance", &money(view.cash), Color::White);
    metric_card(
        f,
        cards[2],
        "Daily PnL",
        &signed_money(daily),
        pnl_color(daily),
    );
    let total = view.realized_pnl + view.unrealized_pnl;
    metric_card(
        f,
        cards[3],
        "Total PnL",
        &signed_money(total),
        pnl_color(total),
    );

    // Bottom: counters + recent trades.
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(rows[1]);

    let info = vec![
        kv_line("Open Positions", &positions.to_string()),
        kv_line("Open Orders", &open_orders.to_string()),
        kv_line("Running Strategies", &running.to_string()),
        kv_line("ROI", &format!("{}%", view.roi_pct)),
        kv_line("Realized PnL", &signed_money(view.realized_pnl)),
        kv_line("Unrealized PnL", &signed_money(view.unrealized_pnl)),
    ];
    let p = Paragraph::new(info)
        .block(panel("Account"))
        .wrap(Wrap { trim: true });
    f.render_widget(p, bottom[0]);

    let trade_rows: Vec<Row> = recent
        .iter()
        .map(|t| {
            Row::new(vec![
                Cell::from(t.timestamp.format("%H:%M:%S").to_string()),
                side_cell(t.side),
                Cell::from(truncate(&t.question, 30)),
                Cell::from(t.size.round_dp(1).to_string()),
                Cell::from(format!("{:.3}", t.price)),
            ])
        })
        .collect();
    let table = Table::new(
        trade_rows,
        [
            Constraint::Length(9),
            Constraint::Length(5),
            Constraint::Min(20),
            Constraint::Length(8),
            Constraint::Length(7),
        ],
    )
    .header(header_row(&["Time", "Side", "Market", "Size", "Price"]))
    .block(panel("Recent Trades"));
    f.render_widget(table, bottom[1]);
}

// --- Markets ---------------------------------------------------------------

fn markets(f: &mut Frame, app: &App, area: Rect) {
    let markets = app.filtered_markets();
    let rows: Vec<Row> = markets
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let yes = m
                .prices
                .first()
                .map(|p| pct(*p))
                .unwrap_or_else(|| "—".into());
            Row::new(vec![
                Cell::from(truncate(&m.question, 52)),
                Cell::from(yes).style(Style::default().fg(GOOD)),
                Cell::from(short_money(m.volume)),
                Cell::from(short_money(m.liquidity)),
                Cell::from(status_label(m.closed, m.active)),
            ])
            .style(zebra(i))
        })
        .collect();

    let title = if app.searching {
        format!("Markets — type query, Enter to search: {}_", app.search)
    } else if app.search.is_empty() {
        "Markets".to_string()
    } else if app.search_pending() {
        format!("Markets — searching “{}”…", app.search)
    } else {
        format!(
            "Markets — search “{}” ({} result{})",
            app.search,
            markets.len(),
            if markets.len() == 1 { "" } else { "s" }
        )
    };

    // Empty-state message when a finished search returned nothing.
    if markets.is_empty() && !app.search.is_empty() {
        let msg = if app.search_pending() {
            format!("Searching Gamma for “{}”…", app.search)
        } else {
            format!("No markets match “{}”. Esc to clear.", app.search)
        };
        f.render_widget(Paragraph::new(msg.fg(DIM)).block(panel(&title)), area);
        return;
    }

    let table = Table::new(
        rows,
        [
            Constraint::Min(30),
            Constraint::Length(7),
            Constraint::Length(11),
            Constraint::Length(11),
            Constraint::Length(8),
        ],
    )
    .header(header_row(&[
        "Market",
        "Yes %",
        "Volume",
        "Liquidity",
        "Status",
    ]))
    .block(panel(&title))
    .row_highlight_style(highlight())
    .highlight_symbol("▶ ");
    f.render_stateful_widget(table, area, &mut sel_state(app.markets_sel, markets.len()));
}

// --- Market detail ---------------------------------------------------------

fn market_detail(f: &mut Frame, app: &App, area: Rect) {
    let Some(d) = &app.detail else {
        f.render_widget(
            Paragraph::new("No market selected").block(panel("Market")),
            area,
        );
        return;
    };
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    // Left: market info + outcomes.
    let mut lines = vec![
        Line::from(d.question.clone().bold()),
        Line::from(""),
        Line::from(vec![
            Span::styled("Market ID  ", Style::default().fg(DIM)),
            Span::raw(d.id.clone()),
        ]),
        Line::from(""),
        Line::from("Outcomes (←→ to focus):".fg(ACCENT)),
    ];
    for (i, tid) in d.token_ids.iter().enumerate() {
        let name = d
            .outcomes
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("Outcome {}", i + 1));
        let price = d
            .prices
            .get(i)
            .map(|p| pct(*p))
            .unwrap_or_else(|| "—".into());
        let marker = if i == app.detail_token { "▶ " } else { "  " };
        let style = if i == app.detail_token {
            Style::default().fg(Color::Black).bg(ACCENT)
        } else {
            Style::default()
        };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}{name:<20}"), style),
            Span::raw(format!("  {price}  ")),
            Span::styled(truncate(tid, 16), Style::default().fg(DIM)),
        ]));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(
        "Press b to buy, s to sell, g to attach a strategy.".fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(lines)
            .block(panel("Market Details"))
            .wrap(Wrap { trim: false }),
        cols[0],
    );

    // Right: order book for focused token + position.
    let token = d
        .token_ids
        .get(app.detail_token)
        .cloned()
        .unwrap_or_default();
    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(8), Constraint::Length(6)])
        .split(cols[1]);
    render_book(f, app, &token, right[0]);
    render_position_box(f, app, &token, right[1]);
}

fn render_book(f: &mut Frame, app: &App, token: &str, area: Rect) {
    let data = app.data.lock().unwrap();
    let book = data.book(token).cloned();
    drop(data);
    let mut rows: Vec<Row> = Vec::new();
    if let Some(b) = &book {
        let asks: Vec<_> = b.asks.iter().take(6).rev().collect();
        for (p, s) in asks {
            rows.push(Row::new(vec![
                Cell::from(""),
                Cell::from(""),
                Cell::from(format!("{p:.3}")).style(Style::default().fg(BAD)),
                Cell::from(format!("{}", s.round_dp(0))),
            ]));
        }
        let spread = match (b.best_bid, b.best_ask) {
            (Some(bid), Some(ask)) => format!("spread {:.3}", ask - bid),
            _ => "—".into(),
        };
        rows.push(Row::new(vec![
            Cell::from("──"),
            Cell::from(""),
            Cell::from(spread).style(Style::default().fg(ACCENT)),
            Cell::from(""),
        ]));
        for (p, s) in b.bids.iter().take(6) {
            rows.push(Row::new(vec![
                Cell::from(format!("{}", s.round_dp(0))),
                Cell::from(format!("{p:.3}")).style(Style::default().fg(GOOD)),
                Cell::from(""),
                Cell::from(""),
            ]));
        }
    } else {
        rows.push(Row::new(vec![Cell::from("fetching book…")]));
    }
    let table = Table::new(
        rows,
        [
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(8),
        ],
    )
    .header(header_row(&["BidSz", "Bid", "Ask", "AskSz"]))
    .block(panel("Order Book"));
    f.render_widget(table, area);
}

fn render_position_box(f: &mut Frame, app: &App, token: &str, area: Rect) {
    let acct = app.account.lock().unwrap();
    let pos = acct.positions.get(token).cloned();
    drop(acct);
    let lines = match pos {
        Some(p) => vec![
            kv_line("Shares", &p.size.round_dp(2).to_string()),
            kv_line("Avg Price", &format!("{:.4}", p.avg_price)),
            kv_line("Realized", &signed_money(p.realized_pnl)),
        ],
        None => vec![Line::from("No position".fg(DIM))],
    };
    f.render_widget(Paragraph::new(lines).block(panel("Your Position")), area);
}

// --- Portfolio -------------------------------------------------------------

fn portfolio(f: &mut Frame, app: &App, area: Rect) {
    let marks = marks_snapshot(app);
    let acct = app.account.lock().unwrap();
    let view = engine::portfolio_view(&acct, &marks);
    drop(acct);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(5)])
        .split(area);

    let summary = vec![
        kv_line("Equity", &money(view.equity)),
        kv_line("Cash", &money(view.cash)),
        kv_line("Reserved (open buys)", &money(view.reserved_cash)),
        kv_line("Positions Value", &money(view.positions_value)),
        kv_line("Realized PnL", &signed_money(view.realized_pnl)),
        kv_line("Unrealized PnL", &signed_money(view.unrealized_pnl)),
    ];
    f.render_widget(
        Paragraph::new(summary).block(panel(&format!(
            "Portfolio — ROI {}% (start {})",
            view.roi_pct,
            money(view.initial_balance)
        ))),
        layout[0],
    );

    let rows: Vec<Row> = view
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let upnl = p.unrealized_pnl.unwrap_or_default();
            Row::new(vec![
                Cell::from(truncate(&p.position.question, 34)),
                Cell::from(truncate(&p.position.outcome, 10)),
                Cell::from(p.position.size.round_dp(1).to_string()),
                Cell::from(format!("{:.3}", p.position.avg_price)),
                Cell::from(
                    p.mark_price
                        .map(|m| format!("{m:.3}"))
                        .unwrap_or_else(|| "—".into()),
                ),
                Cell::from(p.market_value.map(money).unwrap_or_else(|| "—".into())),
                Cell::from(signed_money(upnl)).style(Style::default().fg(pnl_color(upnl))),
            ])
            .style(zebra(i))
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header_row(&[
        "Market", "Outcome", "Shares", "Avg", "Mark", "Value", "uPnL",
    ]))
    .block(panel("Holdings"));
    f.render_widget(table, layout[1]);
}

// --- Positions -------------------------------------------------------------

fn positions(f: &mut Frame, app: &App, area: Rect) {
    let marks = marks_snapshot(app);
    let acct = app.account.lock().unwrap();
    let view = engine::portfolio_view(&acct, &marks);
    drop(acct);
    let rows: Vec<Row> = view
        .positions
        .iter()
        .enumerate()
        .map(|(i, p)| {
            let upnl = p.unrealized_pnl.unwrap_or_default();
            Row::new(vec![
                Cell::from(truncate(&p.position.question, 40)),
                Cell::from(truncate(&p.position.outcome, 10)),
                Cell::from(p.position.size.round_dp(2).to_string()),
                Cell::from(format!("{:.4}", p.position.avg_price)),
                Cell::from(
                    p.mark_price
                        .map(|m| format!("{m:.4}"))
                        .unwrap_or_else(|| "—".into()),
                ),
                Cell::from(signed_money(upnl)).style(Style::default().fg(pnl_color(upnl))),
            ])
            .style(zebra(i))
        })
        .collect();
    let n = view.positions.len();
    let table = Table::new(
        rows,
        [
            Constraint::Min(24),
            Constraint::Length(10),
            Constraint::Length(9),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
        ],
    )
    .header(header_row(&[
        "Market", "Outcome", "Shares", "Avg", "Mark", "uPnL",
    ]))
    .block(panel(&format!("Positions ({n})")))
    .row_highlight_style(highlight())
    .highlight_symbol("▶ ");
    f.render_stateful_widget(table, area, &mut sel_state(app.positions_sel, n));
}

// --- Orders ----------------------------------------------------------------

fn orders(f: &mut Frame, app: &App, area: Rect) {
    if app.live {
        f.render_widget(
            Paragraph::new(vec![
                Line::from("Live open orders are managed at the CLOB.".fg(GOLD)),
                Line::from(""),
                Line::from("View:   polymarket clob orders".fg(DIM)),
                Line::from("Cancel: polymarket clob cancel <id>".fg(DIM)),
                Line::from(""),
                Line::from("(In-terminal live order management is on the roadmap.)".fg(DIM)),
            ])
            .block(panel("Open Orders · LIVE")),
            area,
        );
        return;
    }
    let acct = app.account.lock().unwrap();
    let open = acct.open_orders.clone();
    drop(acct);
    let rows: Vec<Row> = open
        .iter()
        .enumerate()
        .map(|(i, o)| {
            Row::new(vec![
                Cell::from(o.id.to_string()),
                side_cell(o.side),
                Cell::from(truncate(&o.question, 34)),
                Cell::from(truncate(&o.outcome, 10)),
                Cell::from(format!("{:.4}", o.price)),
                Cell::from(o.size.round_dp(2).to_string()),
                Cell::from(o.created_at.format("%m-%d %H:%M").to_string()),
            ])
            .style(zebra(i))
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(5),
            Constraint::Length(5),
            Constraint::Min(20),
            Constraint::Length(10),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(12),
        ],
    )
    .header(header_row(&[
        "ID", "Side", "Market", "Outcome", "Price", "Size", "Created",
    ]))
    .block(panel(&format!(
        "Open Orders ({}) — c to cancel",
        open.len()
    )))
    .row_highlight_style(highlight())
    .highlight_symbol("▶ ");
    f.render_stateful_widget(table, area, &mut sel_state(app.orders_sel, open.len()));
}

// --- History ---------------------------------------------------------------

fn history(f: &mut Frame, app: &App, area: Rect) {
    if app.live {
        f.render_widget(
            Paragraph::new(vec![
                Line::from("Live trade history is recorded at the CLOB.".fg(GOLD)),
                Line::from(""),
                Line::from("View: polymarket clob trades".fg(DIM)),
            ])
            .block(panel("Trade History · LIVE")),
            area,
        );
        return;
    }
    let acct = app.account.lock().unwrap();
    let mut trades: Vec<_> = acct.trades.iter().rev().cloned().collect();
    drop(acct);
    let visible = area.height.saturating_sub(3) as usize;
    let start = app.history_scroll.min(trades.len().saturating_sub(1));
    trades = trades.into_iter().skip(start).take(visible).collect();
    let rows: Vec<Row> = trades
        .iter()
        .enumerate()
        .map(|(i, t)| {
            let pnl = t.realized_pnl.map(signed_money).unwrap_or_default();
            Row::new(vec![
                Cell::from(t.timestamp.format("%m-%d %H:%M:%S").to_string()),
                side_cell(t.side),
                Cell::from(t.kind.to_string()),
                Cell::from(truncate(&t.question, 30)),
                Cell::from(t.size.round_dp(2).to_string()),
                Cell::from(format!("{:.4}", t.price)),
                Cell::from(money(t.notional)),
                Cell::from(pnl)
                    .style(Style::default().fg(pnl_color(t.realized_pnl.unwrap_or_default()))),
            ])
            .style(zebra(i))
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(15),
            Constraint::Length(5),
            Constraint::Length(7),
            Constraint::Min(18),
            Constraint::Length(8),
            Constraint::Length(8),
            Constraint::Length(10),
            Constraint::Length(10),
        ],
    )
    .header(header_row(&[
        "Time", "Side", "Type", "Market", "Size", "Price", "Notional", "PnL",
    ]))
    .block(panel("Trade History (↑↓ scroll)"));
    f.render_widget(table, area);
}

// --- Strategies ------------------------------------------------------------

fn strategies(f: &mut Frame, app: &App, area: Rect) {
    let snap = app.engine.snapshot();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(6)])
        .split(area);

    let rows: Vec<Row> = snap
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let (state, color) = if s.running {
                ("● running", GOOD)
            } else if s.enabled {
                ("○ idle", GOLD)
            } else {
                ("· disabled", DIM)
            };
            Row::new(vec![
                Cell::from(s.id.clone()),
                Cell::from(s.kind.clone()),
                Cell::from(state).style(Style::default().fg(color)),
                Cell::from(s.tokens.len().to_string()),
                Cell::from(s.signals.to_string()),
                Cell::from(s.orders.to_string()),
                Cell::from(s.errors.to_string()).style(Style::default().fg(if s.errors > 0 {
                    BAD
                } else {
                    DIM
                })),
                Cell::from(s.last_action.clone().unwrap_or_else(|| "-".into())),
            ])
            .style(zebra(i))
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(16),
            Constraint::Length(14),
            Constraint::Length(9),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Length(7),
            Constraint::Min(16),
        ],
    )
    .header(header_row(&[
        "ID",
        "Kind",
        "State",
        "Tokens",
        "Signals",
        "Orders",
        "Errors",
        "Last Action",
    ]))
    .block(panel(&format!(
        "Strategies — {} mode, {}s tick (n new · s start · x stop · e enable · d disable)",
        app.engine.mode(),
        app.engine.tick_secs()
    )))
    .row_highlight_style(highlight())
    .highlight_symbol("▶ ");
    f.render_stateful_widget(
        table,
        layout[0],
        &mut sel_state(app.strategies_sel, snap.len()),
    );

    // Description of the selected strategy.
    let desc = snap
        .get(app.strategies_sel)
        .map(|s| s.description.clone())
        .unwrap_or_else(|| {
            "No strategies yet. Press n to create one here, or open a market and press g to attach momentum.".into()
        });
    f.render_widget(
        Paragraph::new(desc)
            .block(panel("Details"))
            .wrap(Wrap { trim: true }),
        layout[1],
    );
}

// --- Logs ------------------------------------------------------------------

fn logs(f: &mut Frame, app: &App, area: Rect) {
    let lines = app.engine.recent_logs(500);
    let visible = area.height.saturating_sub(2) as usize;
    let total = lines.len();
    let start = if total > visible {
        (total - visible).saturating_sub(app.logs_scroll)
    } else {
        0
    };
    let items: Vec<ListItem> = lines
        .iter()
        .skip(start)
        .take(visible)
        .map(|l| {
            use crate::strategy::engine::LogLevel;
            let color = match l.level {
                LogLevel::Trade => GOOD,
                LogLevel::Warn => Color::Yellow,
                LogLevel::Error => BAD,
                LogLevel::Info => DIM,
            };
            ListItem::new(Line::from(vec![
                Span::styled(
                    format!("{} ", l.time.format("%H:%M:%S")),
                    Style::default().fg(DIM),
                ),
                Span::styled(
                    format!("{:<5} ", l.level.label()),
                    Style::default().fg(color),
                ),
                Span::styled(
                    format!("{:<14} ", truncate(&l.source, 14)),
                    Style::default().fg(ACCENT),
                ),
                Span::raw(l.message.clone()),
            ]))
        })
        .collect();
    let list = List::new(items).block(panel("Strategy Engine Logs (↑↓ scroll)"));
    f.render_widget(list, area);
}

// --- Settings --------------------------------------------------------------

fn settings(f: &mut Frame, app: &App, area: Rect) {
    let acct = app.account.lock().unwrap();
    let initial = acct.initial_balance;
    let cash = acct.cash;
    drop(acct);
    let cfg = crate::config::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let (mode_text, mode_col) = if app.live {
        ("LIVE — real wallet & CLOB", LIVE)
    } else {
        ("PAPER — simulated account", PAPER)
    };
    let relaunch = if app.live {
        "Relaunch with `polymarket tui --paper` to simulate."
    } else {
        "Relaunch with `polymarket tui` (no --paper) for live trading."
    };
    let lines = vec![
        Line::from("Settings".bold()),
        Line::from(""),
        Line::from(vec![
            Span::styled(format!("{:<22}", "Mode"), Style::default().fg(DIM)),
            Span::styled(mode_text, Style::default().fg(mode_col).bold()),
        ]),
        kv_line(
            if app.live {
                "Cash (live)"
            } else {
                "Starting balance"
            },
            &money(initial),
        ),
        kv_line("Cash", &money(cash)),
        kv_line("Strategy tick", &format!("{}s", app.engine.tick_secs())),
        {
            let (ticks, last) = app.engine.runtime_stats();
            let last = last
                .map(|t| t.format("%H:%M:%S").to_string())
                .unwrap_or_else(|| "—".into());
            kv_line("Engine ticks", &format!("{ticks} (last {last})"))
        },
        Line::from(""),
        kv_line("Wallet config", &cfg),
        Line::from(""),
        Line::from(format!("Mode is set at launch and fixed for the session. {relaunch}").fg(DIM)),
        Line::from("Strategies & paper account live under ~/.config/polymarket/".fg(DIM)),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(panel("Settings"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

// --- Modal -----------------------------------------------------------------

fn render_modal(f: &mut Frame, app: &App, m: &OrderModal) {
    let area = centered_rect(58, 16, f.area());
    f.render_widget(Clear, area);
    let side = m.side.to_string();
    let venue = if app.live { "LIVE" } else { "PAPER" };
    let title = format!(" {side} ORDER · {venue} — {} ", truncate(&m.outcome, 18));

    let kind_line = Line::from(vec![
        Span::raw("Type: "),
        toggle_span("Market", m.kind == OrderKind::Market),
        Span::raw("  "),
        toggle_span("Limit", m.kind == OrderKind::Limit),
        Span::styled("   (m / L)", Style::default().fg(DIM)),
    ]);

    let mut lines = vec![Line::from(truncate(&m.question, 52).fg(ACCENT))];
    if app.live {
        lines.push(Line::from(Span::styled(
            "⚠ REAL FUNDS — this submits a signed order to the CLOB.",
            Style::default().fg(LIVE).bold(),
        )));
    }
    lines.push(Line::from(""));
    lines.push(kind_line);
    lines.push(Line::from(""));
    match m.kind {
        OrderKind::Market => {
            let label = if m.side == TradeSide::Buy {
                "Amount (pUSD)"
            } else {
                "Shares"
            };
            lines.push(field_line(label, &m.amount, m.field == ModalField::Amount));
        }
        OrderKind::Limit => {
            lines.push(field_line("Price", &m.price, m.field == ModalField::Price));
            lines.push(field_line(
                "Size (shares)",
                &m.size,
                m.field == ModalField::Size,
            ));
        }
    }
    lines.push(Line::from(""));
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(BAD),
        )));
    }
    lines.push(Line::from(
        "Tab next field · Enter submit · Esc cancel".fg(DIM),
    ));

    let border = if app.live {
        LIVE
    } else if m.side == TradeSide::Buy {
        GOOD
    } else {
        BAD
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(title.bold())
        .border_style(Style::default().fg(border));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_strat_modal(f: &mut Frame, m: &super::app::StratModal) {
    let area = centered_rect(60, 14, f.area());
    f.render_widget(Clear, area);
    let avail = crate::strategy::registry::available();
    let cur = avail.get(m.kind_idx);

    let mut kind_spans = vec![Span::raw("Strategy: ")];
    for (i, meta) in avail.iter().enumerate() {
        kind_spans.push(toggle_span(meta.kind, i == m.kind_idx));
        kind_spans.push(Span::raw(" "));
    }
    kind_spans.push(Span::styled("(←→)", Style::default().fg(DIM)));

    let summary = cur.map(|m| m.summary).unwrap_or("");
    let lines = vec![
        Line::from("Create a local strategy".bold()),
        Line::from(""),
        Line::from(kind_spans),
        Line::from(Span::styled(summary, Style::default().fg(DIM))),
        Line::from(""),
        field_line("Token IDs (csv)", &m.tokens, true),
        Line::from(""),
        match &m.error {
            Some(e) => Line::from(Span::styled(e.clone(), Style::default().fg(BAD))),
            None => Line::from("Tip: open a market and press g to grab its token ID.".fg(DIM)),
        },
        Line::from("←→ pick strategy · Enter create & start · Esc cancel".fg(DIM)),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" NEW STRATEGY ".bold())
        .border_style(Style::default().fg(ACCENT));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

// --- Shared widgets / helpers ---------------------------------------------

fn metric_card(f: &mut Frame, area: Rect, label: &str, value: &str, color: Color) {
    let lines = vec![
        Line::from(Span::styled(label.to_uppercase(), Style::default().fg(DIM))),
        Line::from(""),
        Line::from(Span::styled(
            value,
            Style::default().fg(color).add_modifier(Modifier::BOLD),
        )),
    ];
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center).block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(color)),
        ),
        area,
    );
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(PANEL))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(HEADER).bold(),
        ))
}

fn header_row(cells: &[&str]) -> Row<'static> {
    Row::new(
        cells
            .iter()
            .map(|c| {
                Cell::from((*c).to_uppercase())
                    .style(Style::default().fg(HEADER).add_modifier(Modifier::BOLD))
            })
            .collect::<Vec<_>>(),
    )
    .height(1)
    .bottom_margin(0)
}

fn highlight() -> Style {
    Style::default()
        .bg(SELECT_BG)
        .fg(Color::White)
        .add_modifier(Modifier::BOLD)
}

/// Subtle alternating-row background for readability.
fn zebra(i: usize) -> Style {
    if i.is_multiple_of(2) {
        Style::default().bg(ZEBRA_BG)
    } else {
        Style::default()
    }
}

fn sel_state(sel: usize, len: usize) -> TableState {
    let mut s = TableState::default();
    if len > 0 {
        s.select(Some(sel.min(len - 1)));
    }
    s
}

fn kv_line(key: &str, val: &str) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<22}"), Style::default().fg(DIM)),
        Span::raw(val.to_string()),
    ])
}

fn field_line(label: &str, value: &str, focused: bool) -> Line<'static> {
    let cursor = if focused { "_" } else { "" };
    let val_style = if focused {
        Style::default().fg(Color::Black).bg(ACCENT)
    } else {
        Style::default().fg(Color::White)
    };
    Line::from(vec![
        Span::styled(format!("{label:<16}"), Style::default().fg(DIM)),
        Span::styled(format!(" {value}{cursor} "), val_style),
    ])
}

fn toggle_span(label: &str, on: bool) -> Span<'static> {
    if on {
        Span::styled(
            format!("[{label}]"),
            Style::default().fg(Color::Black).bg(ACCENT).bold(),
        )
    } else {
        Span::styled(format!(" {label} "), Style::default().fg(DIM))
    }
}

fn side_cell(side: TradeSide) -> Cell<'static> {
    let color = match side {
        TradeSide::Buy => GOOD,
        TradeSide::Sell => BAD,
    };
    Cell::from(side.to_string()).style(Style::default().fg(color))
}

fn marks_snapshot(app: &App) -> BTreeMap<String, Decimal> {
    let d = app.data.lock().unwrap();
    d.marks.iter().map(|(k, v)| (k.clone(), *v)).collect()
}

fn daily_pnl(acct: &crate::paper::types::PaperAccount) -> Decimal {
    let today = chrono::Utc::now().date_naive();
    acct.trades
        .iter()
        .filter(|t| t.timestamp.date_naive() == today)
        .filter_map(|t| t.realized_pnl)
        .sum()
}

fn pnl_color(v: Decimal) -> Color {
    if v > Decimal::ZERO {
        GOOD
    } else if v < Decimal::ZERO {
        BAD
    } else {
        Color::White
    }
}

fn money(d: Decimal) -> String {
    format!("${:.2}", d.round_dp(2))
}

fn signed_money(d: Decimal) -> String {
    if d < Decimal::ZERO {
        format!("-${:.2}", (-d).round_dp(2))
    } else {
        format!("${:.2}", d.round_dp(2))
    }
}

/// Compact money with K/M/B suffixes, e.g. `$2.9M` (reuses the CLI formatter).
fn short_money(d: Option<Decimal>) -> String {
    d.map(crate::output::format_decimal)
        .unwrap_or_else(|| "—".into())
}

/// A probability (0..1) as a percentage, e.g. `0.004 → 0.4%`, `0.612 → 61.2%`.
fn pct(p: Decimal) -> String {
    format!("{:.1}%", p * Decimal::from(100))
}

fn status_label(closed: Option<bool>, active: Option<bool>) -> String {
    crate::output::active_status(closed, active).to_string()
}

fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() <= max {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(max.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

fn centered_rect(width: u16, height: u16, area: Rect) -> Rect {
    let w = width.min(area.width);
    let h = height.min(area.height);
    let x = area.x + (area.width.saturating_sub(w)) / 2;
    let y = area.y + (area.height.saturating_sub(h)) / 2;
    Rect::new(x, y, w, h)
}
