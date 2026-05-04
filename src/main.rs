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
    event::{KeyCode, KeyEvent, KeyModifiers, EnableMouseCapture, DisableMouseCapture},
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
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
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
    execute!(terminal.backend_mut(), LeaveAlternateScreen, DisableMouseCapture)?;
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
            Some(AppEvent::MouseScroll { column, up }) => {
                if app.view == View::Sessions {
                    // Mouse scroll moves the cursor in sessions list
                    if let Some(live) = app.engine.live_engine() {
                        let indices: Vec<usize> = live.active_sessions().map(|(i, _)| i).collect();
                        if let Some(pos) = indices.iter().position(|&i| i == app.session_list_cursor) {
                            let new_pos = if up {
                                pos.saturating_sub(1)
                            } else {
                                (pos + 1).min(indices.len().saturating_sub(1))
                            };
                            app.session_list_cursor = indices[new_pos];
                        }
                    }
                } else if app.view == View::Graph {
                    // Scroll the detail panel
                    let cur = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0);
                    let new_val = if up {
                        cur.saturating_sub(3)
                    } else {
                        cur.saturating_add(3)
                    };
                    app.pane_scrolls.insert(usize::MAX, new_val);
                } else if app.view == View::Agent {
                    // Find which pane the mouse is over
                    let pane = app.pane_columns.iter().position(|(x_start, x_end)| {
                        column >= *x_start && column < *x_end
                    });
                    if let Some(pane_idx) = pane {
                        let cur = *app.pane_scrolls.get(&pane_idx).unwrap_or(&0);
                        let max = app.pane_max_scrolls.get(&pane_idx).copied().unwrap_or(0);
                        let new_val = if up {
                            cur.saturating_sub(3)
                        } else {
                            cur.saturating_add(3).min(max)
                        };
                        app.pane_scrolls.insert(pane_idx, new_val);
                    }
                }
            }
            Some(AppEvent::Tick) => {
                app.tick = app.tick.wrapping_add(1);
                // Advance boot animation
                if app.view == View::Boot {
                    app.boot_ticks += 1;
                    app.engine.tick(app.session_locked);
                    // After boot: always go to session list in live mode
                    if app.boot_ticks >= 54 {
                        app.view = if app.engine.is_live() {
                            View::Sessions
                        } else {
                            View::Agent
                        };
                    }
                } else if !app.paused {
                    // Track turn count before tick for auto-follow
                    let prev_turn_count = if app.view == View::Graph {
                        app.engine.live_engine()
                            .and_then(|e| e.sessions.get(e.active_idx))
                            .map(|s| s.usage.turn_count())
                            .unwrap_or(0)
                    } else { 0 };
                    let was_on_last = app.view == View::Graph
                        && app.selected_dot >= prev_turn_count.saturating_sub(1);

                    let should_lock = app.engine.tick(app.session_locked);
                    if should_lock {
                        app.session_locked = true;
                    }

                    // Auto-follow: if user was on last dot, stay on last dot
                    if was_on_last {
                        let new_count = app.engine.live_engine()
                            .and_then(|e| e.sessions.get(e.active_idx))
                            .map(|s| s.usage.turn_count())
                            .unwrap_or(0);
                        if new_count > prev_turn_count {
                            app.selected_dot = new_count.saturating_sub(1);
                        }
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
                app.view = View::Graph;
                // Start at last turn
                let turn_count = app.engine.live_engine()
                    .and_then(|e| e.sessions.get(e.active_idx))
                    .map(|s| s.usage.turn_count())
                    .unwrap_or(0);
                app.selected_dot = turn_count.saturating_sub(1);
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

    // Graph view keys
    if app.view == View::Graph {
        let turn_count = app.engine.live_engine()
            .and_then(|e| e.sessions.get(e.active_idx))
            .map(|s| s.usage.turn_count())
            .unwrap_or(0);
        let max_dot = turn_count.saturating_sub(1);

        // Jump-to-turn input mode
        if let Some(ref mut buf) = app.graph_jump_input {
            match key.code {
                KeyCode::Char(c) if c.is_ascii_digit() => {
                    buf.push(c);
                }
                KeyCode::Enter => {
                    if let Ok(n) = buf.parse::<usize>() {
                        let target = n.saturating_sub(1).min(max_dot);
                        app.selected_dot = target;
                        app.pane_scrolls.insert(usize::MAX, 0);
                    }
                    app.graph_jump_input = None;
                }
                KeyCode::Esc | KeyCode::Backspace => {
                    if buf.is_empty() || key.code == KeyCode::Esc {
                        app.graph_jump_input = None;
                    } else {
                        buf.pop();
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Left => {
                app.selected_dot = app.selected_dot.saturating_sub(1);
                app.pane_scrolls.insert(usize::MAX, 0);
            }
            KeyCode::Right => {
                app.selected_dot = (app.selected_dot + 1).min(max_dot);
                app.pane_scrolls.insert(usize::MAX, 0);
            }
            // First turn
            KeyCode::Home | KeyCode::Char('h') => {
                app.selected_dot = 0;
                app.pane_scrolls.insert(usize::MAX, 0);
            }
            // Latest turn
            KeyCode::End | KeyCode::Char('l') => {
                app.selected_dot = max_dot;
                app.pane_scrolls.insert(usize::MAX, 0);
            }
            // g = go to turn number
            KeyCode::Char('g') => {
                app.graph_jump_input = Some(String::new());
            }
            KeyCode::Down => {
                // Switch to next session
                if let Some(live) = app.engine.live_engine_mut() {
                    let len = live.sessions.len();
                    if len > 0 {
                        live.active_idx = (live.active_idx + 1) % len;
                        app.session_list_cursor = live.active_idx;
                    }
                }
                let turn_count = app.engine.live_engine()
                    .and_then(|e| e.sessions.get(e.active_idx))
                    .map(|s| s.usage.turn_count())
                    .unwrap_or(0);
                app.selected_dot = turn_count.saturating_sub(1);
                app.pane_scrolls.clear();
            }
            KeyCode::Up => {
                // Switch to previous session
                if let Some(live) = app.engine.live_engine_mut() {
                    let len = live.sessions.len();
                    if len > 0 {
                        live.active_idx = (live.active_idx + len - 1) % len;
                        app.session_list_cursor = live.active_idx;
                    }
                }
                let turn_count = app.engine.live_engine()
                    .and_then(|e| e.sessions.get(e.active_idx))
                    .map(|s| s.usage.turn_count())
                    .unwrap_or(0);
                app.selected_dot = turn_count.saturating_sub(1);
                app.pane_scrolls.clear();
            }
            KeyCode::Esc => {
                app.session_locked = false;
                app.view = View::Sessions;
                if let Some(live) = app.engine.live_engine() {
                    app.session_list_cursor = live.active_idx;
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
            let max = app.pane_max_scrolls.get(&app.focused_pane).copied().unwrap_or(0);
            app.set_scroll_offset(cur.saturating_add(1).min(max));
        }
        KeyCode::Up => {
            let cur = app.scroll_offset();
            app.set_scroll_offset(cur.saturating_sub(1));
        }
        _ => {}
    }
}
