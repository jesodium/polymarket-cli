//! All TUI rendering. Reads `App` + shared data and paints the current view.

use std::collections::BTreeMap;

use polymarket_client_sdk_v2::types::Decimal;
use ratatui::Frame;
use ratatui::layout::{Alignment, Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{
    Block, BorderType, Borders, Cell, Clear, List, ListItem, Padding, Paragraph, Row, Table,
    TableState, Wrap,
};

use super::app::{
    App, COPY_FIELDS, CopyField, CopyModal, ModalField, OnboardingState, OrderModal, ResetModal,
    SETTING_ROWS, SettingRow, SettingsEditModal, View, WalletAction, WalletActionModal,
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

/// Per-render mirror of `app.frame`, so pure row-builders can animate without
/// threading the frame counter through every signature. Set once in [`render`].
static FRAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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

/// Braille spinner frame for the current tick.
fn spinner(frame: u64) -> char {
    const S: &[char] = &['⠋', '⠙', '⠹', '⠸', '⠼', '⠴', '⠦', '⠧', '⠇', '⠏'];
    S[frame as usize % S.len()]
}

/// How many of `w` bar cells are filled for a probability `p` in [0,1].
fn prob_count(p: Decimal, w: usize) -> usize {
    (0..w)
        .filter(|k| p > Decimal::from(*k as i64) / Decimal::from(w as i64))
        .count()
}

fn dec_half() -> Decimal {
    Decimal::new(5, 1)
}

/// A slow pulse in [0,1] for breathing/glow effects, period ~2.4s at 11fps.
fn pulse(frame: u64) -> f32 {
    let t = (frame % 27) as f32 / 27.0;
    (1.0 - (t * std::f32::consts::TAU).cos()) / 2.0
}

/// Lerp two colours; `t` in [0,1].
fn lerp(a: Color, b: Color, t: f32) -> Color {
    let (ar, ag, ab) = rgb(a);
    let (br, bg, bb) = rgb(b);
    let m = |x: u8, y: u8| (x as f32 + (y as f32 - x as f32) * t) as u8;
    Color::Rgb(m(ar, br), m(ag, bg), m(ab, bb))
}

fn rgb(c: Color) -> (u8, u8, u8) {
    match c {
        Color::Rgb(r, g, b) => (r, g, b),
        Color::Cyan => (0, 200, 220),
        Color::White => (235, 235, 235),
        Color::Black => (0, 0, 0),
        _ => (180, 180, 180),
    }
}

pub(crate) fn render(f: &mut Frame, app: &App) {
    FRAME.store(app.frame, std::sync::atomic::Ordering::Relaxed);
    // Onboarding takes over the full screen when no wallet is configured.
    if let Some(o) = &app.onboarding {
        render_onboarding(f, o);
        return;
    }
    // Shell: left sidebar nav | (body over status).
    let cols = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Length(22), Constraint::Min(20)])
        .split(f.area());

    render_sidebar(f, app, cols[0]);

    let right = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(5), Constraint::Length(3)])
        .split(cols[1]);
    let body = right[0];

    match app.view {
        View::Onboarding => {} // handled by early return above
        View::Dashboard => dashboard(f, app, body),
        View::Markets => markets(f, app, body),
        View::MarketDetail => market_detail(f, app, body),
        View::Portfolio => portfolio(f, app, body),
        View::Positions => positions(f, app, body),
        View::Orders => orders(f, app, body),
        View::History => history(f, app, body),
        View::Copytrade => copytrade(f, app, body),
        View::Logs => logs(f, app, body),
        View::Settings => settings(f, app, body),
    }
    render_status(f, app, right[1]);

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

