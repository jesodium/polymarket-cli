//! The interactive trading terminal — the primary interface to the app.
//!
//! Brings together three concurrent pieces in one process:
//!   * a background [`data`] refresher keeping markets and order books live
//!     (and ticking the TP/SL [`crate::guard`]s),
//!   * the [`crate::copytrade`] engine mirroring followed wallets, and
//!   * a render loop ([`App`]/[`ui`]) that stays responsive throughout because
//!     it only ever reads already-fetched state.

mod app;
pub(crate) mod data;
pub(crate) mod live;
mod ui;

use std::io;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;

use crate::config;
use crate::copytrade::engine::CopyEngine;
use crate::paper::store;
use crate::paper::types::{PaperAccount, default_starting_balance};
use app::App;

/// Launch the terminal. `paper = true` trades the simulated account;
/// `paper = false` runs LIVE against the real wallet and CLOB.
pub(crate) async fn run(paper: bool) -> Result<()> {
    // Boot the background guard worker so TP/SL exits keep firing after the
    // terminal closes. While we run, our heartbeat makes it skip this mode's
    // guards — the in-process ticker below owns them.
    crate::commands::guard::ensure_worker(true);

    let (account, live_user) = if paper {
        // Paper account is the backend; create one on first launch.
        let acct = match store::load()? {
            Some(a) => a,
            None => {
                let a = PaperAccount::new(default_starting_balance(), true);
                store::save(&a)?;
                a
            }
        };
        (acct, None)
    } else {
        // Live: wallet may or may not exist. If missing we'll show onboarding.
        let has_wallet = config::resolve_key(None)?.0.is_some();
        let user = if has_wallet {
            Some(
                live::resolve_user_address()
                    .context("Could not derive wallet address for live mode")?,
            )
        } else {
            None
        };
        // Empty shell; the refresher fills in real cash and positions.
        (
            PaperAccount::new(polymarket_client_sdk_v2::types::Decimal::ZERO, false),
            user,
        )
    };
    let account = Arc::new(Mutex::new(account));

    // The copy engine shares the same account handle so paper fills show up live
    // alongside manual trades. It's scoped to this TUI's mode (below), so a live
    // TUI only mirrors live followers (CLOB) and never writes this paper handle —
    // the headless daemon owns the other mode's followers while we're closed.
    let copy_engine = CopyEngine::new(Arc::clone(&account), crate::settings::load().copy_poll_secs);
    copy_engine.set_scope(paper);
    let shared = data::new_shared();

    // Background workers. The refresher also ticks the TP/SL guards.
    tokio::spawn(data::refresher(
        Arc::clone(&shared),
        Arc::clone(&account),
        live_user,
    ));
    tokio::spawn(copy_engine.clone().run_forever());

    let mut app = App::new(
        Arc::clone(&shared),
        Arc::clone(&account),
        copy_engine.clone(),
        !paper,
    );

    let mut terminal = setup_terminal()?;
    let res = event_loop(&mut terminal, &mut app).await;
    let run_upgrade = app.run_upgrade;
    restore_terminal(&mut terminal)?;

    // Persist only the paper account; live state lives on-chain / at the CLOB.
    if paper {
        let _ = store::save(&account.lock().unwrap());
    }

    if run_upgrade {
        return crate::commands::upgrade::execute();
    }

    res
}

async fn event_loop<B: ratatui::backend::Backend>(
    terminal: &mut Terminal<B>,
    app: &mut App,
) -> Result<()> {
    loop {
        app.pre_frame();
        terminal.draw(|f| ui::render(f, app))?;

        // Block up to 90ms for input; background tasks keep running on other
        // worker threads, so the UI redraws ~11x/sec even when idle (drives
        // animations — spinners, matrix rain). Cheap: only reads cached state.
        if event::poll(Duration::from_millis(90))?
            && let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.on_key(key);
        }
        if app.should_quit {
            return Ok(());
        }
    }
}

fn setup_terminal() -> Result<Terminal<CrosstermBackend<io::Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    Ok(Terminal::new(backend)?)
}

fn restore_terminal(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}
