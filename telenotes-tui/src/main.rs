mod app;
mod botclient;
mod config;
mod session;
mod supabase;
mod ui;

use std::io::Stdout;
use std::time::Duration;

use anyhow::Result;
use crossterm::event::{self, Event, KeyEventKind};
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use crossterm::{execute, ExecutableCommand};
use ratatui::backend::CrosstermBackend;
use ratatui::Terminal;

use app::App;
use botclient::BotClient;
use config::Config;
use supabase::SupabaseClient;

#[tokio::main]
async fn main() -> Result<()> {
    let config = Config::from_env()?;
    let supabase = SupabaseClient::new(config)?;
    let bot = BotClient::new(supabase.bot_url().to_string())?;

    let mut app = App::new(supabase, bot);
    app.bootstrap().await;

    let mut terminal = start_terminal()?;
    let result = run(&mut terminal, &mut app).await;
    stop_terminal(&mut terminal)?;

    result
}

fn start_terminal() -> Result<Terminal<CrosstermBackend<Stdout>>> {
    enable_raw_mode()?;
    let mut stdout = std::io::stdout();
    stdout.execute(EnterAlternateScreen)?;
    Ok(Terminal::new(CrosstermBackend::new(stdout))?)
}

fn stop_terminal(terminal: &mut Terminal<CrosstermBackend<Stdout>>) -> Result<()> {
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;
    Ok(())
}

/// Polling `crossterm::event` blocks the current task for up to 200ms per
/// iteration, which is fine here: network calls only happen in response to a
/// key press, never concurrently with input polling, so there's nothing else
/// for this task to be doing in between.
async fn run(terminal: &mut Terminal<CrosstermBackend<Stdout>>, app: &mut App) -> Result<()> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;

        if event::poll(Duration::from_millis(200))? {
            if let Event::Key(key) = event::read()? {
                if key.kind == KeyEventKind::Press {
                    app.handle_key(key.code).await;
                }
            }
        }

        if app.should_quit {
            return Ok(());
        }
    }
}
