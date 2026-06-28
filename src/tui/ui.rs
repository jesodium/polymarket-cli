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

use super::app::{
    App, COPY_FIELDS, CopyField, CopyModal, ModalField, OnboardingState, OnboardingStep,
    OrderModal, ResetModal, SETTING_ROWS, SettingRow, SettingsEditModal, View, WalletAction,
    WalletActionModal,
};
use super::data::ResolutionInfo;
use crate::paper::engine;
use crate::paper::types::{OrderKind, PositionView, Trade, TradeSide};

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
    // Onboarding takes over the full screen when no wallet is configured.
    if let Some(o) = &app.onboarding {
        render_onboarding(f, o);
        return;
    }
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
        View::Onboarding => {} // handled by early return above
        View::Dashboard => dashboard(f, app, chunks[1]),
        View::Markets => markets(f, app, chunks[1]),
        View::MarketDetail => market_detail(f, app, chunks[1]),
        View::Portfolio => portfolio(f, app, chunks[1]),
        View::Positions => positions(f, app, chunks[1]),
        View::Orders => orders(f, app, chunks[1]),
        View::History => history(f, app, chunks[1]),
        View::Copytrade => copytrade(f, app, chunks[1]),
        View::Logs => logs(f, app, chunks[1]),
        View::Settings => settings(f, app, chunks[1]),
    }
    render_status(f, app, chunks[2]);

    if let Some(modal) = &app.modal {
        render_modal(f, app, modal);
    }
    if let Some(cm) = &app.copy_modal {
        render_copy_modal(f, cm);
    }
    if let Some(rm) = &app.reset_modal {
        render_reset_modal(f, rm);
    }
    if let Some(sem) = &app.settings_modal {
        render_settings_modal(f, sem);
    }
    if let Some(wam) = &app.wallet_action_modal {
        render_wallet_action_modal(f, wam);
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
        View::MarketDetail => "←→ outcome · ↑↓ scroll · b buy · s sell · Esc back",
        View::Positions => "↑↓ move · b buy · s sell · r redeem resolved · Tab views",
        View::Orders => "↑↓ move · c cancel · Tab views",
        View::Copytrade => {
            "n follow · s start · x stop · e enable · d disable · D unfollow · ↑↓ move"
        }
        View::Settings => {
            "↑↓ move · Enter edit/cycle · w reveal key · n create · m import · o browser · a approve · c check · d deposit · r reset paper · Tab views"
        }
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
    let loading = data_loading(app);
    let acct = app.account.lock().unwrap();
    let view = engine::portfolio_view(&acct, &marks);
    let daily = daily_pnl(&acct);
    let stats = trade_stats(&acct);
    let equity_stats = equity_metrics(&acct.equity_curve);
    let recent: Vec<_> = acct.trades.iter().rev().take(8).cloned().collect();
    let positions = acct.positions.len();
    let open_orders = acct.open_orders.len();
    drop(acct);
    let following = app.copy_engine.running_count();

    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(7), Constraint::Min(5)])
        .split(area);

    // Metric cards.
    let cards = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Ratio(1, 4); 4])
        .split(rows[0]);

    metric_card(
        f,
        cards[0],
        "Portfolio Value",
        &loading_money(view.equity, loading),
        if loading { DIM } else { ACCENT },
    );
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
        &loading_signed(total, loading),
        if loading { DIM } else { pnl_color(total) },
    );

    // Bottom: counters + recent trades.
    let bottom = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(34), Constraint::Percentage(66)])
        .split(rows[1]);

    let mut info = vec![
        kv_line("Open Positions", &positions.to_string()),
        kv_line("Open Orders", &open_orders.to_string()),
        kv_line("Copy Followers", &following.to_string()),
        kv_line(
            "ROI",
            &if loading {
                LOADING.into()
            } else {
                format!("{}%", view.roi_pct)
            },
        ),
        kv_line("Realized PnL", &signed_money(view.realized_pnl)),
        kv_line(
            "Unrealized PnL",
            &loading_signed(view.unrealized_pnl, loading),
        ),
        kv_line(
            "Win Rate",
            &format!(
                "{}% ({}W {}L)",
                stats.win_rate.round_dp(1),
                stats.wins,
                stats.losses
            ),
        ),
        kv_line("Avg Win", &signed_money(stats.avg_win)),
        kv_line("Avg Loss", &signed_money(stats.avg_loss)),
        kv_line(
            "Profit Factor",
            &match stats.profit_factor {
                Some(pf) => pf.round_dp(2).to_string(),
                None => "∞".into(),
            },
        ),
        kv_line("Expectancy", &signed_money(stats.expectancy)),
    ];
    // Only shown once equity snapshots have accumulated (hidden for accounts
    // predating equity snapshotting).
    if let Some(eq) = equity_stats {
        info.push(kv_line(
            "Max Drawdown",
            &format!("{}%", eq.max_drawdown_pct),
        ));
        if let Some(sh) = eq.sharpe {
            info.push(kv_line("Sharpe", &sh.to_string()));
        }
    }
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
    lines.push(Line::from("Press b to buy, s to sell.".fg(DIM)));

    if let Some(end) = d.end_date {
        lines.push(Line::from(vec![
            Span::styled("Closes     ", Style::default().fg(DIM)),
            Span::raw(end.format("%Y-%m-%d %H:%M UTC").to_string()),
        ]));
    }
    if let Some(src) = d.resolution_source.as_deref().filter(|s| !s.is_empty()) {
        lines.push(Line::from(vec![
            Span::styled("Resolver   ", Style::default().fg(DIM)),
            Span::raw(src.to_string()),
        ]));
    }
    if let Some(rules) = d.description.as_deref().filter(|s| !s.is_empty()) {
        lines.push(Line::from(""));
        lines.push(Line::from("Resolution rules:".fg(ACCENT)));
        for para in rules.lines() {
            lines.push(Line::from(para.to_string()));
        }
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(panel("Market Details — ↑↓ scroll"))
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
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
            (Some(bid), Some(ask)) => format!("{:.3}", ask - bid),
            _ => "—".into(),
        };
        rows.push(Row::new(vec![
            Cell::from("spread").style(Style::default().fg(ACCENT)),
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
    let loading = data_loading(app);
    let acct = app.account.lock().unwrap();
    let view = engine::portfolio_view(&acct, &marks);
    drop(acct);

    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(8), Constraint::Min(5)])
        .split(area);

    let summary = vec![
        kv_line("Equity", &loading_money(view.equity, loading)),
        kv_line("Cash", &money(view.cash)),
        kv_line("Reserved (open buys)", &money(view.reserved_cash)),
        kv_line(
            "Positions Value",
            &loading_money(view.positions_value, loading),
        ),
        kv_line("Realized PnL", &signed_money(view.realized_pnl)),
        kv_line(
            "Unrealized PnL",
            &loading_signed(view.unrealized_pnl, loading),
        ),
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
            let ph = if loading { LOADING } else { "—" };
            let (mark_cell, value_cell, upnl_cell) = match p.mark_price {
                Some(mark) => {
                    let upnl = p.unrealized_pnl.unwrap_or_default();
                    (
                        Cell::from(format!("{mark:.3}")),
                        Cell::from(p.market_value.map(money).unwrap_or_else(|| ph.into())),
                        Cell::from(signed_money(upnl)).style(Style::default().fg(pnl_color(upnl))),
                    )
                }
                None => {
                    let dim = Style::default().fg(DIM);
                    (
                        Cell::from(ph).style(dim),
                        Cell::from(ph).style(dim),
                        Cell::from(ph).style(dim),
                    )
                }
            };
            Row::new(vec![
                Cell::from(truncate(&p.position.question, 34)),
                Cell::from(truncate(&p.position.outcome, 10)),
                Cell::from(p.position.size.round_dp(1).to_string()),
                Cell::from(format!("{:.3}", p.position.avg_price)),
                mark_cell,
                value_cell,
                upnl_cell,
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
    let loading = data_loading(app);
    let resolutions = app.data.lock().unwrap().resolutions.clone();
    let acct = app.account.lock().unwrap();
    let view = engine::portfolio_view(&acct, &marks);
    drop(acct);

    // Open positions render first; resolved ones drop into a -REDEEMABLE-
    // section at the bottom. The cursor (App::selected_position) walks this
    // same order, so keep the two in lockstep.
    let (open, resolved): (Vec<&PositionView>, Vec<&PositionView>) = view
        .positions
        .iter()
        .partition(|p| !resolutions.contains_key(&p.position.token_id));

    let mut rows: Vec<Row> = Vec::with_capacity(view.positions.len() + 1);
    let mut zi = 0usize; // zebra index, continuous across the real rows
    for p in &open {
        rows.push(open_position_row(p, zi, loading));
        zi += 1;
    }
    if !resolved.is_empty() {
        rows.push(redeemable_header_row());
        for p in &resolved {
            let info = &resolutions[&p.position.token_id];
            rows.push(redeemable_row(p, info, zi));
            zi += 1;
        }
    }

    let n = view.positions.len();
    let table = Table::new(
        rows,
        [
            Constraint::Min(14),
            Constraint::Length(5),
            Constraint::Length(17),
            Constraint::Length(13),
            Constraint::Length(13),
            Constraint::Length(9),
            Constraint::Length(7),
        ],
    )
    .header(header_row(&[
        "Market",
        "Out",
        "Shares (value)",
        "Avg (prob)",
        "Mark (prob)",
        "uPnL",
        "ROI",
    ]))
    .block(panel(&format!(
        "Positions ({n}) — b buy · s sell · r redeem"
    )))
    .row_highlight_style(highlight())
    .highlight_symbol("▶ ");

    // The -REDEEMABLE- header is a real table row, so any cursor landing in
    // the resolved group sits one row lower than its selection index.
    let table_len = n + usize::from(!resolved.is_empty());
    let table_sel = if app.positions_sel >= open.len() {
        app.positions_sel + 1
    } else {
        app.positions_sel
    };
    f.render_stateful_widget(table, area, &mut sel_state(table_sel, table_len));
}

/// A live (unresolved) position row: mark/value/uPnL/ROI from the quote feed.
/// Until a mark exists, the quote-derived cells show "loading…" (during the
/// first refresh) or "—", never a misleading $0.00 uPnL.
fn open_position_row(p: &PositionView, zi: usize, loading: bool) -> Row<'static> {
    let placeholder = if loading { LOADING } else { "—" };
    let value_str = p
        .market_value
        .map(money)
        .unwrap_or_else(|| placeholder.to_string());

    let (mark_cell, upnl_cell, roi_cell) = match p.mark_price {
        Some(mark) => {
            let upnl = p.unrealized_pnl.unwrap_or_default();
            let roi_cell = match p.roi() {
                Some(r) => Cell::from(format!("{:+.1}%", r * Decimal::ONE_HUNDRED))
                    .style(Style::default().fg(pnl_color(r))),
                None => Cell::from("—"),
            };
            (
                Cell::from(price_pct(mark)),
                Cell::from(signed_money(upnl)).style(Style::default().fg(pnl_color(upnl))),
                roi_cell,
            )
        }
        None => {
            let dim = Style::default().fg(DIM);
            (
                Cell::from(placeholder).style(dim),
                Cell::from(placeholder).style(dim),
                Cell::from(placeholder).style(dim),
            )
        }
    };

    Row::new(vec![
        Cell::from(truncate(&p.position.question, 36)),
        Cell::from(truncate(&p.position.outcome, 8)),
        Cell::from(format!("{} ({})", p.position.size.round_dp(1), value_str)),
        Cell::from(price_pct(p.position.avg_price)),
        mark_cell,
        upnl_cell,
        roi_cell,
    ])
    .style(zebra(zi))
}

/// Section divider between live positions and resolved (redeemable) ones.
fn redeemable_header_row() -> Row<'static> {
    let mut cells = vec![Cell::from("── REDEEMABLE ──").style(Style::default().fg(GOLD).bold())];
    cells.extend(std::iter::repeat_with(|| Cell::from("")).take(6));
    Row::new(cells).style(Style::default().bg(ZEBRA_BG))
}

/// A resolved position row. The market settled, so mark/value/uPnL/ROI come
/// from the payout (1 won, 0 lost) instead of a live quote, and the WON/LOST
/// verdict leads the Market column.
fn redeemable_row(p: &PositionView, info: &ResolutionInfo, zi: usize) -> Row<'static> {
    let size = p.position.size;
    // Basis is actual cost (avg fill), matching the settlement realized PnL.
    let entry = p.position.avg_price;
    let payout = info.payout;
    let value = payout * size;
    let upnl = (payout - entry) * size;
    let basis = entry * size;

    let (tag, tag_color) = if info.won {
        ("WON ", GOOD)
    } else {
        ("LOST ", BAD)
    };
    let market_cell = Cell::from(Line::from(vec![
        Span::styled(tag, Style::default().fg(tag_color).bold()),
        Span::raw(truncate(&p.position.question, 30)),
    ]));
    let roi_cell = if basis > Decimal::ZERO {
        let r = upnl / basis;
        Cell::from(format!("{:+.1}%", r * Decimal::ONE_HUNDRED))
            .style(Style::default().fg(pnl_color(r)))
    } else {
        Cell::from("—")
    };
    Row::new(vec![
        market_cell,
        Cell::from(truncate(&p.position.outcome, 8)),
        Cell::from(format!("{} ({})", size.round_dp(1), money(value))),
        Cell::from(price_pct(p.position.avg_price)),
        Cell::from(price_pct(payout)),
        Cell::from(signed_money(upnl)).style(Style::default().fg(pnl_color(upnl))),
        roi_cell,
    ])
    .style(zebra(zi))
}

