//! The interactive trading terminal — the primary interface to the app.
//!
//! Brings together three concurrent pieces in one process:
//!   * a background [`data`] refresher keeping markets and order books live,
//!   * the local [`crate::strategy`] engine ticking strategies, and
//!   * a render loop ([`App`]/[`ui`]) that stays responsive throughout because
//!     it only ever reads already-fetched state.

mod app;
mod data;
mod live;
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
use crate::paper::store;
use crate::paper::types::{PaperAccount, default_starting_balance};
use crate::strategy::engine::{ExecutionMode, StrategyEngine};
use app::App;

const TICK_SECS: u64 = 10;

/// Launch the terminal. `paper = true` trades the simulated account;
/// `paper = false` runs LIVE against the real wallet and CLOB.
pub(crate) async fn run(paper: bool) -> Result<()> {
    let mode = if paper {
        ExecutionMode::Paper
    } else {
        ExecutionMode::Live
    };

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
        // Live: a wallet is required; the account is hydrated from real state.
        if config::resolve_key(None)?.0.is_none() {
            anyhow::bail!(
                "LIVE mode needs a wallet. Run `polymarket wallet create` (or import), or launch with `polymarket tui --paper` for simulated trading."
            );
        }
        let user = live::resolve_user_address()
            .context("Could not derive wallet address for live mode")?;
        // Empty shell; the refresher fills in real cash and positions.
        (
            PaperAccount::new(polymarket_client_sdk_v2::types::Decimal::ZERO, false),
            Some(user),
        )
    };
    let account = Arc::new(Mutex::new(account));

    // Engine shares the same account handle so its activity shows up live.
    let engine = StrategyEngine::new(Arc::clone(&account), TICK_SECS, mode);
    let shared = data::new_shared();

    // Background workers.
    tokio::spawn(data::refresher(
        Arc::clone(&shared),
        Arc::clone(&account),
        live_user,
    ));
    tokio::spawn(engine.clone().run_forever());

    let mut app = App::new(
        Arc::clone(&shared),
        Arc::clone(&account),
        engine.clone(),
        !paper,
    );

    let mut terminal = setup_terminal()?;
    let res = event_loop(&mut terminal, &mut app).await;
    restore_terminal(&mut terminal)?;

    // Persist only the paper account; live state lives on-chain / at the CLOB.
    if paper {
        let _ = store::save(&account.lock().unwrap());
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

        // Block up to 200ms for input; background tasks keep running on other
        // worker threads, so the UI redraws ~5x/sec even when idle.
        if event::poll(Duration::from_millis(200))?
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