/// Left navigation rail: mode badge, the view list with an active-item bar,
/// and a live connection footer.
fn render_sidebar(f: &mut Frame, app: &App, area: Rect) {
    const W: usize = 20; // inner width of the rail
    let mc = mode_color(app);
    // The mode badge breathes — a gentle pulse, more alarming in LIVE.
    let badge_bg = lerp(mc, Color::Rgb(255, 255, 255), pulse(app.frame) * 0.45);

    let mut lines: Vec<Line> = vec![
        Line::from(Span::styled(
            format!("{:^W$}", mode_label(app).trim()),
            Style::default().fg(Color::Black).bg(badge_bg).bold(),
        )),
        Line::from(""),
    ];

    for (i, v) in View::TABS.iter().enumerate() {
        let active = *v == app.view || (app.view == View::MarketDetail && *v == View::Markets);
        let label = format!(" {} {}", i + 1, v.title());
        if active {
            lines.push(Line::from(vec![
                Span::styled("▌", Style::default().fg(ACCENT)),
                Span::styled(
                    format!("{label:<width$}", width = W - 1),
                    Style::default().fg(Color::White).bg(SELECT_BG).bold(),
                ),
            ]));
        } else {
            lines.push(Line::from(Span::styled(
                format!(" {label}"),
                Style::default().fg(DIM),
            )));
        }
    }

    // Push the footer to the bottom of the rail.
    let used = lines.len() as u16 + 2; // + border
    let pad = area.height.saturating_sub(used + 1);
    for _ in 0..pad {
        lines.push(Line::from(""));
    }

    let d = app.data.lock().unwrap();
    let connected = d.connected;
    let markets_status = d.markets_status.clone();
    drop(d);
    // Footer: heartbeat when live, spinner while data is syncing.
    let loading = data_loading(app);
    let footer = if !connected {
        Span::styled(
            format!(" {} offline", spinner(app.frame)),
            Style::default().fg(GOLD),
        )
    } else if loading {
        Span::styled(
            format!(" {} syncing…", spinner(app.frame)),
            Style::default().fg(ACCENT),
        )
    } else {
        let dot = lerp(Color::Rgb(20, 70, 35), GOOD, pulse(app.frame));
        Span::styled(" ● connected", Style::default().fg(dot))
    };
    lines.push(Line::from(footer));
    if !markets_status.is_empty() {
        lines.push(Line::from(Span::styled(
            format!(" {markets_status}"),
            Style::default().fg(DIM),
        )));
    }

    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(mc))
        .title(Span::styled(
            " ◈ FIBERGLASS ",
            Style::default().fg(mc).bold(),
        ));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn render_status(f: &mut Frame, app: &App, area: Rect) {
    let help = match app.view {
        View::Markets => "↑↓/jk move · Enter open · / search · Tab views · q quit",
        View::MarketDetail => "←→ outcome · t timeframe · ↑↓ scroll · b buy · s sell · Esc back",
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
            .border_type(BorderType::Rounded)
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
        kv_colored(
            "ROI",
            &if loading {
                loading_anim()
            } else {
                format!("{}%", view.roi_pct)
            },
            if loading {
                DIM
            } else {
                pnl_color(view.roi_pct)
            },
        ),
        kv_colored(
            "Realized PnL",
            &signed_money(view.realized_pnl),
            pnl_color(view.realized_pnl),
        ),
        kv_colored(
            "Unrealized PnL",
            &loading_signed(view.unrealized_pnl, loading),
            if loading {
                DIM
            } else {
                pnl_color(view.unrealized_pnl)
            },
        ),
        win_rate_line(stats.win_rate, stats.wins, stats.losses),
        kv_colored(
            "Avg Win",
            &signed_money(stats.avg_win),
            pnl_color(stats.avg_win),
        ),
        kv_colored(
            "Avg Loss",
            &signed_money(stats.avg_loss),
            pnl_color(stats.avg_loss),
        ),
        kv_line(
            "Profit Factor",
            &match stats.profit_factor {
                Some(pf) => pf.round_dp(2).to_string(),
                None => "∞".into(),
            },
        ),
        kv_colored(
            "Expectancy",
            &signed_money(stats.expectancy),
            pnl_color(stats.expectancy),
        ),
    ];
    // Only shown once equity snapshots have accumulated (hidden for accounts
    // predating equity snapshotting).
    if let Some(eq) = equity_stats
        && let Some(sh) = eq.sharpe
    {
        info.push(kv_line("Sharpe", &sh.to_string()));
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
        .constraints([Constraint::Percentage(56), Constraint::Percentage(44)])
        .split(area);

    // Left column: header · outcome probability bars · resolution rules.
    let n = d.token_ids.len() as u16;
    let left = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(4),     // header
            Constraint::Length(n + 2), // outcomes
            Constraint::Length(11),    // price-history chart
            Constraint::Min(4),        // rules
        ])
        .split(cols[0]);

    // Header: question + the facts that matter (close, resolver, volume).
    let mut facts: Vec<Span> = Vec::new();
    if let Some(end) = d.end_date {
        facts.push(Span::styled("closes ", Style::default().fg(DIM)));
        facts.push(Span::raw(end.format("%b %d %H:%M").to_string()));
    }
    if let Some(v) = d.volume {
        facts.push(Span::styled("   vol ", Style::default().fg(DIM)));
        facts.push(Span::raw(short_money(Some(v))));
    }
    if let Some(src) = d.resolution_source.as_deref().filter(|s| !s.is_empty()) {
        facts.push(Span::styled("   via ", Style::default().fg(DIM)));
        facts.push(Span::raw(truncate(src, 18)));
    }
    facts.push(Span::styled(
        format!("   #{}", truncate(&d.id, 10)),
        Style::default().fg(PANEL),
    ));
    f.render_widget(
        Paragraph::new(vec![
            Line::from(d.question.clone().bold()),
            Line::from(facts),
        ])
        .block(panel("Market"))
        .wrap(Wrap { trim: true }),
        left[0],
    );

    // Outcomes as horizontal probability bars; ←→ moves the focus.
    const BARW: usize = 14;
    let mut outcomes: Vec<Line> = Vec::new();
    for (i, _tid) in d.token_ids.iter().enumerate() {
        let name = d
            .outcomes
            .get(i)
            .cloned()
            .unwrap_or_else(|| format!("Outcome {}", i + 1));
        let p = d.prices.get(i).copied().unwrap_or(Decimal::ZERO);
        let selected = i == app.detail_token;
        let fill = prob_count(p, BARW);
        let bar_color = if selected {
            ACCENT
        } else if p >= dec_half() {
            GOOD
        } else {
            GOLD
        };
        let marker = if selected { "▶ " } else { "  " };
        let name_style = if selected {
            Style::default().fg(ACCENT).bold()
        } else {
            Style::default().fg(Color::White)
        };
        outcomes.push(Line::from(vec![
            Span::styled(format!("{marker}{:<12}", truncate(&name, 12)), name_style),
            Span::styled("█".repeat(fill), Style::default().fg(bar_color)),
            Span::styled("░".repeat(BARW - fill), Style::default().fg(PANEL)),
            Span::styled(format!(" {:>4}", pct(p)), Style::default().fg(bar_color)),
        ]));
    }
    f.render_widget(
        Paragraph::new(outcomes).block(panel("Outcomes ←→ · b buy · s sell")),
        left[1],
    );

    // Price history of the focused outcome, at the chosen timeframe (t cycles).
    let focused_name = d
        .outcomes
        .get(app.detail_token)
        .cloned()
        .unwrap_or_else(|| format!("Outcome {}", app.detail_token + 1));
    render_price_chart(f, app, &focused_name, left[2]);

    // Resolution rules (scrollable).
    let rules: Vec<Line> = match d.description.as_deref().filter(|s| !s.is_empty()) {
        Some(text) => text.lines().map(|l| Line::from(l.to_string())).collect(),
        None => vec![Line::from("No resolution rules provided.".fg(DIM))],
    };
    f.render_widget(
        Paragraph::new(rules)
            .block(panel("Resolution ↑↓ scroll"))
            .wrap(Wrap { trim: false })
            .scroll((app.detail_scroll, 0)),
        left[3],
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

/// Tug-of-war bars: one row per time bucket, each split between the focused
/// outcome (left, green) and the opposing side (right, gold) so the boundary
/// shows who's winning and how the fight shifted over time. The series is
/// fetched in the background keyed by token + timeframe; until the fresh one
/// lands we show a placeholder rather than a stale picture.
fn render_price_chart(f: &mut Frame, app: &App, outcome: &str, area: Rect) {
    use super::data::DETAIL_TIMEFRAMES;

    let focused_token = app
        .detail
        .as_ref()
        .and_then(|d| d.token_ids.get(app.detail_token))
        .cloned()
        .unwrap_or_default();
    // The opposing side: the other outcome in a binary market, else "rest".
    let opponent = app
        .detail
        .as_ref()
        .filter(|d| d.outcomes.len() == 2)
        .map(|d| d.outcomes[1 - app.detail_token.min(1)].clone())
        .unwrap_or_else(|| "rest".to_string());

    // Title: the matchup + the timeframe selector with the active one lit.
    let mut title: Vec<Span> = vec![
        Span::styled(
            format!(" {} ", truncate(outcome, 12)),
            Style::default().fg(GOOD).bold(),
        ),
        Span::styled("vs ", Style::default().fg(DIM)),
        Span::styled(
            format!("{} ", truncate(&opponent, 12)),
            Style::default().fg(GOLD).bold(),
        ),
        Span::styled("· t ", Style::default().fg(DIM)),
    ];
    for (i, (label, _, _)) in DETAIL_TIMEFRAMES.iter().enumerate() {
        let style = if i == app.detail_timeframe {
            Style::default().fg(ACCENT).bold()
        } else {
            Style::default().fg(DIM)
        };
        title.push(Span::styled(format!("{label} "), style));
    }
    let block = Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(PANEL))
        .title(Line::from(title));

    let data = app.data.lock().unwrap();
    let points: Vec<(f64, f64)> = match data.price_history.as_ref() {
        Some(ph) if ph.token == focused_token && ph.timeframe == app.detail_timeframe => {
            ph.points.clone()
        }
        _ => Vec::new(),
    };
    drop(data);

    if points.len() < 2 {
        f.render_widget(
            Paragraph::new("loading price history…".fg(DIM)).block(block),
            area,
        );
        return;
    }

    // One bar per usable text row; downsample the series evenly to fit. Time
    // flows top (oldest) → bottom (latest).
    let rows = area.height.saturating_sub(2).max(1) as usize;
    let inner_w = area.width.saturating_sub(2) as usize;
    // Reserve "HH:MM " label (6) and " 63%" readout (5); the rest is the bar.
    let bar_w = inner_w.saturating_sub(11).max(4);

    let n = points.len();
    let take = rows.min(n);
    let lines: Vec<Line> = (0..take)
        .map(|i| {
            // Evenly spaced sample indices across the whole series.
            let idx = if take == 1 {
                n - 1
            } else {
                i * (n - 1) / (take - 1)
            };
            let (t, p) = points[idx];
            let p = p.clamp(0.0, 1.0);
            let fill = (p * bar_w as f64).round() as usize;
            let fill = fill.min(bar_w);
            let time = chrono::DateTime::from_timestamp(t as i64, 0)
                .map_or("--:--".to_string(), |dt| dt.format("%H:%M").to_string());
            Line::from(vec![
                Span::styled(format!("{time} "), Style::default().fg(DIM)),
                Span::styled("█".repeat(fill), Style::default().fg(GOOD)),
                Span::styled("█".repeat(bar_w - fill), Style::default().fg(GOLD)),
                Span::styled(format!(" {:>3.0}%", p * 100.0), Style::default().fg(GOOD)),
            ])
        })
        .collect();

    f.render_widget(Paragraph::new(lines).block(block), area);
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
            let ph = if loading {
                loading_anim()
            } else {
                "—".to_string()
            };
            let (mark_cell, value_cell, upnl_cell) = match p.mark_price {
                Some(mark) => {
                    let upnl = p.unrealized_pnl.unwrap_or_default();
                    (
                        Cell::from(format!("{mark:.3}")),
                        Cell::from(p.market_value.map(money).unwrap_or_else(|| ph.clone())),
                        Cell::from(signed_money(upnl)).style(Style::default().fg(pnl_color(upnl))),
                    )
                }
                None => {
                    let dim = Style::default().fg(DIM);
                    (
                        Cell::from(ph.clone()).style(dim),
                        Cell::from(ph.clone()).style(dim),
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
    let placeholder = if loading {
        loading_anim()
    } else {
        "—".to_string()
    };
    let value_str = p
        .market_value
        .map(money)
        .unwrap_or_else(|| placeholder.clone());

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
                Cell::from(placeholder.clone()).style(dim),
                Cell::from(placeholder.clone()).style(dim),
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
                r#"  "command": "fiberglass", "args": ["mcp"]"#,
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
                "m — Log in (import key)",
                Style::default().fg(GOLD),
            )));
            lines.push(Line::from(Span::styled(
                "x — Set proxy/funder address",
                Style::default().fg(GOLD),
            )));
            lines.push(Line::from(Span::styled(
                "y — Cycle signature type",
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
                    "m — Log in (import key)",
                    Style::default().fg(GOLD),
                )));
            } else {
                lines.push(Line::from(
                    "Run `fiberglass wallet import <key>` then relaunch without --paper for live mode."
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
            " FIBERGLASS LIVE TRADING ",
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
        Line::from("Log in to your Polymarket account".bold()),
        Line::from(""),
        Line::from("Export your private key from the Polymarket web app (Settings →".fg(DIM)),
        Line::from("Export Private Key) and paste it below.".fg(DIM)),
    ];
    f.render_widget(
        Paragraph::new(welcome).alignment(Alignment::Center),
        chunks[0],
    );

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
    lines.push(Line::from(
        "  Enter to log in · Esc to browse markets only".fg(DIM),
    ));
    f.render_widget(
        Paragraph::new(lines).alignment(Alignment::Center),
        chunks[1],
    );
}

// --- Wallet action modal (Settings tab) ----------------------------------

fn render_wallet_action_modal(f: &mut Frame, m: &WalletActionModal) {
    match m.action {
        WalletAction::Import => {
            let mut lines = vec![
                Line::from(Span::styled("Private key", Style::default().fg(DIM))),
                Line::from(format!("{}█", m.import_key)),
                Line::from(""),
            ];
            if let Some(e) = &m.error {
                lines.push(Line::from(Span::styled(
                    format!("✗ {e}"),
                    Style::default().fg(BAD),
                )));
                lines.push(Line::from(""));
            }
            if m.confirmed {
                lines.push(Line::from("This REPLACES your current wallet.".fg(BAD)));
                lines.push(Line::from("Back up your existing key first.".fg(DIM)));
                lines.push(Line::from(""));
                lines.push(Line::from(
                    "Enter to confirm overwrite · Esc to cancel".fg(DIM),
                ));
            } else {
                lines.push(Line::from("Enter to import · Esc to cancel".fg(DIM)));
            }
            popup(f, 58, "IMPORT WALLET", ACCENT, lines);
        }
        WalletAction::SetProxy => {
            let mut lines = vec![
                Line::from(
                    "Proxy/funder address — the wallet Polymarket shows on your profile.".fg(DIM),
                ),
                Line::from("Fixes \"maker address not allowed\". Leave blank to clear.".fg(DIM)),
                Line::from(""),
                Line::from(Span::styled(
                    "Proxy address (0x…)",
                    Style::default().fg(DIM),
                )),
                Line::from(format!("{}█", m.import_key)),
                Line::from(""),
            ];
            if let Some(e) = &m.error {
                lines.push(Line::from(Span::styled(
                    format!("✗ {e}"),
                    Style::default().fg(BAD),
                )));
                lines.push(Line::from(""));
            }
            lines.push(Line::from("Enter to save · Esc to cancel".fg(DIM)));
            popup(f, 64, "SET PROXY ADDRESS", ACCENT, lines);
        }
    }
}

// --- Modal -----------------------------------------------------------------

fn render_modal(f: &mut Frame, app: &App, m: &OrderModal) {
    let side = m.side.to_string();
    let venue = if app.live { "LIVE" } else { "PAPER" };
    let title = format!("{side} ORDER · {venue} — {}", truncate(&m.outcome, 18));

    let kind_line = Line::from(vec![
        Span::raw("Type: "),
        toggle_span("Market", m.kind == OrderKind::Market),
        Span::raw("  "),
        toggle_span("Limit", m.kind == OrderKind::Limit),
        Span::styled("   (m / L)", Style::default().fg(DIM)),
    ]);

    let border = if app.live {
        LIVE
    } else if m.side == TradeSide::Buy {
        GOOD
    } else {
        BAD
    };

    // Live execution price from the book; fall back to the last mark.
    let (bid, ask) = {
        let d = app.data.lock().unwrap();
        let b = d.book(&m.token_id).cloned();
        (
            b.as_ref().and_then(|b| b.best_bid),
            b.as_ref().and_then(|b| b.best_ask),
        )
    };
    let mark = marks_snapshot(app).get(&m.token_id).copied();
    let exec_px = match m.kind {
        OrderKind::Limit => m.price.parse::<Decimal>().ok(),
        _ => match m.side {
            TradeSide::Buy => ask.or(mark),
            TradeSide::Sell => bid.or(mark),
        },
    };

    let area = centered_rect(78, 19, f.area());
    f.render_widget(Clear, area);
    let block = modal_block(&title, border);
    let inner = block.inner(area);
    f.render_widget(block, area);

    // header (question / live warning) | body (ticket | receipt) | footer.
    let header_h = if app.live { 2 } else { 1 };
    let rows = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(header_h),
            Constraint::Min(6),
            Constraint::Length(2),
        ])
        .split(inner);

    let mut head = vec![Line::from(truncate(&m.question, 72).fg(ACCENT))];
    if app.live {
        head.push(Line::from(Span::styled(
            "⚠ REAL FUNDS — submits a signed CLOB order.",
            Style::default().fg(LIVE).bold(),
        )));
    }
    f.render_widget(Paragraph::new(head), rows[0]);

    let body = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
        .split(rows[1]);
    render_order_form(f, app, m, kind_line, body[0]);
    render_order_receipt(f, m, exec_px, bid, ask, body[1]);

    let footer = if m.awaiting_confirm {
        Line::from(Span::styled(
            "⏎ CONFIRM — send order   ·   Esc cancel",
            Style::default().fg(GOLD).bold(),
        ))
    } else if let Some(err) = &m.error {
        Line::from(Span::styled(format!("✗ {err}"), Style::default().fg(BAD)))
    } else {
        Line::from(Span::styled(
            "Tab field · m/L type · p preset · ⏎ submit · Esc cancel",
            Style::default().fg(DIM),
        ))
    };
    f.render_widget(Paragraph::new(footer).wrap(Wrap { trim: true }), rows[2]);
}