// --- Orders ----------------------------------------------------------------

fn orders(f: &mut Frame, app: &App, area: Rect) {
    if app.live {
        live_orders(f, app, area);
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

/// Live open orders at the CLOB, refreshed on the slow cadence.
fn live_orders(f: &mut Frame, app: &App, area: Rect) {
    let orders = app.data.lock().unwrap().live_orders.clone();
    if orders.is_empty() {
        f.render_widget(
            Paragraph::new(vec![
                Line::from("No open orders at the CLOB.".fg(DIM)),
                Line::from(""),
                Line::from(
                    "Orders refresh about every 30s; place one with b/s on a market.".fg(DIM),
                ),
            ])
            .block(panel("Open Orders · LIVE")),
            area,
        );
        return;
    }
    let rows: Vec<Row> = orders
        .iter()
        .enumerate()
        .map(|(i, o)| {
            let side_color = if o.side.eq_ignore_ascii_case("buy") {
                GOOD
            } else {
                BAD
            };
            Row::new(vec![
                Cell::from(truncate(&o.id, 12)),
                Cell::from(o.side.clone()).style(Style::default().fg(side_color)),
                Cell::from(truncate(&o.outcome, 12)),
                Cell::from(o.price.clone()),
                Cell::from(o.size.clone()),
                Cell::from(o.matched.clone()),
                Cell::from(o.created_at.clone()),
            ])
            .style(zebra(i))
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(13),
            Constraint::Length(5),
            Constraint::Length(12),
            Constraint::Length(8),
            Constraint::Length(9),
            Constraint::Length(9),
            Constraint::Length(12),
        ],
    )
    .header(header_row(&[
        "ID", "Side", "Outcome", "Price", "Size", "Matched", "Created",
    ]))
    .block(panel(&format!(
        "Open Orders · LIVE ({}) — c to cancel",
        orders.len()
    )))
    .row_highlight_style(highlight())
    .highlight_symbol("▶ ");
    f.render_stateful_widget(table, area, &mut sel_state(app.orders_sel, orders.len()));
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
            .block(panel("Position History · LIVE")),
            area,
        );
        return;
    }
    let acct = app.account.lock().unwrap();
    // Two views of the same trade log: every fill (the order log) and the
    // subset that closed a position (carries a realized PnL).
    let orders: Vec<_> = acct.trades.iter().rev().cloned().collect();
    let closed: Vec<_> = acct
        .trades
        .iter()
        .rev()
        .filter(|t| t.realized_pnl.is_some())
        .cloned()
        .collect();
    drop(acct);

    // Stack the order log on top of the closed-position history.
    let halves = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(area);

    orders_table(f, &orders, app.history_scroll, halves[0]);
    positions_table(f, &closed, app.history_scroll, halves[1]);
}

