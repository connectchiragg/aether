pub mod event;

use crate::model::{Agent, AgentStatus, Message, MessageType};
use crate::theme;
use event::LiveEvent;
use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::PathBuf;
use std::time::SystemTime;

/// Represents a single session's state and file reader.
pub struct SessionState {
    pub session_id: String,
    pub name: String,
    /// User-set name override (persisted, takes priority over JSONL name)
    pub name_override: Option<String>,
    pub agents: Vec<Agent>,
    pub messages: Vec<Message>,
    pub turns: Vec<TurnMarker>,
    pub file_path: PathBuf,
    file_pos: u64,
    next_message_id: usize,
    color_idx: usize,
    partial_line: String,
    pub file_found: bool,
    pub last_modified: u64,
    /// Maps agent IDs to canonical agent IDs (for grouping same-type agents)
    id_aliases: HashMap<String, String>,
}

#[derive(Clone, Debug)]
pub struct TurnMarker {
    pub turn_index: usize,
    pub prompt: String,
    /// Index into messages vec where this turn starts
    pub message_start_idx: usize,
}

impl SessionState {
    fn new(file_path: PathBuf) -> Self {
        let session_id = file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        Self {
            session_id: session_id.clone(),
            name: session_id,
            name_override: None,
            agents: Vec::new(),
            messages: Vec::new(),
            turns: Vec::new(),
            file_path,
            file_pos: 0,
            next_message_id: 0,
            color_idx: 0,
            partial_line: String::new(),
            file_found: false,
            last_modified: 0,
            id_aliases: HashMap::new(),
        }
    }

    fn clear_display(&mut self) {
        self.agents.clear();
        self.messages.clear();
        self.turns.clear();
        self.next_message_id = 0;
        self.color_idx = 0;
        self.id_aliases.clear();
    }

    /// Resolve an agent ID through aliases (same agent type → same column)
    fn resolve_id(&self, id: &str) -> String {
        self.id_aliases.get(id).cloned().unwrap_or_else(|| id.to_string())
    }

    fn poll_file(&mut self) {
        let file = match File::open(&self.file_path) {
            Ok(f) => {
                self.file_found = true;
                f
            }
            Err(_) => {
                self.file_found = false;
                return;
            }
        };

        // Track modification time
        if let Ok(meta) = file.metadata() {
            if let Ok(modified) = meta.modified() {
                self.last_modified = modified
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|d| d.as_secs())
                    .unwrap_or(0);
            }

            // Detect file truncation
            let file_len = meta.len();
            if file_len < self.file_pos {
                self.clear_display();
                self.file_pos = 0;
                self.partial_line.clear();
            }
        }

        let mut reader = BufReader::new(file);
        if reader.seek(SeekFrom::Start(self.file_pos)).is_err() {
            return;
        }

        let mut buf = String::new();
        while reader.read_line(&mut buf).unwrap_or(0) > 0 {
            if buf.ends_with('\n') {
                let line = if self.partial_line.is_empty() {
                    buf.trim().to_string()
                } else {
                    let full = format!("{}{}", self.partial_line, buf.trim());
                    self.partial_line.clear();
                    full
                };

                if !line.is_empty() {
                    self.process_line(&line);
                }
            } else {
                self.partial_line.push_str(&buf);
            }
            buf.clear();
        }