/// Left pane of the order ticket: the editable inputs.
fn render_order_form(f: &mut Frame, app: &App, m: &OrderModal, kind_line: Line, area: Rect) {
    let mut lines = vec![kind_line, Line::from("")];
    match m.kind {
        OrderKind::Market => {
            let label = if m.side == TradeSide::Buy {
                "Amount ($)"
            } else {
                "Shares"
            };
            lines.push(field_line(label, &m.amount, m.field == ModalField::Amount));
        }
        OrderKind::Limit => {
            lines.push(field_line("Price", &m.price, m.field == ModalField::Price));
            lines.push(field_line("Size", &m.size, m.field == ModalField::Size));
        }
        OrderKind::Settlement => {}
    }
    // Take-profit / stop-loss guards auto-attach on fill (buys only).
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
    lines.push(Line::from(""));
    let preset_hint = match m.side {
        TradeSide::Buy => format!(
            "p: {}",
            crate::settings::fmt_money_list(&app.settings.quickbuy_presets)
        ),
        TradeSide::Sell => format!(
            "p: {} of {} held",
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
            "slip {}% · {}",
            app.settings.slippage_pct.normalize(),
            app.settings.trading_mode
        ),
        Style::default().fg(DIM),
    )));
    f.render_widget(
        Paragraph::new(lines)
            .block(panel("Ticket"))
            .wrap(Wrap { trim: true }),
        area,
    );
}