/// `size (value)` — share count with its cash value in parens.
fn size_value(size: Decimal, notional: Decimal) -> String {
    format!("{} ({})", size.round_dp(1), money(notional))
}

/// Every fill, buy or sell, newest first.
fn orders_table(f: &mut Frame, orders: &[Trade], scroll: usize, area: Rect) {
    let total = orders.len();
    if total == 0 {
        f.render_widget(
            Paragraph::new("No orders yet — fills show here.".fg(DIM)).block(panel("Orders")),
            area,
        );
        return;
    }
    let visible = area.height.saturating_sub(3) as usize;
    let start = scroll.min(total.saturating_sub(1));
    let rows: Vec<Row> = orders
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(i, t)| {
            let (side, scolor) = match t.side {
                TradeSide::Buy => ("BUY", GOOD),
                TradeSide::Sell => ("SELL", BAD),
            };
            Row::new(vec![
                Cell::from(t.timestamp.format("%m-%d %H:%M").to_string()),
                Cell::from(side).style(Style::default().fg(scolor).bold()),
                Cell::from(truncate(&t.question, 30)),
                Cell::from(truncate(&t.outcome, 6)),
                Cell::from(size_value(t.size, t.notional)),
                Cell::from(format!("{:.2}", t.price)),
                Cell::from(t.kind.to_string()).style(Style::default().fg(DIM)),
            ])
            .style(zebra(i))
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Length(5),
            Constraint::Min(18),
            Constraint::Length(6),
            Constraint::Length(16),
            Constraint::Length(6),
            Constraint::Length(8),
        ],
    )
    .header(header_row(&[
        "Time",
        "Side",
        "Market",
        "Out",
        "Size (Value)",
        "Price",
        "Kind",
    ]))
    .block(panel(&format!("Orders ({total}) — ↑↓ scroll")));
    f.render_widget(table, area);
}