        self.file_pos = reader.stream_position().unwrap_or(self.file_pos);
    }

    fn process_line(&mut self, line: &str) {
        let event: LiveEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => return,
        };

        match event {
            LiveEvent::SessionStart { session_id, name, .. } => {
                // A new session_start in the same file means the old session was cleared
                if self.session_id != session_id && !self.messages.is_empty() {
                    self.clear_display();
                }
                self.session_id = session_id;
                // Only update name from JSONL if user hasn't set a custom name
                if self.name_override.is_none() {
                    if let Some(n) = name {
                        self.name = n;
                    }
                }
            }
            LiveEvent::TurnStart { turn_index, prompt, .. } => {
                let marker = TurnMarker {
                    turn_index,
                    prompt: prompt.unwrap_or_default(),
                    message_start_idx: self.messages.len(),
                };
                self.turns.push(marker);
            }
            LiveEvent::SessionClear { .. } => {
                self.clear_display();
            }
            LiveEvent::AgentSpawn { id, name, role, .. } => {
                // Group by agent name (type) — reuse existing column
                if let Some(existing) = self.agents.iter_mut().find(|a| a.name == name) {
                    // Alias this new ID to the existing agent's ID
                    self.id_aliases.insert(id, existing.id.clone());
                    existing.role = role;
                    existing.status = AgentStatus::Idle;
                    return;
                }
                // Exact ID match (refresh)
                if let Some(agent) = self.agents.iter_mut().find(|a| a.id == id) {
                    agent.name = name;
                    agent.role = role;
                    agent.status = AgentStatus::Idle;
                    return;
                }
                let color = theme::AGENT_COLORS[self.color_idx % theme::AGENT_COLORS.len()];
                self.color_idx += 1;
                let agent = Agent::new(&id, &name, &role, color, vec![]);
                self.agents.push(agent);
            }
            LiveEvent::Message { from, to, content, .. } => {
                let resolved_from = self.resolve_id(&from);
                let resolved_to = self.resolve_id(&to);
                self.ensure_agent(&resolved_from, "Parent");
                self.ensure_agent(&resolved_to, "Agent");

                let id = self.next_message_id;
                self.next_message_id += 1;
                let mut msg = Message::new(id, &resolved_from, &resolved_to, &content, MessageType::Response);
                msg.revealed_chars = msg.content.len();
                self.messages.push(msg);
            }
            LiveEvent::AgentDone { id, .. } => {
                let resolved = self.resolve_id(&id);
                if let Some(agent) = self.agents.iter_mut().find(|a| a.id == resolved) {
                    agent.status = AgentStatus::Idle;
                }
            }
        }
    }

    fn ensure_agent(&mut self, id: &str, default_role: &str) {
        if self.agents.iter().any(|a| a.id == id) {
            return;
        }
        let color = theme::AGENT_COLORS[self.color_idx % theme::AGENT_COLORS.len()];
        self.color_idx += 1;
        let display_name = if id == "parent" {
            "Claude (Parent)".to_string()
        } else {
            id.to_string()
        };
        let agent = Agent::new(id, &display_name, default_role, color, vec![]);
        self.agents.push(agent);
    }
}

/// Manages multiple sessions, scanning the threads directory.
pub struct LiveEngine {
    pub sessions: Vec<SessionState>,
    pub active_idx: usize,
    threads_dir: PathBuf,
    scan_cooldown: u32,
    /// Persisted name overrides: file stem → custom name
    name_overrides: HashMap<String, String>,
}

impl LiveEngine {
    pub fn new(threads_dir: PathBuf) -> Self {
        let name_overrides = Self::load_name_overrides(&threads_dir);
        Self {
            sessions: Vec::new(),
            active_idx: 0,
            threads_dir,
            scan_cooldown: 0,
            name_overrides,
        }
    }

    fn overrides_path(threads_dir: &PathBuf) -> PathBuf {
        threads_dir.join(".session-names.json")
    }

