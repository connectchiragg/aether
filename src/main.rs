#![allow(dead_code)]

mod app;
mod engine;
mod event;
mod live;
mod mock;
mod model;
mod provider;
mod theme;
mod ui;

use app::{App, View};
use clap::{Parser, Subcommand};
use crossterm::{
    event::{
        DisableFocusChange, DisableMouseCapture, EnableFocusChange, EnableMouseCapture, KeyCode,
        KeyEvent, KeyModifiers,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use event::{AppEvent, EventHandler};
use provider::{AetherConfig, ProviderKind};
use ratatui::prelude::*;
use std::io;
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "aether",
    about = "See the invisible — live agent observability"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Watch enabled provider sessions
    Watch {
        /// Override the provider session directory
        #[arg(short, long)]
        dir: Option<PathBuf>,
    },
    /// Enable observability for a provider
    Setup { provider: Option<ProviderKind> },
}

#[tokio::main]
async fn main() -> io::Result<()> {
    let cli = Cli::parse();

    let mut app = match cli.command {
        Some(Commands::Setup { provider }) => {
            return run_setup(provider);
        }
        Some(Commands::Watch { dir }) => App::new_live(None, dir),
        None => {
            eprintln!("Usage: aether <command>\n\n  aether setup <provider>   Enable a provider\n  aether watch              Choose a provider and watch sessions\n");
            return Ok(());
        }
    };

    // Setup terminal
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableFocusChange
    )?;
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
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture,
        DisableFocusChange
    )?;
    terminal.show_cursor()?;

    if let Err(e) = result {
        eprintln!("Error: {e}");
    }

    Ok(())
}

fn run_setup(provider: Option<ProviderKind>) -> io::Result<()> {
    match provider {
        None => return print_setup_status(),
        Some(ProviderKind::Codex) => return run_setup_codex(),
        Some(ProviderKind::Claude) => {}
    }

    let eye = "\x1b[31m⠑⠽⠑\x1b[0m";
    println!();
    println!("  {} \x1b[1;31maether\x1b[0m setup", eye);
    println!("  \x1b[2m─────────────────────\x1b[0m");
    println!();

    let mut config = AetherConfig::load();
    config.enable(ProviderKind::Claude);
    config.save()?;

    println!("  \x1b[31m●\x1b[0m Claude provider enabled");
    println!("  \x1b[2m─────────────────────\x1b[0m");
    println!("  \x1b[1;31m✓\x1b[0m Claude setup complete");
    println!();
    println!(
        "  \x1b[2mAether will watch\x1b[0m  \x1b[1m{}\x1b[0m",
        provider::claude_projects_dir().display()
    );
    println!("  \x1b[2mRun\x1b[0m  \x1b[1maether watch\x1b[0m          \x1b[2mto choose a provider\x1b[0m");
    println!("  \x1b[2mNo Claude hooks or additional model calls are installed\x1b[0m");
    println!();

    Ok(())
}

fn print_setup_status() -> io::Result<()> {
    let config = AetherConfig::load();
    println!("aether setup");
    println!();
    for provider in ProviderKind::ALL {
        let enabled = if config.is_enabled(provider) {
            "enabled"
        } else {
            "not enabled"
        };
        println!("  {:<8} {}", provider.id(), enabled);
    }
    println!();
    println!("Run `aether setup claude` or `aether setup codex`.");
    Ok(())
}