/// Closed positions — every sell carries a realized PnL, whether it closed by
/// resolution (Settlement) or an early sale.
fn positions_table(f: &mut Frame, closed: &[Trade], scroll: usize, area: Rect) {
    let total = closed.len();
    if total == 0 {
        f.render_widget(
            Paragraph::new(
                "No closed positions yet — resolved or sold positions show here.".fg(DIM),
            )
            .block(panel("Positions")),
            area,
        );
        return;
    }
    let visible = area.height.saturating_sub(3) as usize;
    let start = scroll.min(total.saturating_sub(1));
    let rows: Vec<Row> = closed
        .iter()
        .skip(start)
        .take(visible)
        .enumerate()
        .map(|(i, t)| {
            let pnl_val = t.realized_pnl.unwrap_or_default();
            // Resolved markets read WON/LOST by payout; early sales read SOLD,
            // coloured green for profit and red for loss.
            let (verdict, vcolor) = if t.kind == OrderKind::Settlement {
                if pnl_val >= Decimal::ZERO {
                    ("WON", GOOD)
                } else {
                    ("LOST", BAD)
                }
            } else if pnl_val > Decimal::ZERO {
                ("SOLD", GOOD)
            } else if pnl_val < Decimal::ZERO {
                ("SOLD", BAD)
            } else {
                ("SOLD", DIM)
            };
            // Cost basis = exit notional minus realized PnL; entry = basis/size.
            let basis = t.notional - pnl_val;
            let entry_cell = if t.size > Decimal::ZERO {
                Cell::from(format!("{:.2}", basis / t.size))
            } else {
                Cell::from("—")
            };
            let roi_cell = if basis > Decimal::ZERO {
                let r = pnl_val / basis * Decimal::ONE_HUNDRED;
                Cell::from(format!("{r:+.1}%")).style(Style::default().fg(pnl_color(pnl_val)))
            } else {
                Cell::from("—")
            };
            Row::new(vec![
                Cell::from(t.timestamp.format("%m-%d %H:%M").to_string()),
                Cell::from(verdict).style(Style::default().fg(vcolor).bold()),
                Cell::from(truncate(&t.question, 30)),
                Cell::from(truncate(&t.outcome, 6)),
                Cell::from(size_value(t.size, t.notional)),
                entry_cell,
                Cell::from(format!("{:.2}", t.price)),
                Cell::from(signed_money(pnl_val)).style(Style::default().fg(pnl_color(pnl_val))),
                roi_cell,
            ])
            .style(zebra(i))
        })
        .collect();
    let table = Table::new(
        rows,
        [
            Constraint::Length(11),
            Constraint::Length(6),
            Constraint::Min(18),
            Constraint::Length(6),
            Constraint::Length(16),
            Constraint::Length(6),
            Constraint::Length(6),
            Constraint::Length(10),
            Constraint::Length(9),
        ],
    )
    .header(header_row(&[
        "Time",
        "Result",
        "Market",
        "Out",
        "Size (Value)",
        "Entry",
        "Exit",
        "PnL",
        "ROI",
    ]))
    .block(panel(&format!("Positions ({total}) — ↑↓ scroll")));
    f.render_widget(table, area);
}

// --- Copytrade -------------------------------------------------------------