    fn load_name_overrides(threads_dir: &PathBuf) -> HashMap<String, String> {
        let path = Self::overrides_path(threads_dir);
        match fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    fn save_name_overrides(&self) {
        let path = Self::overrides_path(&self.threads_dir);
        if let Ok(data) = serde_json::to_string_pretty(&self.name_overrides) {
            let _ = fs::write(path, data);
        }
    }

    pub fn rename_session(&mut self, session_idx: usize, new_name: String) {
        if let Some(session) = self.sessions.get_mut(session_idx) {
            let file_stem = session.file_path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_string();

            session.name = new_name.clone();
            session.name_override = Some(new_name);

            // Persist
            self.name_overrides.insert(file_stem, session.name.clone());
            self.save_name_overrides();
        }
    }

    pub fn tick(&mut self, session_locked: bool) -> bool {
        // Scan for new session files every ~2 seconds (40 ticks at 50ms)
        self.scan_cooldown = self.scan_cooldown.wrapping_add(1);
        if self.scan_cooldown % 40 == 0 || self.sessions.is_empty() {
            self.scan_sessions();
        }

        // Poll all sessions for new data
        for session in &mut self.sessions {
            session.poll_file();
        }

        // Auto-switch to most recently modified session (only when not locked)
        if !session_locked && self.sessions.len() > 1 {
            let most_recent = self
                .sessions
                .iter()
                .enumerate()
                .max_by_key(|(_, s)| s.last_modified);
            if let Some((idx, _)) = most_recent {
                if idx != self.active_idx && self.sessions[idx].last_modified > 0 {
                    self.active_idx = idx;
                }
            }
        }

        false
    }

    pub fn reset(&mut self) {
        if let Some(session) = self.sessions.get_mut(self.active_idx) {
            session.clear_display();
            session.file_pos = 0;
            session.partial_line.clear();
        }
    }

    fn scan_sessions(&mut self) {
        let entries = match fs::read_dir(&self.threads_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // Skip the helper script
            if path.file_name().and_then(|n| n.to_str()) == Some("tui-log.py") {
                continue;
            }

            let already_tracked = self.sessions.iter().any(|s| s.file_path == path);
            if !already_tracked {
                let mut session = SessionState::new(path.clone());
                // Apply persisted name override
                let stem = path.file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("");
                if let Some(custom_name) = self.name_overrides.get(stem) {
                    session.name = custom_name.clone();
                    session.name_override = Some(custom_name.clone());
                }
                self.sessions.push(session);
            }
        }

        // Remove sessions whose files no longer exist
        self.sessions.retain(|s| s.file_path.exists());

        // Keep active_idx in bounds
        if self.active_idx >= self.sessions.len() {
            self.active_idx = self.sessions.len().saturating_sub(1);
        }
    }

    pub fn next_session(&mut self) {
        let len = self.sessions.len();
        if len == 0 { return; }
        for i in 1..=len {
            let idx = (self.active_idx + i) % len;
            let s = &self.sessions[idx];
            if !s.agents.is_empty() || !s.messages.is_empty() {
                self.active_idx = idx;
                return;
            }
        }
    }

    pub fn prev_session(&mut self) {
        let len = self.sessions.len();
        if len == 0 { return; }
        for i in 1..=len {
            let idx = (self.active_idx + len - i) % len;
            let s = &self.sessions[idx];
            if !s.agents.is_empty() || !s.messages.is_empty() {
                self.active_idx = idx;
                return;
            }
        }
    }

    // Convenience accessors for the active session
    pub fn agents(&self) -> &[Agent] {
        self.active_session().map(|s| s.agents.as_slice()).unwrap_or(&[])
    }

    pub fn messages(&self) -> &[Message] {
        self.active_session().map(|s| s.messages.as_slice()).unwrap_or(&[])
    }

    pub fn active_session(&self) -> Option<&SessionState> {
        self.sessions.get(self.active_idx)
    }

    pub fn file_found(&self) -> bool {
        self.active_session().map(|s| s.file_found).unwrap_or(false)
    }

    pub fn session_count(&self) -> usize {
        self.active_sessions().count()
    }

    /// Sessions that have actual content (agents or messages).
    pub fn active_sessions(&self) -> impl Iterator<Item = (usize, &SessionState)> {
        self.sessions.iter().enumerate().filter(|(_, s)| !s.agents.is_empty() || !s.messages.is_empty())
    }

    pub fn active_session_name(&self) -> &str {
        self.active_session().map(|s| s.name.as_str()).unwrap_or("none")
    }
}