/// Right pane: a live receipt — what you pay, shares, and payout if it resolves
/// your way. This is the part that makes the order legible at a glance.
fn render_order_receipt(
    f: &mut Frame,
    m: &OrderModal,
    exec_px: Option<Decimal>,
    bid: Option<Decimal>,
    ask: Option<Decimal>,
    area: Rect,
) {
    let px = |p: Option<Decimal>| p.map(|v| format!("{v:.3}")).unwrap_or_else(|| "—".into());
    let dec = |p: Option<Decimal>| {
        p.map(|v| v.round_dp(2).to_string())
            .unwrap_or_else(|| "—".into())
    };
    let kv = |k: &str, v: String, c: Color| {
        Line::from(vec![
            Span::styled(format!("{k:<12}"), Style::default().fg(DIM)),
            Span::styled(v, Style::default().fg(c)),
        ])
    };
    let won = |profit: Decimal| {
        Line::from(Span::styled(
            format!("→ if wins  {}", signed_money(profit)),
            Style::default()
                .fg(if profit >= Decimal::ZERO { GOOD } else { BAD })
                .bold(),
        ))
    };

    let mut lines = vec![
        kv(
            "Bid / Ask",
            format!("{} / {}", px(bid), px(ask)),
            Color::White,
        ),
        kv("Exec ~", px(exec_px), ACCENT),
        Line::from(""),
    ];
    let amt = m.amount.parse::<Decimal>().ok();
    let size = m.size.parse::<Decimal>().ok();
    let price = m.price.parse::<Decimal>().ok();
    match (m.kind, m.side) {
        (OrderKind::Market, TradeSide::Buy) => {
            let shares = match (amt, exec_px) {
                (Some(p), Some(x)) if x > Decimal::ZERO => Some(p / x),
                _ => None,
            };
            lines.push(kv("You pay", dec(amt), Color::White));
            lines.push(kv("Est. shares", dec(shares), Color::White));
            if let (Some(p), Some(s)) = (amt, shares) {
                lines.push(kv("Max payout", money(s), GOLD));
                lines.push(Line::from(""));
                lines.push(won(s - p));
            }
        }
        (OrderKind::Market, TradeSide::Sell) => {
            let proceeds = match (amt, exec_px) {
                (Some(s), Some(x)) => Some(s * x),
                _ => None,
            };
            lines.push(kv("Sell shares", dec(amt), Color::White));
            lines.push(kv(
                "Proceeds",
                proceeds.map(money).unwrap_or_else(|| "—".into()),
                GOOD,
            ));
        }
        (OrderKind::Limit, side) => {
            let cost = match (price, size) {
                (Some(p), Some(s)) => Some(p * s),
                _ => None,
            };
            lines.push(kv("Size", dec(size), Color::White));
            match side {
                TradeSide::Buy => {
                    lines.push(kv(
                        "Cost",
                        cost.map(money).unwrap_or_else(|| "—".into()),
                        Color::White,
                    ));
                    if let (Some(c), Some(s)) = (cost, size) {
                        lines.push(kv("Max payout", money(s), GOLD));
                        lines.push(Line::from(""));
                        lines.push(won(s - c));
                    }
                }
                TradeSide::Sell => {
                    lines.push(kv(
                        "Proceeds",
                        cost.map(money).unwrap_or_else(|| "—".into()),
                        GOOD,
                    ));
                }
            }
        }
        _ => {}
    }
    f.render_widget(
        Paragraph::new(lines)
            .block(panel("Preview"))
            .wrap(Wrap { trim: true }),
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
    let mut lines: Vec<Line> = Vec::new();
    for (i, field) in COPY_FIELDS.iter().enumerate() {
        lines.push(field_line(field.label(), &value(*field), m.focus == i));
    }
    lines.push(Line::from(""));
    lines.push(match &m.error {
        Some(e) => Line::from(Span::styled(format!("✗ {e}"), Style::default().fg(BAD))),
        None => Line::from("Mirrors the wallet's new trades with your own size.".fg(DIM)),
    });
    lines.push(Line::from(
        "↑↓ move · space toggles mirror · Enter follow · Esc cancel".fg(DIM),
    ));
    popup(f, 60, "FOLLOW WALLET", ACCENT, lines);
}

fn render_reset_modal(f: &mut Frame, m: &ResetModal) {
    let lines = vec![
        Line::from(
            "Wipes cash, positions, open orders, and trade history, then starts fresh.".fg(DIM),
        ),
        Line::from(""),
        field_line("Starting balance ($)", &m.balance, true),
        Line::from(""),
        match &m.error {
            Some(e) => Line::from(Span::styled(format!("✗ {e}"), Style::default().fg(BAD))),
            None => {
                Line::from("Guards and copy-trades are kept; only the account is reset.".fg(DIM))
            }
        },
        Line::from("Enter confirm · Esc cancel".fg(DIM)),
    ];
    popup(f, 58, "RESET PAPER ACCOUNT", GOLD, lines);
}

fn render_settings_modal(f: &mut Frame, m: &SettingsEditModal) {
    let lines = vec![
        field_line(m.field.label(), &m.input, true),
        Line::from(""),
        match &m.error {
            Some(e) => Line::from(Span::styled(format!("✗ {e}"), Style::default().fg(BAD))),
            None => {
                Line::from("Lists are comma separated. Blank turns optional values off.".fg(DIM))
            }
        },
        Line::from("Enter save · Esc cancel".fg(DIM)),
    ];
    popup(f, 58, "EDIT SETTING", ACCENT, lines);
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
                .border_type(BorderType::Rounded)
                .border_style(Style::default().fg(color)),
        ),
        area,
    );
}

