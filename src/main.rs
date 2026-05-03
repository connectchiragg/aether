#![allow(dead_code)]

mod app;
mod engine;
mod event;
mod live;
mod mock;
mod model;
mod theme;
mod ui;

use app::{App, View};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{KeyCode, KeyEvent, KeyModifiers},
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use event::{AppEvent, EventHandler};
use ratatui::prelude::*;
use std::io;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "aether", about = "See the invisible — live agent observability")]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Watch live agent activity from session JSONL files
    Watch {
        /// Path to the threads directory
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
    /// Run a scripted demo scenario
    Demo,
}

fn default_threads_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".claude").join("threads")
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let mut app = match cli.command {
        Some(Commands::Watch { dir }) => {
            let threads_dir = dir.unwrap_or_else(default_threads_dir);
            App::new_live(threads_dir)
        }
        Some(Commands::Demo) | None => App::new_mock(),
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen)?;
    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend)?;

    // Run app
    let (result, events) = run_app(&mut terminal, &mut app).await;

    // Stop event tasks before restoring terminal
    events.stop();
    // Brief yield to let tasks wind down
    tokio::task::yield_now().await;

    // Restore terminal
    disable_raw_mode()?;
    execute!(terminal.backend_mut(), LeaveAlternateScreen)?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }

    Ok(())
}

async fn run_app(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    app: &mut App,
) -> (io::Result<()>, EventHandler) {
    let mut events = EventHandler::new(50);

    loop {
        if let Err(e) = terminal.draw(|frame| ui::render(frame, &mut *app)) {
            return (Err(e), events);
        }

        match events.next().await {
            Some(AppEvent::Key(key)) => {
                // Any key press during boot skips to main view
                if app.view == View::Boot {
                    app.view = if app.engine.is_live() {
                        View::Sessions
                    } else {
                        View::Agent
                    };
                } else {
                    handle_key(app, key);
                }
            }
            Some(AppEvent::Tick) => {
                // Advance boot animation
                if app.view == View::Boot {
                    app.boot_ticks += 1;
                    app.engine.tick(app.session_locked);
                    // After boot: always go to session list in live mode
                    if app.boot_ticks >= 30 {
                        app.view = if app.engine.is_live() {
                            View::Sessions
                        } else {
                            View::Agent
                        };
                    }
                } else if !app.paused {
                    let should_lock = app.engine.tick(app.session_locked);
                    if should_lock {
                        app.session_locked = true;
                    }
                }
            }
            None => break,
        }

        if app.should_quit {
            break;
        }
    }

    (Ok(()), events)
}

fn handle_key(app: &mut App, key: KeyEvent) {
    // Global keys
    match key.code {
        KeyCode::Char('q') => {
            app.should_quit = true;
            return;
        }
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
            app.should_quit = true;
            return;
        }
        _ => {}
    }

    // Session list view
    if app.view == View::Sessions {
        // Rename mode: capture text input
        if let Some(ref mut buf) = app.rename_input {
            match key.code {
                KeyCode::Enter => {
                    let new_name = buf.clone();
                    if !new_name.is_empty() {
                        if let Some(live) = app.engine.live_engine_mut() {
                            live.rename_session(app.session_list_cursor, new_name);
                        }
                    }
                    app.rename_input = None;
                }
                KeyCode::Esc => {
                    app.rename_input = None;
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => {
                    buf.push(c);
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Down => {
                if let Some(live) = app.engine.live_engine() {
                    let indices: Vec<usize> = live.active_sessions().map(|(i, _)| i).collect();
                    if let Some(pos) = indices.iter().position(|&i| i == app.session_list_cursor) {
                        app.session_list_cursor = indices[(pos + 1) % indices.len()];
                    } else if let Some(&first) = indices.first() {
                        app.session_list_cursor = first;
                    }
                }
            }
            KeyCode::Up => {
                if let Some(live) = app.engine.live_engine() {
                    let indices: Vec<usize> = live.active_sessions().map(|(i, _)| i).collect();
                    if let Some(pos) = indices.iter().position(|&i| i == app.session_list_cursor) {
                        app.session_list_cursor = indices[(pos + indices.len() - 1) % indices.len()];
                    } else if let Some(&last) = indices.last() {
                        app.session_list_cursor = last;
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(live) = app.engine.live_engine_mut() {
                    live.active_idx = app.session_list_cursor;
                }
                app.session_locked = true;
                app.view = View::Agent;
                app.focused_pane = 0;
                app.pane_scrolls.clear();
            }
            KeyCode::Char('r') => {
                // Start rename with current name pre-filled
                if let Some(live) = app.engine.live_engine() {
                    if let Some(session) = live.sessions.get(app.session_list_cursor) {
                        app.rename_input = Some(session.name.clone());
                    }
                }
            }
            _ => {}
        }
        return;
    }

    // n/p cycle sessions in live mode (agent view only)
    if app.engine.is_live() {
        match key.code {
            KeyCode::Char('n') => {
                if let Some(live) = app.engine.live_engine_mut() {
                    live.next_session();
                    app.session_list_cursor = live.active_idx;
                    app.session_locked = true;
                    app.focused_pane = 0;
                    app.pane_scrolls.clear();
                }
                return;
            }
            KeyCode::Char('p') => {
                if let Some(live) = app.engine.live_engine_mut() {
                    live.prev_session();
                    app.session_list_cursor = live.active_idx;
                    app.session_locked = true;
                    app.focused_pane = 0;
                    app.pane_scrolls.clear();
                }
                return;
            }
            _ => {}
        }
    }

    // Agent view keys
    match key.code {
        KeyCode::Char(' ') => {
            app.paused = !app.paused;
        }
        KeyCode::Char('r') => {
            if !app.engine.is_live() {
                app.reset();
            }
        }
        KeyCode::Esc => {
            if app.engine.is_live() {
                app.session_locked = false;
                app.view = View::Sessions;
                if let Some(live) = app.engine.live_engine() {
                    app.session_list_cursor = live.active_idx;
                }
            }
        }
        KeyCode::Left => {
            if app.focused_pane > 0 {
                app.focused_pane -= 1;
            }
        }
        KeyCode::Right => {
            let agent_count = app.engine.agents().len();
            if app.focused_pane + 1 < agent_count {
                app.focused_pane += 1;
            }
        }
        KeyCode::Down => {
            let cur = app.scroll_offset();
            app.set_scroll_offset(cur.saturating_add(1));
        }
        KeyCode::Up => {
            let cur = app.scroll_offset();
            app.set_scroll_offset(cur.saturating_sub(1));
        }
        _ => {}
    }
}