fn run_setup_codex() -> io::Result<()> {
    let mut config = AetherConfig::load();
    config.enable(ProviderKind::Codex);
    config.save()?;

    println!();
    println!("  \x1b[31m●\x1b[0m Codex provider enabled");
    println!();
    println!(
        "  \x1b[2mAether will watch\x1b[0m  \x1b[1m{}\x1b[0m",
        provider::codex_sessions_dir().display()
    );
    println!("  \x1b[2mRun\x1b[0m  \x1b[1maether watch\x1b[0m         \x1b[2mto choose a provider\x1b[0m");
    println!();
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
                if key.code == KeyCode::Char('l') && key.modifiers.contains(KeyModifiers::CONTROL) {
                    if let Err(error) = force_terminal_redraw(terminal) {
                        return (Err(error), events);
                    }
                    continue;
                }
                // Any key press during boot skips to main view
                if app.view == View::Boot {
                    app.view = if let Some(live) = app.engine.live_engine() {
                        if live.active_provider.is_some() {
                            View::Sessions
                        } else {
                            View::Providers
                        }
                    } else {
                        View::Agent
                    };
                } else {
                    handle_key(app, key);
                }
            }
            Some(AppEvent::MouseScroll { column, up }) => {
                if app.view == View::Providers {
                    if let Some(live) = app.engine.live_engine() {
                        let count = live.provider_statuses().len();
                        if count > 0 {
                            app.provider_list_cursor = if up {
                                app.provider_list_cursor.saturating_sub(1)
                            } else {
                                (app.provider_list_cursor + 1).min(count.saturating_sub(1))
                            };
                        }
                    }
                } else if app.view == View::Sessions {
                    // Mouse scroll moves the cursor in sessions list
                    if let Some(live) = app.engine.live_engine() {
                        let indices: Vec<usize> = live.active_sessions().map(|(i, _)| i).collect();
                        if let Some(pos) =
                            indices.iter().position(|&i| i == app.session_list_cursor)
                        {
                            let new_pos = if up {
                                pos.saturating_sub(1)
                            } else {
                                (pos + 1).min(indices.len().saturating_sub(1))
                            };
                            app.session_list_cursor = indices[new_pos];
                        }
                    }
                } else if app.view == View::Graph {
                    // Scroll the complete graph + transcript document.
                    let cur = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0);
                    let max = app.pane_max_scrolls.get(&usize::MAX).copied().unwrap_or(0);
                    let new_val = if up {
                        cur.saturating_sub(3)
                    } else {
                        cur.saturating_add(3).min(max)
                    };
                    app.pane_scrolls.insert(usize::MAX, new_val);
                } else if app.view == View::Agent {
                    // Find which pane the mouse is over
                    let pane = app
                        .pane_columns
                        .iter()
                        .position(|(x_start, x_end)| column >= *x_start && column < *x_end);
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
            Some(AppEvent::Redraw) => {
                if let Err(error) = force_terminal_redraw(terminal) {
                    return (Err(error), events);
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
                        app.view = if let Some(live) = app.engine.live_engine() {
                            if live.active_provider.is_some() {
                                View::Sessions
                            } else {
                                View::Providers
                            }
                        } else {
                            View::Agent
                        };
                    }
                } else if !app.paused {
                    // Track turn count before tick for auto-follow
                    let prev_turn_count = if app.view == View::Graph {
                        app.engine
                            .live_engine()
                            .and_then(|e| e.sessions.get(e.active_idx))
                            .map(|s| s.usage.turn_count())
                            .unwrap_or(0)
                    } else {
                        0
                    };
                    let was_on_last = app.view == View::Graph
                        && app.selected_dot >= prev_turn_count.saturating_sub(1);

                    let should_lock = app.engine.tick(app.session_locked);
                    if should_lock {
                        app.session_locked = true;
                    }

                    // Auto-follow: if user was on last dot, stay on last dot
                    if was_on_last {
                        let new_count = app
                            .engine
                            .live_engine()
                            .and_then(|e| e.sessions.get(e.active_idx))
                            .map(|s| s.usage.turn_count())
                            .unwrap_or(0);
                        if new_count > prev_turn_count {
                            app.selected_dot = new_count.saturating_sub(1);
                            app.graph_navigation_direction = 1;
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

fn force_terminal_redraw<B: Backend>(terminal: &mut Terminal<B>) -> io::Result<()> {
    terminal.autoresize()?;
    terminal.clear()
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

    if app.view == View::Providers {
        match key.code {
            KeyCode::Down | KeyCode::Right => {
                if let Some(live) = app.engine.live_engine() {
                    let count = live.provider_statuses().len();
                    if count > 0 {
                        app.provider_list_cursor = (app.provider_list_cursor + 1) % count;
                    }
                }
            }
            KeyCode::Up | KeyCode::Left => {
                if let Some(live) = app.engine.live_engine() {
                    let count = live.provider_statuses().len();
                    if count > 0 {
                        app.provider_list_cursor = (app.provider_list_cursor + count - 1) % count;
                    }
                }
            }
            KeyCode::Enter => {
                if let Some(live) = app.engine.live_engine_mut() {
                    let statuses = live.provider_statuses();
                    if let Some(status) = statuses.get(app.provider_list_cursor) {
                        live.select_provider(status.kind);
                        app.session_list_cursor = live.active_idx;
                        app.session_locked = false;
                        app.session_list_scroll = 0;
                        app.view = View::Sessions;
                    }
                }
            }
            _ => {}
        }
        return;
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
                        app.session_list_cursor =
                            indices[(pos + indices.len() - 1) % indices.len()];
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
                let turn_count = app
                    .engine
                    .live_engine()
                    .and_then(|e| e.sessions.get(e.active_idx))
                    .map(|s| s.usage.turn_count())
                    .unwrap_or(0);
                app.selected_dot = turn_count.saturating_sub(1);
                app.graph_navigation_direction = 1;
                app.focused_pane = 0;
                app.pane_scrolls.clear();
            }
            KeyCode::Esc => {
                if let Some(live) = app.engine.live_engine_mut() {
                    live.clear_provider();
                }
                app.view = View::Providers;
                app.session_locked = false;
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
        let turn_count = app
            .engine
            .live_engine()
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
                        app.graph_navigation_direction =
                            if target < app.selected_dot { -1 } else { 1 };
                        app.selected_dot = target;
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
                app.graph_navigation_direction = -1;
                app.selected_dot = app.selected_dot.saturating_sub(1);
            }
            KeyCode::Right => {
                app.graph_navigation_direction = 1;
                app.selected_dot = (app.selected_dot + 1).min(max_dot);
            }
            // First turn
            KeyCode::Home | KeyCode::Char('h') => {
                app.graph_navigation_direction = -1;
                app.selected_dot = 0;
            }
            // Latest turn
            KeyCode::End | KeyCode::Char('l') => {
                app.graph_navigation_direction = 1;
                app.selected_dot = max_dot;
            }
            // g = go to turn number
            KeyCode::Char('g') => {
                app.graph_jump_input = Some(String::new());
            }
            // e = expand/collapse all content (prompt + response + agent)
            KeyCode::Char('e') => {
                app.expanded_view = if app.expanded_view.is_some() {
                    None
                } else {
                    Some('e')
                };
            }
            KeyCode::Down | KeyCode::Char('j') => {
                let cur = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0);
                let max = app.pane_max_scrolls.get(&usize::MAX).copied().unwrap_or(0);
                app.pane_scrolls
                    .insert(usize::MAX, cur.saturating_add(1).min(max));
            }
            KeyCode::Up | KeyCode::Char('k') => {
                let cur = app.pane_scrolls.get(&usize::MAX).copied().unwrap_or(0);
                app.pane_scrolls.insert(usize::MAX, cur.saturating_sub(1));
            }
            KeyCode::Char('n') => {
                // Switch to next session
                if let Some(live) = app.engine.live_engine_mut() {
                    live.next_session();
                    app.session_list_cursor = live.active_idx;
                }
                let turn_count = app
                    .engine
                    .live_engine()
                    .and_then(|e| e.sessions.get(e.active_idx))
                    .map(|s| s.usage.turn_count())
                    .unwrap_or(0);
                app.selected_dot = turn_count.saturating_sub(1);
                app.graph_navigation_direction = 1;
                app.pane_scrolls.clear();
            }
            KeyCode::Char('p') => {
                // Switch to previous session
                if let Some(live) = app.engine.live_engine_mut() {
                    live.prev_session();
                    app.session_list_cursor = live.active_idx;
                }
                let turn_count = app
                    .engine
                    .live_engine()
                    .and_then(|e| e.sessions.get(e.active_idx))
                    .map(|s| s.usage.turn_count())
                    .unwrap_or(0);
                app.selected_dot = turn_count.saturating_sub(1);
                app.graph_navigation_direction = 1;
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
            let max = app
                .pane_max_scrolls
                .get(&app.focused_pane)
                .copied()
                .unwrap_or(0);
            app.set_scroll_offset(cur.saturating_add(1).min(max));
        }
        KeyCode::Up => {
            let cur = app.scroll_offset();
            app.set_scroll_offset(cur.saturating_sub(1));
        }
        _ => {}
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::*;
    use ratatui::{backend::TestBackend, widgets::Paragraph};

    #[test]
    fn watch_uses_one_provider_agnostic_command() {
        let cli = Cli::try_parse_from(["aether", "watch"]).unwrap();
        assert!(matches!(cli.command, Some(Commands::Watch { dir: None })));
        assert!(Cli::try_parse_from(["aether", "watch", "codex"]).is_err());
        assert!(Cli::try_parse_from(["aether", "watch", "claude"]).is_err());
    }

    #[test]
    fn forced_redraw_restores_terminal_contents_lost_outside_ratatui() {
        let backend = TestBackend::new(12, 1);
        let mut terminal = Terminal::new(backend).unwrap();
        let render =
            |frame: &mut Frame| frame.render_widget(Paragraph::new("aether"), frame.area());

        terminal.draw(render).unwrap();
        terminal.backend().assert_buffer_lines(["aether      "]);

        terminal.backend_mut().clear().unwrap();
        terminal.draw(render).unwrap();
        terminal.backend().assert_buffer_lines(["            "]);

        force_terminal_redraw(&mut terminal).unwrap();
        terminal.draw(render).unwrap();
        terminal.backend().assert_buffer_lines(["aether      "]);
    }
}