fn panel(title: &str) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(PANEL))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(HEADER).bold(),
        ))
}

/// One look for every popup: rounded border in the accent colour, a coloured
/// title, and breathing room inside (1 col padding, 1 row top/bottom).
fn modal_block(title: &str, color: Color) -> Block<'static> {
    Block::default()
        .borders(Borders::ALL)
        .border_type(BorderType::Rounded)
        .border_style(Style::default().fg(color))
        .title(Span::styled(
            format!(" {title} "),
            Style::default().fg(color).bold(),
        ))
        .padding(Padding::new(2, 2, 1, 1))
}

/// Center, clear, and draw a modal sized to its content. Single code path so
/// every popup lines up the same way. ponytail: one helper kills five copies.
fn popup(f: &mut Frame, width: u16, title: &str, color: Color, lines: Vec<Line>) {
    // Count wrapped rows so long help text never clips the footer.
    let inner = width.saturating_sub(6).max(1); // 1 border + 2 padding each side
    let rows: u16 = lines
        .iter()
        .map(|l| (l.width() as u16).div_ceil(inner).max(1))
        .sum();
    let height = (rows + 4).min(f.area().height); // + border + vertical padding
    let area = centered_rect(width, height, f.area());
    f.render_widget(Clear, area);
    f.render_widget(
        Paragraph::new(lines)
            .block(modal_block(title, color))
            .wrap(Wrap { trim: true }),
        area,
    );
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

/// Same as `kv_line` but tints the value (used for PnL/ROI lines).
fn kv_colored(key: &str, val: &str, color: Color) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{key:<22}"), Style::default().fg(DIM)),
        Span::styled(val.to_string(), Style::default().fg(color)),
    ])
}

