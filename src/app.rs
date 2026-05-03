use std::collections::HashMap;
use std::path::PathBuf;

use crate::engine::Engine;
use crate::live::LiveEngine;
use crate::mock::MockEngine;

#[derive(PartialEq)]
pub enum View {
    Boot,
    Sessions,
    Agent,
}

pub struct App {
    pub engine: Engine,
    pub should_quit: bool,
    pub paused: bool,
    pub focused_pane: usize,
    pub session_locked: bool,
    pub session_list_cursor: usize,
    pub boot_ticks: u16,
    pub view: View,
    /// Per-pane scroll offsets (preserved across pane switches)
    pub pane_scrolls: HashMap<usize, u16>,
    /// Rename input state: Some(buffer) when actively renaming
    pub rename_input: Option<String>,
    /// Pane column ranges (x_start, x_end) set during render for mouse hit-testing
    pub pane_columns: Vec<(u16, u16)>,
}

impl App {
    pub fn new_mock() -> Self {
        Self {
            engine: Engine::Mock(MockEngine::new()),
            should_quit: false,
            paused: false,
            focused_pane: 0,
            session_locked: true,
            session_list_cursor: 0,
            boot_ticks: 0,
            view: View::Boot,
            pane_scrolls: HashMap::new(),
            rename_input: None,
            pane_columns: Vec::new(),
        }
    }

    pub fn new_live(threads_dir: PathBuf) -> Self {
        Self {
            engine: Engine::Live(LiveEngine::new(threads_dir)),
            should_quit: false,
            paused: false,
            focused_pane: 0,
            session_locked: false,
            session_list_cursor: 0,
            boot_ticks: 0,
            view: View::Boot,
            pane_scrolls: HashMap::new(),
            rename_input: None,
            pane_columns: Vec::new(),
        }
    }

    pub fn scroll_offset(&self) -> u16 {
        *self.pane_scrolls.get(&self.focused_pane).unwrap_or(&0)
    }

    pub fn set_scroll_offset(&mut self, val: u16) {
        self.pane_scrolls.insert(self.focused_pane, val);
    }

    pub fn reset(&mut self) {
        self.engine.reset();
        self.paused = false;
        self.pane_scrolls.clear();
        self.focused_pane = 0;
    }
}
