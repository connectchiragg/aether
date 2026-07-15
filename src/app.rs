use std::collections::HashMap;
use std::path::PathBuf;

use crate::engine::Engine;
use crate::live::LiveEngine;
use crate::mock::MockEngine;
use crate::provider::ProviderKind;

#[derive(PartialEq)]
pub enum View {
    Boot,
    Providers,
    Sessions,
    Agent,
    Graph,
}

pub struct App {
    pub engine: Engine,
    pub should_quit: bool,
    pub paused: bool,
    pub focused_pane: usize,
    pub session_locked: bool,
    pub provider_list_cursor: usize,
    pub session_list_cursor: usize,
    pub boot_ticks: u16,
    /// Global tick counter (increments every frame, wraps)
    pub tick: u32,
    pub view: View,
    /// Per-pane scroll offsets (preserved across pane switches)
    pub pane_scrolls: HashMap<usize, u16>,
    /// Scroll offset for the sessions list view
    pub session_list_scroll: u16,
    /// Selected dot index in the graph view
    pub selected_dot: usize,
    /// Stable first turn in the graph viewport; moves only at a viewport edge.
    pub graph_window_start: usize,
    /// Last horizontal graph navigation: -1 for left, 1 for right.
    pub graph_navigation_direction: i8,
    /// Jump-to-turn input buffer in graph view (typing a number)
    pub graph_jump_input: Option<String>,
    /// Rename input state: Some(buffer) when actively renaming
    pub rename_input: Option<String>,
    /// Pane column ranges (x_start, x_end) set during render for mouse hit-testing
    pub pane_columns: Vec<(u16, u16)>,
    /// Per-pane max scroll (estimated during render)
    pub pane_max_scrolls: HashMap<usize, u16>,
    /// Expanded view: 'u' = full user prompt, 'a' = full agent response, None = normal
    pub expanded_view: Option<char>,
}

impl App {
    pub fn new_mock() -> Self {
        Self {
            engine: Engine::Mock(MockEngine::new()),
            should_quit: false,
            paused: false,
            focused_pane: 0,
            session_locked: true,
            provider_list_cursor: 0,
            session_list_cursor: 0,
            boot_ticks: 0,
            tick: 0,
            view: View::Boot,
            pane_scrolls: HashMap::new(),
            session_list_scroll: 0,
            selected_dot: 0,
            graph_window_start: 0,
            graph_navigation_direction: 1,
            graph_jump_input: None,
            rename_input: None,
            pane_columns: Vec::new(),
            pane_max_scrolls: HashMap::new(),
            expanded_view: None,
        }
    }

    pub fn new_live(provider: Option<ProviderKind>, dir: Option<PathBuf>) -> Self {
        Self::with_live_engine(LiveEngine::new(provider, dir))
    }

    fn with_live_engine(engine: LiveEngine) -> Self {
        Self {
            engine: Engine::Live(engine),
            should_quit: false,
            paused: false,
            focused_pane: 0,
            session_locked: false,
            provider_list_cursor: 0,
            session_list_cursor: 0,
            boot_ticks: 0,
            tick: 0,
            view: View::Boot,
            pane_scrolls: HashMap::new(),
            session_list_scroll: 0,
            selected_dot: 0,
            graph_window_start: 0,
            graph_navigation_direction: 1,
            graph_jump_input: None,
            rename_input: None,
            pane_columns: Vec::new(),
            pane_max_scrolls: HashMap::new(),
            expanded_view: None,
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
        self.graph_window_start = 0;
        self.graph_navigation_direction = 1;
    }
}