/// Win-rate line with the W count in green and the L count in red.
fn win_rate_line(rate: Decimal, wins: usize, losses: usize) -> Line<'static> {
    Line::from(vec![
        Span::styled(format!("{:<22}", "Win Rate"), Style::default().fg(DIM)),
        Span::raw(format!("{}% (", rate.round_dp(1))),
        Span::styled(format!("{wins}W"), Style::default().fg(GOOD)),
        Span::raw(" "),
        Span::styled(format!("{losses}L"), Style::default().fg(BAD)),
        Span::raw(")"),
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

/// Animated "loading" placeholder. Pure row-builders can't see `app.frame`, so
/// they read the per-render frame mirror set in [`render`].
/// ponytail: one atomic beats threading `frame` through a dozen call sites.
fn loading_anim() -> String {
    let frame = FRAME.load(std::sync::atomic::Ordering::Relaxed);
    format!("{} loading", spinner(frame))
}

/// Money that isn't trustworthy until quotes load: animated dots while loading.
fn loading_money(v: Decimal, is_loading: bool) -> String {
    if is_loading { loading_anim() } else { money(v) }
}

/// Signed money gated on the first quote refresh (see [`loading_money`]).
fn loading_signed(v: Decimal, is_loading: bool) -> String {
    if is_loading {
        loading_anim()
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

/// Sharpe (sample, per-snapshot, unitless) over the persisted equity curve.
/// Returns `None` until enough samples accumulate, so accounts predating
/// equity snapshotting show nothing.
struct EquityMetrics {
    sharpe: Option<Decimal>,
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
    Some(EquityMetrics { sharpe })
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
    fn prob_bar_scales_with_probability() {
        assert_eq!(prob_count(dec!(0), 14), 0);
        assert_eq!(prob_count(dec!(1), 14), 14);
        assert_eq!(prob_count(dec!(0.5), 14), 7);
        assert!(prob_count(dec!(0.05), 14) < prob_count(dec!(0.92), 14));
    }

    #[test]
    fn popup_draws_rounded_frame_and_title() {
        use ratatui::Terminal;
        use ratatui::backend::TestBackend;
        let mut t = Terminal::new(TestBackend::new(70, 14)).unwrap();
        let m = ResetModal {
            balance: "10000".into(),
            error: None,
        };
        t.draw(|f| render_reset_modal(f, &m)).unwrap();
        let buf = t.backend().buffer().clone();
        let mut text = String::new();
        for y in 0..buf.area.height {
            for x in 0..buf.area.width {
                text.push_str(buf[(x, y)].symbol());
            }
        }
        assert!(text.contains('╭'), "rounded corner missing");
        assert!(text.contains("RESET PAPER ACCOUNT"), "title missing");
        assert!(text.contains("Esc cancel"), "footer clipped");
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
}