fn copytrade(f: &mut Frame, app: &App, area: Rect) {
    let snap = app.copy_engine.snapshot();
    let layout = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(6), Constraint::Length(8)])
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
                Cell::from(truncate(&s.nickname, 16)),
                Cell::from(short_wallet(&s.wallet)),
                Cell::from(state).style(Style::default().fg(color)),
                Cell::from(s.copied.to_string()),
                Cell::from(s.skipped.to_string()),
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
            Constraint::Length(14),
            Constraint::Length(16),
            Constraint::Length(13),
            Constraint::Length(11),
            Constraint::Length(7),
            Constraint::Length(8),
            Constraint::Length(7),
            Constraint::Min(14),
        ],
    )
    .header(header_row(&[
        "ID",
        "Nickname",
        "Wallet",
        "State",
        "Copied",
        "Skipped",
        "Errors",
        "Last Action",
    ]))
    .block(panel(&format!(
        "Copy Trading — {} mode, {}s poll (n follow · s start · x stop · e enable · d disable · D unfollow)",
        app.copy_engine.mode(),
        app.copy_engine.interval()
    )))
    .row_highlight_style(highlight())
    .highlight_symbol("▶ ");
    f.render_stateful_widget(
        table,
        layout[0],
        &mut sel_state(app.copytrade_sel, snap.len()),
    );

    // Recent copy-trading activity (this tab has no separate logs view).
    let logs = app.copy_engine.recent_logs(6);
    let items: Vec<ListItem> = if snap.is_empty() {
        vec![ListItem::new(
            "Not following anyone yet. Press n to follow a wallet's trades.".fg(DIM),
        )]
    } else {
        logs.iter()
            .map(|l| {
                use crate::copytrade::engine::LogLevel;
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
            .collect()
    };
    f.render_widget(
        List::new(items).block(panel("Recent Copy Activity")),
        layout[1],
    );
}

/// `0x1234…cdef` short form for the wallet column.
fn short_wallet(wallet: &str) -> String {
    let w = wallet.trim();
    if w.len() <= 12 {
        return w.to_string();
    }
    format!("{}…{}", &w[..6], &w[w.len() - 4..])
}

// --- Logs ------------------------------------------------------------------

fn logs(f: &mut Frame, app: &App, area: Rect) {
    let lines = app.copy_engine.recent_logs(500);
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
            use crate::copytrade::engine::LogLevel;
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
    let list = List::new(items).block(panel("Copy-Trading Logs (↑↓ scroll)"));
    f.render_widget(list, area);
}

// --- Settings --------------------------------------------------------------

fn settings(f: &mut Frame, app: &App, area: Rect) {
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
        .split(area);

    render_trading_settings(f, app, cols[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(8),
            Constraint::Length(7),
            Constraint::Length(9),
        ])
        .split(cols[1]);
    render_wallet_panel(f, app, right[0]);
    render_session_panel(f, app, right[1]);
    render_mcp_panel(f, right[2]);
}

/// Right-bottom panel: MCP server status, so you can see whether an AI client
/// is connected. The server runs as a separate process spawned by the client;
/// this reads the status file it maintains.
fn render_mcp_panel(f: &mut Frame, area: Rect) {
    let status = crate::mcp::status::load();
    let mut lines: Vec<Line> = Vec::new();

    let (label, color) = match &status {
        Some(s) if s.is_recent() => ("● Connected", GOOD),
        Some(s) if s.state == "stopped" => ("○ Stopped", DIM),
        Some(_) => ("◌ Idle", GOLD),
        None => ("○ Not running", DIM),
    };
    lines.push(Line::from(vec![
        Span::styled(format!("{:<22}", "Status"), Style::default().fg(DIM)),
        Span::styled(label, Style::default().fg(color).bold()),
    ]));

    match &status {
        Some(s) => {
            let client = match (&s.client_name, &s.client_version) {
                (Some(n), Some(v)) => format!("{n} v{v}"),
                (Some(n), None) => n.clone(),
                _ => "—".to_string(),
            };
            lines.push(kv_line("Client", &client));
            let last = match (&s.last_tool, s.tool_calls) {
                (Some(t), n) => format!("{n} (last: {t})"),
                (None, n) => n.to_string(),
            };
            lines.push(kv_line("Tool calls", &last));
            lines.push(kv_line("Last activity", &rel_time(s.last_activity)));
        }
        None => {
            lines.push(Line::from(
                "No MCP session yet. Register the server with your AI client:".fg(DIM),
            ));
            lines.push(Line::from(Span::styled(
                r#"  "command": "polymarket", "args": ["mcp"]"#,
                Style::default().fg(ACCENT),
            )));
        }
    }

    f.render_widget(
        Paragraph::new(lines)
            .block(panel("MCP Server"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// Human "x ago" for an optional timestamp.
fn rel_time(t: Option<chrono::DateTime<chrono::Utc>>) -> String {
    let Some(t) = t else { return "—".to_string() };
    let secs = (chrono::Utc::now() - t).num_seconds().max(0);
    if secs < 60 {
        format!("{secs}s ago")
    } else if secs < 3600 {
        format!("{}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("{}h ago", secs / 3600)
    } else {
        format!("{}d ago", secs / 86_400)
    }
}

/// Left panel: the editable trading settings.
fn render_trading_settings(f: &mut Frame, app: &App, area: Rect) {
    let s = &app.settings;
    let mode_value = format!("{} — {}", s.trading_mode, s.trading_mode.describe());
    let rows: Vec<Row> = SETTING_ROWS
        .iter()
        .enumerate()
        .map(|(i, row)| {
            let (label, value) = match row {
                SettingRow::Mode => ("Trading mode", mode_value.clone()),
                SettingRow::AutoSettle => (
                    "Resolved markets",
                    if s.auto_settle {
                        "Auto-settle to cash".to_string()
                    } else {
                        "Manual claim — r on Positions".to_string()
                    },
                ),
                SettingRow::Field(field) => {
                    let v = app.setting_current_value(*field);
                    let v = if v.is_empty() { "off".to_string() } else { v };
                    (field.label(), v)
                }
            };
            Row::new(vec![Cell::from(label), Cell::from(value)]).style(zebra(i))
        })
        .collect();
    let table = Table::new(rows, [Constraint::Length(34), Constraint::Min(20)])
        .header(header_row(&["Setting", "Value"]))
        .block(panel("Trading Settings — Enter to edit / cycle"))
        .row_highlight_style(highlight())
        .highlight_symbol("▶ ");
    f.render_stateful_widget(
        table,
        area,
        &mut sel_state(app.settings_sel, SETTING_ROWS.len()),
    );
}

/// Right-top panel: the wallet behind live mode (or paper-account info).
fn render_wallet_panel(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    match &app.wallet {
        Some(w) => {
            lines.push(kv_line("Signer (EOA)", &w.eoa));
            match &w.proxy {
                Some(p) => lines.push(kv_line("Proxy wallet", p)),
                None => lines.push(kv_line("Proxy wallet", "—")),
            }
            lines.push(kv_line("Trading as", &w.trading));
            lines.push(kv_line("Signature type", &w.signature_type));
            {
                let cash = app.account.lock().unwrap().cash;
                lines.push(kv_line("Balance (pUSD)", &money(cash)));
            }
            lines.push(kv_line("Config file", &w.config_path));
            lines.push(Line::from(""));
            if app.reveal_key {
                lines.push(Line::from(Span::styled(
                    "⚠ PRIVATE KEY — press w to hide:",
                    Style::default().fg(LIVE).bold(),
                )));
                lines.push(Line::from(Span::styled(
                    w.private_key.clone().unwrap_or_default(),
                    Style::default().fg(GOLD),
                )));
            } else {
                lines.push(Line::from(Span::styled(
                    "Press w to reveal/export the private key.",
                    Style::default().fg(DIM),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "n — Create new wallet",
                Style::default().fg(GOLD),
            )));
            lines.push(Line::from(Span::styled(
                "m — Import wallet",
                Style::default().fg(GOLD),
            )));
            lines.push(Line::from(Span::styled(
                "o — Open profile in browser",
                Style::default().fg(ACCENT),
            )));
            lines.push(Line::from(Span::styled(
                "a — Approve all contracts",
                Style::default().fg(GOOD),
            )));
            lines.push(Line::from(Span::styled(
                "c — Check approval status",
                Style::default().fg(DIM),
            )));
            lines.push(Line::from(Span::styled(
                "d — Show bridge deposit address",
                Style::default().fg(DIM),
            )));
        }
        None => {
            let acct = app.account.lock().unwrap();
            let initial = acct.initial_balance;
            let cash = acct.cash;
            drop(acct);
            lines.push(kv_line("Starting balance", &money(initial)));
            lines.push(kv_line("Cash", &money(cash)));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Press r to reset the paper account.",
                Style::default().fg(GOLD),
            )));
            if app.live {
                lines.push(Line::from(Span::styled(
                    "No wallet configured.",
                    Style::default().fg(DIM),
                )));
                lines.push(Line::from(Span::styled(
                    "n — Create new wallet",
                    Style::default().fg(GOLD),
                )));
                lines.push(Line::from(Span::styled(
                    "m — Import wallet",
                    Style::default().fg(GOLD),
                )));
            } else {
                lines.push(Line::from(
                    "Run `polymarket wallet create` then relaunch without --paper for live mode."
                        .fg(DIM),
                ));
            }
        }
    }
    let title = if app.live {
        "Wallet · LIVE"
    } else {
        "Paper Account"
    };
    f.render_widget(
        Paragraph::new(lines)
            .block(panel(title))
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// Right-bottom panel: session/engine facts.
fn render_session_panel(f: &mut Frame, app: &App, area: Rect) {
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
    let settings_file = crate::settings::config_path()
        .map(|p| p.display().to_string())
        .unwrap_or_default();
    let lines = vec![
        Line::from(vec![
            Span::styled(format!("{:<22}", "Mode"), Style::default().fg(DIM)),
            Span::styled(mode_text, Style::default().fg(mode_col).bold()),
        ]),
        kv_line("Copy poll", &format!("{}s", app.copy_engine.interval())),
        kv_line(
            "Copy followers",
            &app.copy_engine.running_count().to_string(),
        ),
        kv_line("Settings file", &settings_file),
        Line::from(format!("Mode fixed for the session. {relaunch}").fg(DIM)),
    ];
    f.render_widget(
        Paragraph::new(lines)
            .block(panel("Session"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

// --- Onboarding ------------------------------------------------------------

fn render_onboarding(f: &mut Frame, state: &OnboardingState) {
    let area = f.area();
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(Style::default().fg(ACCENT))
        .title(Span::styled(
            " POLYMARKET LIVE TRADING ",
            Style::default().fg(ACCENT).bold(),
        ));
    let inner = block.inner(area);
    f.render_widget(Clear, area);
    f.render_widget(block, area);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(6),
            Constraint::Length(3),
            Constraint::Min(8),
        ])
        .split(inner);

    let welcome = vec![
        Line::from("Welcome to Polymarket CLI".bold()),
        Line::from(""),
        Line::from("No wallet configured — set one up to trade live.".fg(DIM)),
    ];
    f.render_widget(
        Paragraph::new(welcome).alignment(Alignment::Center),
        chunks[0],
    );

    match state.step {
        OnboardingStep::Welcome => {
            let options = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  [c]  Create a new wallet    ",
                    Style::default().fg(GOOD).bold(),
                )),
                Line::from(Span::styled(
                    "  [i]  Import existing key    ",
                    Style::default().fg(ACCENT).bold(),
                )),
                Line::from(""),
                Line::from(Span::styled(
                    "  [Esc]  Skip — browse markets only",
                    Style::default().fg(DIM),
                )),
                Line::from(""),
            ];
            f.render_widget(
                Paragraph::new(options).alignment(Alignment::Center),
                chunks[1],
            );

            let tip = vec![Line::from(
                "You can also press Tab/9 to reach Settings and set up a wallet later.".fg(DIM),
            )];
            f.render_widget(Paragraph::new(tip).alignment(Alignment::Center), chunks[2]);
        }
        OnboardingStep::ImportKey => {
            let mut lines = vec![
                Line::from(""),
                Line::from(Span::styled(
                    "  Paste your private key (hex, with or without 0x prefix):",
                    Style::default().fg(DIM),
                )),
                Line::from(""),
                Line::from(format!("  {}█", state.import_key)),
                Line::from(""),
            ];
            if let Some(e) = &state.error {
                lines.push(Line::from(Span::styled(
                    format!("  ✗ {e}"),
                    Style::default().fg(BAD),
                )));
                lines.push(Line::from(""));
            }
            lines.push(Line::from("  Enter to import · Esc to go back".fg(DIM)));
            f.render_widget(
                Paragraph::new(lines).alignment(Alignment::Center),
                chunks[1],
            );
        }
    }
}

// --- Wallet action modal (Settings tab) ----------------------------------

fn render_wallet_action_modal(f: &mut Frame, m: &WalletActionModal) {
    let area = centered_rect(56, 14, f.area());
    f.render_widget(Clear, area);
    match m.action {
        WalletAction::Create => {
            let mut lines = vec![Line::from("Create new wallet".bold()), Line::from("")];
            if m.confirmed {
                lines.push(Line::from("Generating wallet…".fg(DIM)));
            } else {
                lines.push(Line::from("This will REPLACE your current wallet.".fg(BAD)));
                lines.push(Line::from(
                    "Make sure you have backed up your existing key.".fg(DIM),
                ));
                lines.push(Line::from(""));
                lines.push(Line::from("Press Enter to confirm · Esc to cancel".fg(DIM)));
            }
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" NEW WALLET ".bold())
                .border_style(Style::default().fg(GOLD));
            f.render_widget(
                Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
                area,
            );
        }
        WalletAction::Import => {
            let mut lines = vec![
                Line::from("Import wallet".bold()),
                Line::from(""),
                Line::from(Span::styled("Private key:", Style::default().fg(DIM))),
                Line::from(format!("{}█", m.import_key)),
                Line::from(""),
            ];
            if let Some(e) = &m.error {
                lines.push(Line::from(Span::styled(
                    e.clone(),
                    Style::default().fg(BAD),
                )));
                lines.push(Line::from(""));
            }
            lines.push(Line::from("Enter to import · Esc to cancel".fg(DIM)));
            let block = Block::default()
                .borders(Borders::ALL)
                .title(" IMPORT WALLET ".bold())
                .border_style(Style::default().fg(ACCENT));
            f.render_widget(
                Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
                area,
            );
        }
    }
}

// --- Modal -----------------------------------------------------------------

fn render_modal(f: &mut Frame, app: &App, m: &OrderModal) {
    let area = centered_rect(60, 22, f.area());
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
        // The order form never carries a settlement.
        OrderKind::Settlement => {}
    }
    // Take-profit / stop-loss guard fields on buys (auto-attached on fill).
    if m.side == TradeSide::Buy {
        lines.push(field_line(
            "Take-profit %",
            &m.tp,
            m.field == ModalField::TakeProfit,
        ));
        lines.push(field_line(
            "Stop-loss %",
            &m.sl,
            m.field == ModalField::StopLoss,
        ));
    }
    // Preset hint: one-tap quickbuy $ / quicksell % from Settings.
    let preset_hint = match m.side {
        TradeSide::Buy => format!(
            "p quick-fill: {}",
            crate::settings::fmt_money_list(&app.settings.quickbuy_presets)
        ),
        TradeSide::Sell => format!(
            "p quick-fill: {} of {} held",
            crate::settings::fmt_pct_list(&app.settings.quicksell_presets),
            m.held.round_dp(2)
        ),
    };
    lines.push(Line::from(Span::styled(
        preset_hint,
        Style::default().fg(DIM),
    )));
    lines.push(Line::from(Span::styled(
        format!(
            "slippage {}% · mode {}",
            app.settings.slippage_pct.normalize(),
            app.settings.trading_mode
        ),
        Style::default().fg(DIM),
    )));
    lines.push(Line::from(""));
    if m.awaiting_confirm {
        lines.push(Line::from(Span::styled(
            "CONFIRM ORDER — Enter to send, Esc to cancel.",
            Style::default().fg(GOLD).bold(),
        )));
    }
    if let Some(err) = &m.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(BAD),
        )));
    }
    lines.push(Line::from(
        "Tab next field · p preset · Enter submit · Esc cancel".fg(DIM),
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

fn render_copy_modal(f: &mut Frame, m: &CopyModal) {
    let value = |field: CopyField| -> String {
        match field {
            CopyField::Wallet => m.wallet.clone(),
            CopyField::Nickname => m.nickname.clone(),
            CopyField::Size => m.size.clone(),
            CopyField::MaxDollar => m.max_dollar.clone(),
            CopyField::MinPrice => m.min_price.clone(),
            CopyField::MaxPrice => m.max_price.clone(),
            CopyField::Slippage => m.slippage.clone(),
            CopyField::MirrorSells => if m.mirror_sells { "yes" } else { "no" }.to_string(),
        }
    };
    let mut lines = vec![Line::from("Follow a wallet".bold()), Line::from("")];
    for (i, field) in COPY_FIELDS.iter().enumerate() {
        lines.push(field_line(field.label(), &value(*field), m.focus == i));
    }
    lines.push(Line::from(""));
    lines.push(match &m.error {
        Some(e) => Line::from(Span::styled(e.clone(), Style::default().fg(BAD))),
        None => Line::from("Mirrors the wallet's new trades with your own size.".fg(DIM)),
    });
    lines.push(Line::from(
        "↑↓ move · space toggles mirror · Enter follow · Esc cancel".fg(DIM),
    ));

    let height = (lines.len() as u16 + 2).clamp(14, f.area().height.saturating_sub(2));
    let area = centered_rect(60, height, f.area());
    f.render_widget(Clear, area);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" FOLLOW WALLET ".bold())
        .border_style(Style::default().fg(ACCENT));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_reset_modal(f: &mut Frame, m: &ResetModal) {
    let area = centered_rect(56, 12, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from("Reset paper account".bold()),
        Line::from(""),
        Line::from(
            "Wipes cash, positions, open orders, and trade history, then starts fresh.".fg(DIM),
        ),
        Line::from(""),
        field_line("Starting balance ($)", &m.balance, true),
        Line::from(""),
        match &m.error {
            Some(e) => Line::from(Span::styled(e.clone(), Style::default().fg(BAD))),
            None => {
                Line::from("Guards and copy-trades are kept; only the account is reset.".fg(DIM))
            }
        },
        Line::from("Enter confirm · Esc cancel".fg(DIM)),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" RESET PAPER ACCOUNT ".bold())
        .border_style(Style::default().fg(GOLD));
    f.render_widget(
        Paragraph::new(lines).block(block).wrap(Wrap { trim: true }),
        area,
    );
}

fn render_settings_modal(f: &mut Frame, m: &SettingsEditModal) {
    let area = centered_rect(56, 11, f.area());
    f.render_widget(Clear, area);
    let lines = vec![
        Line::from("Edit setting".bold()),
        Line::from(""),
        field_line(m.field.label(), &m.input, true),
        Line::from(""),
        match &m.error {
            Some(e) => Line::from(Span::styled(e.clone(), Style::default().fg(BAD))),
            None => {
                Line::from("Lists are comma separated. Blank turns optional values off.".fg(DIM))
            }
        },
        Line::from("Enter save · Esc cancel".fg(DIM)),
    ];
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" EDIT SETTING ".bold())
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

/// True until the background refresher finishes its first pass. Marks-dependent
/// figures (equity, value, uPnL, ROI) are meaningless before then — the quote
/// cache is empty — so views show "loading…" instead of misleading zeros.
fn data_loading(app: &App) -> bool {
    app.data.lock().unwrap().last_refresh.is_none()
}

const LOADING: &str = "loading…";

/// Money that isn't trustworthy until quotes load: "loading…" while loading.
fn loading_money(v: Decimal, loading: bool) -> String {
    if loading { LOADING.into() } else { money(v) }
}

/// Signed money gated on the first quote refresh (see [`loading_money`]).
fn loading_signed(v: Decimal, loading: bool) -> String {
    if loading {
        LOADING.into()
    } else {
        signed_money(v)
    }
}

fn daily_pnl(acct: &crate::paper::types::PaperAccount) -> Decimal {
    let today = chrono::Utc::now().date_naive();
    acct.trades
        .iter()
        .filter(|t| t.timestamp.date_naive() == today)
        .filter_map(|t| t.realized_pnl)
        .sum()
}

/// Aggregate win/loss stats over closed (realized) trades. Each sell carries a
/// realized PnL; we treat every such fill as one closed trade.
struct TradeStats {
    wins: usize,
    losses: usize,
    win_rate: Decimal,
    avg_win: Decimal,
    avg_loss: Decimal,
    profit_factor: Option<Decimal>,
    expectancy: Decimal,
}

fn trade_stats(acct: &crate::paper::types::PaperAccount) -> TradeStats {
    let pnls: Vec<Decimal> = acct.trades.iter().filter_map(|t| t.realized_pnl).collect();
    let closed = pnls.len();
    if closed == 0 {
        return TradeStats {
            wins: 0,
            losses: 0,
            win_rate: Decimal::ZERO,
            avg_win: Decimal::ZERO,
            avg_loss: Decimal::ZERO,
            profit_factor: None,
            expectancy: Decimal::ZERO,
        };
    }
    let wins: Vec<Decimal> = pnls
        .iter()
        .copied()
        .filter(|p| *p > Decimal::ZERO)
        .collect();
    let losses: Vec<Decimal> = pnls
        .iter()
        .copied()
        .filter(|p| *p < Decimal::ZERO)
        .collect();
    let sum_win: Decimal = wins.iter().sum();
    let sum_loss: Decimal = losses.iter().sum(); // negative
    let avg = |xs: &[Decimal], s: Decimal| {
        if xs.is_empty() {
            Decimal::ZERO
        } else {
            s / Decimal::from(xs.len())
        }
    };
    let hundred = Decimal::from(100);
    TradeStats {
        wins: wins.len(),
        losses: losses.len(),
        win_rate: Decimal::from(wins.len()) * hundred / Decimal::from(closed),
        avg_win: avg(&wins, sum_win),
        avg_loss: avg(&losses, sum_loss),
        profit_factor: if sum_loss == Decimal::ZERO {
            None
        } else {
            Some(sum_win / -sum_loss)
        },
        expectancy: pnls.iter().sum::<Decimal>() / Decimal::from(closed),
    }
}

/// Sharpe (sample, per-snapshot, unitless) and max drawdown (%) over the
/// persisted equity curve. Returns `None` until enough samples accumulate, so
/// accounts predating equity snapshotting show nothing.
struct EquityMetrics {
    sharpe: Option<Decimal>,
    max_drawdown_pct: Decimal,
}

fn equity_metrics(curve: &[(chrono::DateTime<chrono::Utc>, Decimal)]) -> Option<EquityMetrics> {
    // Need a handful of returns for the numbers to mean anything.
    if curve.len() < 11 {
        return None;
    }
    let eq: Vec<f64> = curve
        .iter()
        .map(|&(_, e)| f64::try_from(e).unwrap_or(0.0))
        .collect();
    let returns: Vec<f64> = eq
        .windows(2)
        .filter(|w| w[0] != 0.0)
        .map(|w| (w[1] - w[0]) / w[0])
        .collect();
    let sharpe = if returns.is_empty() {
        None
    } else {
        let mean = returns.iter().sum::<f64>() / returns.len() as f64;
        let var = returns.iter().map(|r| (r - mean).powi(2)).sum::<f64>() / returns.len() as f64;
        let sd = var.sqrt();
        (sd > 0.0).then(|| Decimal::try_from(mean / sd).unwrap_or_default().round_dp(2))
    };
    let mut peak = eq[0];
    let mut max_dd = 0.0_f64;
    for &e in &eq {
        peak = peak.max(e);
        if peak > 0.0 {
            max_dd = max_dd.max((peak - e) / peak);
        }
    }
    Some(EquityMetrics {
        sharpe,
        max_drawdown_pct: Decimal::try_from(max_dd * 100.0)
            .unwrap_or_default()
            .round_dp(1),
    })
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

/// A price with its implied probability, e.g. `0.25 (25.2%)`. Kept 2-decimal so
/// the parens still fit the Positions table's Avg/Mark columns in a normal
/// terminal width.
fn price_pct(p: Decimal) -> String {
    format!("{:.2} ({})", p, pct(p))
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

#[cfg(test)]
mod stats_tests {
    use super::*;
    use crate::paper::types::{OrderKind, PaperAccount, Trade, TradeSide};
    use rust_decimal_macros::dec;

    fn close(pnl: Decimal) -> Trade {
        Trade {
            id: 1,
            timestamp: chrono::Utc::now(),
            token_id: "t".into(),
            question: "q".into(),
            outcome: "Yes".into(),
            side: TradeSide::Sell,
            kind: OrderKind::Market,
            size: dec!(1),
            price: dec!(0.5),
            notional: dec!(0.5),
            realized_pnl: Some(pnl),
        }
    }

    #[test]
    fn winrate_and_factors() {
        let mut a = PaperAccount::new(dec!(1000), true);
        // 3 wins (+10 each), 1 loss (-20): win rate 75%, PF = 30/20 = 1.5.
        a.trades = vec![
            close(dec!(10)),
            close(dec!(10)),
            close(dec!(10)),
            close(dec!(-20)),
        ];
        let s = trade_stats(&a);
        assert_eq!(s.wins, 3);
        assert_eq!(s.losses, 1);
        assert_eq!(s.win_rate, dec!(75));
        assert_eq!(s.avg_win, dec!(10));
        assert_eq!(s.avg_loss, dec!(-20));
        assert_eq!(s.profit_factor, Some(dec!(1.5)));
        assert_eq!(s.expectancy, dec!(2.5));
    }

    #[test]
    fn no_trades_is_zeroed() {
        let a = PaperAccount::new(dec!(1000), true);
        let s = trade_stats(&a);
        assert_eq!(s.wins, 0);
        assert_eq!(s.losses, 0);
        assert_eq!(s.profit_factor, None);
    }

    #[test]
    fn equity_metrics_need_samples() {
        let now = chrono::Utc::now();
        let short: Vec<_> = (0..5)
            .map(|i| (now, dec!(1000) + Decimal::from(i)))
            .collect();
        assert!(equity_metrics(&short).is_none());
    }

    #[test]
    fn equity_metrics_drawdown() {
        let now = chrono::Utc::now();
        // Up to 1100 (peak), down to 990 → drawdown = (1100-990)/1100 = 10%.
        let vals = [
            1000, 1010, 1020, 1050, 1080, 1100, 1090, 1050, 1010, 1000, 990,
        ];
        let curve: Vec<_> = vals.iter().map(|&v| (now, Decimal::from(v))).collect();
        let m = equity_metrics(&curve).unwrap();
        assert_eq!(m.max_drawdown_pct, dec!(10.0));
    }
}
