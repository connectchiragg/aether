pub mod event;

use crate::model::{
    compute_provider_cost, Agent, AgentCost, AgentStatus, Message, MessageType, TurnMetrics,
    TurnUsage, UsageStats,
};
use crate::provider::{
    claude_projects_dir, claude_threads_dir, codex_sessions_dir, AetherConfig, ProviderKind,
    ProviderStatus,
};
use crate::theme;
use event::LiveEvent;
use std::collections::{HashMap, HashSet};
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
    pub usage: UsageStats,
    pub provider: ProviderKind,
    pub source: String,
    file_pos: u64,
    next_message_id: usize,
    color_idx: usize,
    partial_line: String,
    pub file_found: bool,
    pub last_modified: u64,
    /// Whether we've already resolved the session name from native format
    native_name_resolved: bool,
    /// Tracks current turn's prompt for usage accumulation
    current_turn_prompt: String,
    /// Running total of input context
    cumulative_context: u64,
    /// Set of already-scanned sub-agent meta file paths
    scanned_subagent_files: HashSet<PathBuf>,
    /// Maps agent IDs to canonical agent IDs (for grouping same-type agents)
    id_aliases: HashMap<String, String>,
    current_codex_turn_id: Option<String>,
    current_codex_model: Option<String>,
    seen_codex_user_messages: HashSet<String>,
    last_codex_response: Option<String>,
}

#[derive(Clone, Debug)]
pub struct TurnMarker {
    pub turn_index: usize,
    pub prompt: String,
    /// Index into messages vec where this turn starts
    pub message_start_idx: usize,
}

impl SessionState {
    fn new(file_path: PathBuf, provider: ProviderKind) -> Self {
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
            usage: UsageStats::default(),
            provider,
            source: provider.source_label().to_string(),
            file_pos: 0,
            next_message_id: 0,
            color_idx: 0,
            partial_line: String::new(),
            file_found: false,
            last_modified: 0,
            native_name_resolved: false,
            current_turn_prompt: String::new(),
            cumulative_context: 0,
            scanned_subagent_files: HashSet::new(),
            id_aliases: HashMap::new(),
            current_codex_turn_id: None,
            current_codex_model: None,
            seen_codex_user_messages: HashSet::new(),
            last_codex_response: None,
        }
    }

    fn clear_display(&mut self) {
        self.agents.clear();
        self.messages.clear();
        self.turns.clear();
        self.next_message_id = 0;
        self.color_idx = 0;
        self.id_aliases.clear();
        self.current_codex_turn_id = None;
        self.current_codex_model = None;
        self.seen_codex_user_messages.clear();
        self.last_codex_response = None;
    }

    /// Resolve an agent ID through aliases (same agent type → same column)
    fn resolve_id(&self, id: &str) -> String {
        self.id_aliases
            .get(id)
            .cloned()
            .unwrap_or_else(|| id.to_string())
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

        // Re-scan sub-agents when new ones appear
        if !self.usage.turns.is_empty() {
            self.scan_subagents();
        }
    }

    fn process_line(&mut self, line: &str) {
        match self.provider {
            ProviderKind::Claude => self.process_claude_line(line),
            ProviderKind::Codex => self.process_codex_line(line),
        }
    }

    fn process_claude_line(&mut self, line: &str) {
        // Try our hook event format first
        let event: LiveEvent = match serde_json::from_str(line) {
            Ok(e) => e,
            Err(_) => {
                // Try native Claude Code format for session name extraction
                self.try_native_line(line);
                return;
            }
        };

        match event {
            LiveEvent::SessionStart {
                session_id, name, ..
            } => {
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
            LiveEvent::TurnStart {
                turn_index, prompt, ..
            } => {
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
            LiveEvent::Message {
                from, to, content, ..
            } => {
                let resolved_from = self.resolve_id(&from);
                let resolved_to = self.resolve_id(&to);
                self.ensure_agent(&resolved_from, "Parent");
                self.ensure_agent(&resolved_to, "Agent");

                let id = self.next_message_id;
                self.next_message_id += 1;
                let mut msg = Message::new(
                    id,
                    &resolved_from,
                    &resolved_to,
                    &content,
                    MessageType::Response,
                );
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

    /// Extract session info from native Claude Code JSONL format.
    /// Parses names, usage data, and turn boundaries.
    fn try_native_line(&mut self, line: &str) {
        // Quick filter: only parse lines that might be relevant
        let dominated_name = self.name_override.is_some();
        let dominated_name_native = dominated_name || self.native_name_resolved;

        if dominated_name_native
            && !line.contains("\"assistant\"")
            && !line.contains("\"user\"")
            && !line.contains("custom-title")
            && !line.contains("ai-title")
            && !line.contains("turn-metrics")
        {
            return;
        }

        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return,
        };

        let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        match line_type {
            "custom-title" => {
                // Explicit title (user /rename) — highest priority after user rename
                if !dominated_name {
                    if let Some(title) = v.get("customTitle").and_then(|t| t.as_str()) {
                        self.name = title.to_string();
                        self.native_name_resolved = true;
                    }
                }
            }
            "ai-title" => {
                // Auto-generated title by Claude Code (Haiku)
                if !dominated_name {
                    if let Some(title) = v.get("aiTitle").and_then(|t| t.as_str()) {
                        self.name = title.to_string();
                        self.native_name_resolved = true;
                    }
                }
            }
            "agent-name" => {
                // Ignored for naming — only custom-title and ai-title are used
            }
            "user" => {
                // Only process real user prompts, not tool results or system messages.
                // Real prompts have "userType": "external" and string content.
                let user_type = v.get("userType").and_then(|u| u.as_str()).unwrap_or("");
                if user_type != "external" {
                    return;
                }

                if let Some(content) = v
                    .get("message")
                    .and_then(|m| m.get("content"))
                    .and_then(|c| c.as_str())
                {
                    // Skip system/meta messages and tool call descriptions
                    let trimmed = content.trim_start();
                    if trimmed.starts_with('<')
                        || trimmed.starts_with("Tool: ")
                        || trimmed.starts_with("Working Directory:")
                    {
                        return;
                    }

                    if !dominated_name_native {
                        let preview: String = content
                            .chars()
                            .filter(|c| !c.is_control())
                            .take(40)
                            .collect();
                        if !preview.is_empty() {
                            self.name = preview;
                            // Lock fallback name so later prompts don't overwrite it.
                            // custom-title will still override if it appears later.
                            self.native_name_resolved = true;
                        }
                    }
                    // Start a new turn
                    let prompt: String = content.chars().filter(|c| !c.is_control()).collect();
                    let timestamp = v
                        .get("timestamp")
                        .and_then(|t| t.as_str())
                        .unwrap_or("")
                        .to_string();
                    self.current_turn_prompt = prompt;
                    // Create a new turn entry
                    self.usage.turns.push(TurnUsage {
                        prompt: self.current_turn_prompt.clone(),
                        timestamp,
                        input_tokens: 0,
                        output_tokens: 0,
                        cache_read_tokens: 0,
                        cache_write_tokens: 0,
                        cost: 0.0,
                        agents: Vec::new(),
                        cumulative_context: self.cumulative_context,
                        context_saved: 0,
                        metrics: None,
                        response_text: String::new(),
                        cost_known: false,
                    });
                }
            }
            "assistant" => {
                // Accumulate usage from assistant messages
                if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                    let input = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let output = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let cache_write = usage
                        .get("cache_creation_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let cache_read = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    let model = v.get("model").and_then(|m| m.as_str()).unwrap_or("sonnet");

                    let cost = compute_provider_cost(
                        self.provider,
                        model,
                        input,
                        output,
                        cache_write,
                        cache_read,
                    );
                    self.cumulative_context += input + cache_read + cache_write;

                    // Add to current turn (last one)
                    if let Some(turn) = self.usage.turns.last_mut() {
                        turn.input_tokens += input;
                        turn.output_tokens += output;
                        turn.cache_read_tokens += cache_read;
                        turn.cache_write_tokens += cache_write;
                        if let Some(cost) = cost {
                            turn.cost += cost;
                            turn.cost_known = true;
                        }
                        turn.cumulative_context = self.cumulative_context;
                    }
                }
                // Capture assistant response text for metrics analysis
                if let Some(content) = v.get("message").and_then(|m| m.get("content")) {
                    if let Some(arr) = content.as_array() {
                        for block in arr {
                            if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                                if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                    if let Some(turn) = self.usage.turns.last_mut() {
                                        if !turn.response_text.is_empty() {
                                            turn.response_text.push('\n');
                                        }
                                        // Keep first 2000 chars per turn for analysis
                                        let remaining =
                                            2000usize.saturating_sub(turn.response_text.len());
                                        if remaining > 0 {
                                            turn.response_text.extend(text.chars().take(remaining));
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            "turn-metrics" => {
                // Metrics written by the aether Stop hook.
                // Match to the last turn that doesn't have metrics yet.
                let metrics = TurnMetrics {
                    friction: v.get("friction").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                    hallucination: v
                        .get("hallucination")
                        .and_then(|x| x.as_f64())
                        .unwrap_or(0.0) as f32,
                    confidence: v.get("confidence").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                    acceptance: v.get("acceptance").and_then(|x| x.as_f64()).unwrap_or(0.0) as f32,
                    performance: v.get("performance").and_then(|x| x.as_f64()).unwrap_or(0.0)
                        as f32,
                    recap: v
                        .get("recap")
                        .and_then(|x| x.as_str())
                        .unwrap_or("")
                        .to_string(),
                };
                // Find last turn without metrics (the one this score belongs to)
                if let Some(turn) = self
                    .usage
                    .turns
                    .iter_mut()
                    .rev()
                    .find(|t| t.metrics.is_none())
                {
                    turn.metrics = Some(metrics);
                }
            }
            _ => {}
        }
    }

    /// Extract a useful observability stream from Codex rollout JSONL.
    fn process_codex_line(&mut self, line: &str) {
        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return,
        };

        let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");
        let timestamp = v
            .get("timestamp")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        match line_type {
            "session_meta" => {
                if let Some(payload) = v.get("payload") {
                    if let Some(id) = payload.get("id").and_then(|id| id.as_str()) {
                        self.session_id = id.to_string();
                    }
                    if self.name_override.is_none() {
                        if let Some(cwd) = payload.get("cwd").and_then(|cwd| cwd.as_str()) {
                            self.name = PathBuf::from(cwd)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("Codex session")
                                .to_string();
                            self.native_name_resolved = true;
                        }
                    }
                }
            }
            "turn_context" => {
                let payload = v.get("payload").unwrap_or(&v);
                if let Some(model) = payload
                    .get("model")
                    .or_else(|| v.get("model"))
                    .and_then(|model| model.as_str())
                {
                    self.current_codex_model = Some(model.to_string());
                }
                if self.name_override.is_none() && !self.native_name_resolved {
                    if let Some(cwd) = payload
                        .get("cwd")
                        .or_else(|| v.get("cwd"))
                        .and_then(|cwd| cwd.as_str())
                    {
                        self.name = PathBuf::from(cwd)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("Codex session")
                            .to_string();
                    }
                }
            }
            "response_item" => self.process_codex_response_item(v.get("payload"), &timestamp),
            "event_msg" => self.process_codex_event_msg(v.get("payload"), &timestamp),
            _ => {}
        }
    }

    fn process_codex_response_item(
        &mut self,
        payload: Option<&serde_json::Value>,
        timestamp: &str,
    ) {
        let Some(payload) = payload else {
            return;
        };
        match payload.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "message" => {
                let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let text = content_text(payload.get("content"));
                if text.trim().is_empty() {
                    return;
                }
                match role {
                    "user" => {
                        if self.usage.turns.is_empty()
                            && !is_synthetic_codex_user_message(&text)
                            && !self.seen_codex_user_messages.contains(&text)
                        {
                            self.start_codex_turn(text, timestamp.to_string(), None)
                        }
                    }
                    "assistant" => self.push_codex_response(text),
                    _ => {}
                }
            }
            "function_call" | "custom_tool_call" | "web_search_call" => {
                let name = payload
                    .get("name")
                    .or_else(|| payload.get("status"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool call");
                self.push_observed_message(
                    "tool".to_string(),
                    "codex".to_string(),
                    format!(
                        "{} ({})",
                        name,
                        payload
                            .get("type")
                            .and_then(|t| t.as_str())
                            .unwrap_or("tool")
                    ),
                );
            }
            "function_call_output" | "custom_tool_call_output" => {
                if let Some(output) = payload.get("output").and_then(value_preview) {
                    self.push_observed_message("tool".to_string(), "codex".to_string(), output);
                }
            }
            "reasoning" => {
                if let Some(text) = content_text_opt(payload.get("summary"))
                    .or_else(|| content_text_opt(payload.get("content")))
                {
                    if !text.trim().is_empty() {
                        self.push_observed_message(
                            "reasoning".to_string(),
                            "codex".to_string(),
                            text,
                        );
                    }
                }
            }
            _ => {}
        }
    }

    fn process_codex_event_msg(&mut self, payload: Option<&serde_json::Value>, timestamp: &str) {
        let Some(payload) = payload else {
            return;
        };
        match payload.get("type").and_then(|t| t.as_str()).unwrap_or("") {
            "task_started" => {
                self.current_codex_turn_id = payload
                    .get("turn_id")
                    .and_then(|id| id.as_str())
                    .map(|id| id.to_string());
            }
            "user_message" => {
                if let Some(message) = payload.get("message").and_then(|m| m.as_str()) {
                    if !is_synthetic_codex_user_message(message)
                        && !self.seen_codex_user_messages.contains(message)
                    {
                        self.start_codex_turn(
                            message.to_string(),
                            timestamp.to_string(),
                            self.current_codex_turn_id.clone(),
                        );
                    }
                }
            }
            "agent_message" => {
                if let Some(message) = payload.get("message").and_then(|m| m.as_str()) {
                    if !self.is_duplicate_codex_response(message) {
                        self.push_codex_response(message.to_string());
                    }
                }
            }
            "token_count" => self.apply_codex_usage(payload),
            "task_complete" => {
                if let Some(message) = payload.get("last_agent_message").and_then(|m| m.as_str()) {
                    if self
                        .usage
                        .turns
                        .last()
                        .map(|t| t.response_text.is_empty())
                        .unwrap_or(false)
                    {
                        self.push_codex_response(message.to_string());
                    }
                }
            }
            "patch_apply_end" | "web_search_end" | "item_completed" => {
                let label = payload
                    .get("type")
                    .and_then(|t| t.as_str())
                    .unwrap_or("event");
                let status = payload
                    .get("status")
                    .or_else(|| payload.get("action"))
                    .and_then(|s| s.as_str())
                    .unwrap_or("done");
                self.push_observed_message(
                    "tool".to_string(),
                    "codex".to_string(),
                    format!("{label} ({status})"),
                );
            }
            _ => {}
        }
    }

    fn start_codex_turn(&mut self, prompt: String, timestamp: String, turn_id: Option<String>) {
        if self.usage.turns.last().map(|t| t.prompt.as_str()) == Some(prompt.as_str()) {
            return;
        }
        self.seen_codex_user_messages.insert(prompt.clone());
        self.current_codex_turn_id = turn_id;
        self.last_codex_response = None;
        if self.name_override.is_none() && !self.native_name_resolved {
            let preview: String = prompt
                .chars()
                .filter(|c| !c.is_control())
                .take(40)
                .collect();
            if !preview.is_empty() {
                self.name = preview;
                self.native_name_resolved = true;
            }
        }

        self.current_turn_prompt = prompt.clone();
        self.turns.push(TurnMarker {
            turn_index: self.usage.turns.len() + 1,
            prompt: prompt.clone(),
            message_start_idx: self.messages.len(),
        });
        self.usage.turns.push(TurnUsage {
            prompt: prompt.clone(),
            timestamp,
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cache_write_tokens: 0,
            cost: 0.0,
            agents: Vec::new(),
            cumulative_context: self.cumulative_context,
            context_saved: 0,
            metrics: None,
            response_text: String::new(),
            cost_known: false,
        });
        self.push_observed_message("user".to_string(), "codex".to_string(), prompt);
    }

    fn push_codex_response(&mut self, text: String) {
        if text.trim().is_empty() {
            return;
        }
        if self.is_duplicate_codex_response(&text) {
            return;
        }
        if let Some(turn) = self.usage.turns.last_mut() {
            if !turn.response_text.is_empty() {
                turn.response_text.push('\n');
            }
            turn.response_text.push_str(&text);
        }
        self.last_codex_response = Some(text.clone());
        self.push_observed_message("codex".to_string(), "user".to_string(), text);
    }

    fn is_duplicate_codex_response(&self, text: &str) -> bool {
        self.last_codex_response.as_deref() == Some(text)
            || self
                .usage
                .turns
                .last()
                .map(|turn| turn.response_text.ends_with(text))
                .unwrap_or(false)
    }

    fn apply_codex_usage(&mut self, payload: &serde_json::Value) {
        let usage = payload
            .get("info")
            .and_then(|i| i.get("last_token_usage"))
            .or_else(|| payload.get("info").and_then(|i| i.get("total_token_usage")));
        let Some(usage) = usage else {
            return;
        };

        let input = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let output = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let cached = usage
            .get("cached_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let total = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);

        let has_priced_token_split = input > 0 || output > 0 || cached > 0;
        let cost = if has_priced_token_split {
            self.current_codex_model.as_deref().and_then(|model| {
                compute_provider_cost(self.provider, model, input, output, 0, cached)
            })
        } else {
            None
        };

        if let Some(turn) = self.usage.turns.last_mut() {
            if input == 0 && output == 0 && cached == 0 && total > 0 {
                turn.output_tokens += total;
                self.cumulative_context += total;
            } else {
                turn.input_tokens += input;
                turn.output_tokens += output;
                turn.cache_read_tokens += cached;
                self.cumulative_context += input + cached;
            }
            turn.cumulative_context = self.cumulative_context;
            if let Some(cost) = cost {
                turn.cost += cost;
                turn.cost_known = true;
            }
        }
    }

    /// Scan sub-agent files in <session-id>/subagents/ and correlate to turns.
    /// Incremental: only processes newly appeared agent files.
    fn scan_subagents(&mut self) {
        // Sub-agent dir is next to the session file: <session-id>/subagents/
        let session_stem = self
            .file_path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("")
            .to_string();
        let subagents_dir = self
            .file_path
            .parent()
            .map(|p| p.join(&session_stem).join("subagents"))
            .unwrap_or_default();

        if !subagents_dir.exists() {
            return;
        }

        let entries = match fs::read_dir(&subagents_dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        // Collect meta.json paths, skip already-scanned ones
        let mut new_paths: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if !path.to_string_lossy().ends_with(".meta.json") {
                continue;
            }
            if !self.scanned_subagent_files.contains(&path) {
                new_paths.push(path);
            }
        }

        if new_paths.is_empty() {
            return;
        }

        for path in new_paths {
            // Read meta
            let meta: serde_json::Value = match fs::read_to_string(&path) {
                Ok(data) => match serde_json::from_str(&data) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };

            let agent_type = meta
                .get("agentType")
                .and_then(|v| v.as_str())
                .unwrap_or("agent")
                .to_string();
            let description = meta
                .get("description")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            // Read corresponding jsonl — skip if not ready yet (retry next tick)
            let jsonl_path = path.to_string_lossy().replace(".meta.json", ".jsonl");
            let jsonl_path = PathBuf::from(jsonl_path);
            if !jsonl_path.exists() {
                continue;
            }

            let mut first_ts: Option<String> = None;
            let mut input_tokens: u64 = 0;
            let mut output_tokens: u64 = 0;
            let mut cache_read: u64 = 0;
            let mut cache_write: u64 = 0;
            let mut model = String::from("sonnet");
            let mut agent_prompt = String::new();
            let mut response_parts: Vec<String> = Vec::new();

            if let Ok(file_content) = fs::read_to_string(&jsonl_path) {
                for line in file_content.lines() {
                    let v: serde_json::Value = match serde_json::from_str(line) {
                        Ok(v) => v,
                        Err(_) => continue,
                    };

                    if first_ts.is_none() {
                        if let Some(ts) = v.get("timestamp").and_then(|t| t.as_str()) {
                            first_ts = Some(ts.to_string());
                        }
                    }

                    let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

                    if line_type == "user" && agent_prompt.is_empty() {
                        // Extract the initial prompt given to the agent
                        if let Some(c) = v.get("message").and_then(|m| m.get("content")) {
                            if let Some(s) = c.as_str() {
                                agent_prompt = s.to_string();
                            }
                        }
                    }

                    if line_type == "assistant" {
                        if let Some(m) = v.get("model").and_then(|m| m.as_str()) {
                            model = m.to_string();
                        }
                        if let Some(usage) = v.get("message").and_then(|m| m.get("usage")) {
                            input_tokens += usage
                                .get("input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            output_tokens += usage
                                .get("output_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            cache_read += usage
                                .get("cache_read_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                            cache_write += usage
                                .get("cache_creation_input_tokens")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0);
                        }
                        // Extract text from assistant response
                        if let Some(content) = v.get("message").and_then(|m| m.get("content")) {
                            if let Some(arr) = content.as_array() {
                                for block in arr {
                                    if block.get("type").and_then(|t| t.as_str()) == Some("text") {
                                        if let Some(text) =
                                            block.get("text").and_then(|t| t.as_str())
                                        {
                                            if !text.is_empty() {
                                                response_parts.push(text.to_string());
                                            }
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Don't mark as scanned until we have an assistant response
            if response_parts.is_empty() && output_tokens == 0 {
                continue;
            }

            // Mark as scanned now that we have data
            self.scanned_subagent_files.insert(path.clone());

            let cost = compute_provider_cost(
                self.provider,
                &model,
                input_tokens,
                output_tokens,
                cache_write,
                cache_read,
            )
            .unwrap_or(0.0);
            let agent_cost = AgentCost {
                name: if description.is_empty() {
                    agent_type
                } else {
                    format!(
                        "{}: {}",
                        agent_type,
                        if description.len() > 30 {
                            format!("{}...", &description[..27])
                        } else {
                            description
                        }
                    )
                },
                cost,
                input_tokens,
                output_tokens,
                prompt: agent_prompt,
                response_preview: response_parts.join("\n\n"),
            };

            // Correlate to turn: find the last turn that started before the agent
            // ISO timestamps sort lexicographically
            if let Some(ts) = &first_ts {
                let mut best_turn_idx = 0;
                for (i, turn) in self.usage.turns.iter().enumerate() {
                    if !turn.timestamp.is_empty() && turn.timestamp.as_str() <= ts.as_str() {
                        best_turn_idx = i;
                    }
                }
                if let Some(turn) = self.usage.turns.get_mut(best_turn_idx) {
                    turn.agents.push(agent_cost);
                    turn.context_saved += input_tokens + cache_read;
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

    fn push_observed_message(&mut self, from: String, to: String, content: String) {
        self.ensure_agent(&from, "Observed");
        self.ensure_agent(&to, "Observed");
        let id = self.next_message_id;
        self.next_message_id += 1;
        let mut message = Message::new(id, &from, &to, &content, MessageType::Response);
        message.revealed_chars = message.content.len();
        self.messages.push(message);
    }
}

/// Maximum number of sessions to track (most recent by mtime).
const MAX_SESSIONS: usize = 50;

/// Manages multiple provider session files.
pub struct LiveEngine {
    pub sessions: Vec<SessionState>,
    pub active_idx: usize,
    pub active_provider: Option<ProviderKind>,
    pub provider_cursor: usize,
    dir_override: Option<PathBuf>,
    scan_cooldown: u32,
    /// Persisted name overrides: file stem → custom name
    name_overrides: HashMap<String, String>,
    config: AetherConfig,
}

impl LiveEngine {
    pub fn new(provider: Option<ProviderKind>, dir_override: Option<PathBuf>) -> Self {
        let config = AetherConfig::load();
        let name_overrides = Self::load_name_overrides();
        Self {
            sessions: Vec::new(),
            active_idx: 0,
            active_provider: provider,
            provider_cursor: provider
                .and_then(|p| {
                    ProviderKind::ALL
                        .iter()
                        .position(|candidate| *candidate == p)
                })
                .unwrap_or(0),
            dir_override,
            scan_cooldown: 0,
            name_overrides,
            config,
        }
    }

    fn overrides_path() -> PathBuf {
        crate::provider::config_path()
            .parent()
            .map(|p| p.join("session-names.json"))
            .unwrap_or_else(|| PathBuf::from(".session-names.json"))
    }

    fn load_name_overrides() -> HashMap<String, String> {
        let path = Self::overrides_path();
        match fs::read_to_string(&path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => HashMap::new(),
        }
    }

    fn save_name_overrides(&self) {
        let path = Self::overrides_path();
        if let Ok(data) = serde_json::to_string_pretty(&self.name_overrides) {
            if let Some(parent) = path.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(path, data);
        }
    }

    pub fn rename_session(&mut self, session_idx: usize, new_name: String) {
        if let Some(session) = self.sessions.get_mut(session_idx) {
            let key = Self::session_override_key(session.provider, &session.file_path);

            session.name = new_name.clone();
            session.name_override = Some(new_name);

            // Persist
            self.name_overrides.insert(key, session.name.clone());
            self.save_name_overrides();
        }
    }

    fn session_override_key(provider: ProviderKind, path: &std::path::Path) -> String {
        let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        format!("{}:{}", provider.id(), stem)
    }

    pub fn tick(&mut self, session_locked: bool) -> bool {
        // Scan for new session files every ~2 seconds (40 ticks at 50ms)
        self.scan_cooldown = self.scan_cooldown.wrapping_add(1);
        if self.scan_cooldown % 40 == 0 || self.sessions.is_empty() {
            self.scan_sessions();
        }

        // Poll active session every tick, others every ~5 seconds
        for (i, session) in self.sessions.iter_mut().enumerate() {
            if i == self.active_idx || self.scan_cooldown % 100 == 0 {
                session.poll_file();
            }
        }

        // Auto-switch to most recently modified session (only when not locked)
        if !session_locked && self.sessions.len() > 1 {
            let provider = self.active_provider;
            let most_recent = self
                .sessions
                .iter()
                .enumerate()
                .filter(|(_, s)| provider.map(|p| s.provider == p).unwrap_or(true))
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
        let providers: Vec<ProviderKind> = if let Some(provider) = self.active_provider {
            vec![provider]
        } else {
            ProviderKind::ALL.to_vec()
        };

        for provider in providers {
            if let Some(dir) = self.dir_override.clone() {
                self.scan_directory(&dir, provider, provider == ProviderKind::Codex);
                continue;
            }

            match provider {
                ProviderKind::Claude => {
                    self.scan_directory(&claude_threads_dir(), provider, false);
                    let projects_dir = claude_projects_dir();
                    if let Ok(projects) = fs::read_dir(&projects_dir) {
                        for project in projects.flatten() {
                            let project_path = project.path();
                            if !project_path.is_dir() {
                                continue;
                            }
                            let dir_name = project_path
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or("");
                            if dir_name.starts_with("-private-tmp")
                                || dir_name.starts_with("-tmp")
                                || dir_name.contains("worktrees-")
                            {
                                continue;
                            }
                            self.scan_directory(&project_path, provider, false);
                        }
                    }
                }
                ProviderKind::Codex => {
                    self.scan_directory(&codex_sessions_dir(), provider, true);
                }
            }
        }

        // Remove sessions whose files no longer exist
        self.sessions.retain(|s| s.file_path.exists());

        // Sort by last_modified descending so most recent sessions appear first
        self.sessions
            .sort_by(|a, b| b.last_modified.cmp(&a.last_modified));

        // Cap to latest N sessions
        let active_path = self
            .sessions
            .get(self.active_idx)
            .map(|s| s.file_path.clone());
        self.sessions.truncate(MAX_SESSIONS);

        // Keep active_idx pointing to the same session if possible
        if let Some(path) = active_path {
            if let Some(pos) = self.sessions.iter().position(|s| s.file_path == path) {
                self.active_idx = pos;
            } else {
                self.active_idx = 0;
            }
        } else if self.active_idx >= self.sessions.len() {
            self.active_idx = self.sessions.len().saturating_sub(1);
        }
    }

    fn scan_directory(&mut self, dir: &PathBuf, provider: ProviderKind, recursive: bool) {
        let entries = match fs::read_dir(dir) {
            Ok(e) => e,
            Err(_) => return,
        };

        for entry in entries.flatten() {
            let path = entry.path();
            if recursive && path.is_dir() {
                self.scan_directory(&path, provider, true);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }
            // Skip helper scripts
            if path.file_name().and_then(|n| n.to_str()) == Some("tui-log.py") {
                continue;
            }
            // Skip files inside subagent directories
            if path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                == Some("subagents")
            {
                continue;
            }

            let already_tracked = self.sessions.iter().any(|s| s.file_path == path);
            if !already_tracked {
                let mut session = SessionState::new(path.clone(), provider);
                if let Ok(meta) = fs::metadata(&path) {
                    if let Ok(modified) = meta.modified() {
                        session.last_modified = modified
                            .duration_since(SystemTime::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0);
                    }
                }
                // Apply persisted name override
                let key = Self::session_override_key(provider, &path);
                if let Some(custom_name) = self.name_overrides.get(&key) {
                    session.name = custom_name.clone();
                    session.name_override = Some(custom_name.clone());
                }
                self.sessions.push(session);
            }
        }
    }

    pub fn next_session(&mut self) {
        let indices = self.active_session_indices();
        if indices.is_empty() {
            return;
        }
        let pos = indices
            .iter()
            .position(|idx| *idx == self.active_idx)
            .unwrap_or(0);
        for offset in 1..=indices.len() {
            let idx = indices[(pos + offset) % indices.len()];
            let s = &self.sessions[idx];
            if !s.agents.is_empty() || !s.messages.is_empty() {
                self.active_idx = idx;
                return;
            }
        }
    }

    pub fn prev_session(&mut self) {
        let indices = self.active_session_indices();
        if indices.is_empty() {
            return;
        }
        let pos = indices
            .iter()
            .position(|idx| *idx == self.active_idx)
            .unwrap_or(0);
        for offset in 1..=indices.len() {
            let idx = indices[(pos + indices.len() - offset) % indices.len()];
            let s = &self.sessions[idx];
            if !s.agents.is_empty() || !s.messages.is_empty() {
                self.active_idx = idx;
                return;
            }
        }
    }

    // Convenience accessors for the active session
    pub fn agents(&self) -> &[Agent] {
        self.active_session()
            .map(|s| s.agents.as_slice())
            .unwrap_or(&[])
    }

    pub fn messages(&self) -> &[Message] {
        self.active_session()
            .map(|s| s.messages.as_slice())
            .unwrap_or(&[])
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

    /// Sessions for the selected provider, already sorted by last_modified descending.
    pub fn active_sessions(&self) -> impl Iterator<Item = (usize, &SessionState)> {
        let provider = self.active_provider;
        self.sessions
            .iter()
            .enumerate()
            .filter(move |(_, session)| provider.map(|p| session.provider == p).unwrap_or(true))
    }

    pub fn active_session_name(&self) -> &str {
        self.active_session()
            .map(|s| s.name.as_str())
            .unwrap_or("none")
    }

    pub fn provider_statuses(&self) -> Vec<ProviderStatus> {
        ProviderKind::ALL
            .iter()
            .map(|provider| {
                let session_count = self
                    .sessions
                    .iter()
                    .filter(|session| session.provider == *provider)
                    .count();
                let last_modified = self
                    .sessions
                    .iter()
                    .filter(|session| session.provider == *provider)
                    .map(|session| session.last_modified)
                    .max()
                    .unwrap_or(0);
                ProviderStatus {
                    kind: *provider,
                    enabled: self.config.is_enabled(*provider),
                    available: self.provider_available(*provider),
                    session_count,
                    last_modified,
                }
            })
            .collect()
    }

    pub fn select_provider(&mut self, provider: ProviderKind) {
        self.active_provider = Some(provider);
        self.provider_cursor = ProviderKind::ALL
            .iter()
            .position(|candidate| *candidate == provider)
            .unwrap_or(0);
        let first_idx = self.active_session_indices().first().copied();
        if let Some(idx) = first_idx {
            self.active_idx = idx;
        }
    }

    pub fn clear_provider(&mut self) {
        self.active_provider = None;
    }

    fn active_session_indices(&self) -> Vec<usize> {
        self.active_sessions().map(|(idx, _)| idx).collect()
    }

    fn provider_available(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::Claude => claude_threads_dir().exists() || claude_projects_dir().exists(),
            ProviderKind::Codex => codex_sessions_dir().exists(),
        }
    }
}

fn content_text(value: Option<&serde_json::Value>) -> String {
    content_text_opt(value).unwrap_or_default()
}

fn content_text_opt(value: Option<&serde_json::Value>) -> Option<String> {
    let value = value?;
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if let Some(text) = value.get("text").and_then(|t| t.as_str()) {
        return Some(text.to_string());
    }
    if let Some(arr) = value.as_array() {
        let parts: Vec<String> = arr
            .iter()
            .filter_map(|item| {
                item.get("text")
                    .and_then(|text| text.as_str())
                    .or_else(|| item.get("content").and_then(|text| text.as_str()))
                    .map(|text| text.to_string())
            })
            .collect();
        if !parts.is_empty() {
            return Some(parts.join("\n"));
        }
    }
    None
}

fn value_preview(value: &serde_json::Value) -> Option<String> {
    if let Some(text) = value.as_str() {
        return Some(text.to_string());
    }
    if value.is_null() {
        return None;
    }
    let text = value.to_string();
    Some(text.chars().take(500).collect())
}

fn is_synthetic_codex_user_message(text: &str) -> bool {
    let trimmed = text.trim_start();
    trimmed.starts_with('<')
        || trimmed.starts_with("Tool: ")
        || trimmed.starts_with("Working Directory:")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codex_rollout_lines_create_turn_response_and_usage() {
        let mut session = SessionState::new(
            PathBuf::from("rollout-2026-05-31T00-00-00-test.jsonl"),
            ProviderKind::Codex,
        );

        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"session_meta","payload":{"id":"codex-session","cwd":"/tmp/project","model_provider":"openai"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"write a test"}]}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:02Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"done"}]}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":10,"cached_input_tokens":2,"output_tokens":5,"total_tokens":17}}}}"#,
        );

        assert_eq!(session.session_id, "codex-session");
        assert_eq!(session.name, "project");
        assert_eq!(session.usage.turn_count(), 1);
        assert_eq!(session.usage.turns[0].prompt, "write a test");
        assert_eq!(session.usage.turns[0].response_text, "done");
        assert_eq!(session.usage.turns[0].input_tokens, 10);
        assert_eq!(session.usage.turns[0].cache_read_tokens, 2);
        assert_eq!(session.usage.turns[0].output_tokens, 5);
        assert!(!session.usage.turns[0].cost_known);
    }

    #[test]
    fn codex_usage_is_priced_when_model_is_known() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);

        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"turn_context","payload":{"cwd":"/tmp/project","model":"gpt-5.5"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"price this turn"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"total_tokens":1100}}}}"#,
        );

        let turn = &session.usage.turns[0];
        let expected = (800.0 * 5.0 + 200.0 * 0.50 + 100.0 * 30.0) / 1_000_000.0;
        assert!(turn.cost_known);
        assert!((turn.cost - expected).abs() < f64::EPSILON);
    }

    #[test]
    fn codex_unknown_lines_are_ignored() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"unknown","payload":{"x":1}}"#,
        );
        assert_eq!(session.usage.turn_count(), 0);
        assert!(session.messages.is_empty());
    }

    #[test]
    fn codex_ignores_synthetic_user_records_and_uses_event_messages_for_turns() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);

        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/tmp/project</cwd>\n</environment_context>"}]}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:01Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:02Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"first real prompt"}]}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:02Z","type":"event_msg","payload":{"type":"user_message","message":"first real prompt"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:03Z","type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"first answer"}]}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:03Z","type":"event_msg","payload":{"type":"agent_message","message":"first answer"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:04Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-2"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:05Z","type":"event_msg","payload":{"type":"user_message","message":"second prompt"}}"#,
        );

        assert_eq!(session.usage.turn_count(), 2);
        assert_eq!(session.usage.turns[0].prompt, "first real prompt");
        assert_eq!(session.usage.turns[0].response_text, "first answer");
        assert_eq!(session.usage.turns[1].prompt, "second prompt");
        assert_eq!(
            session
                .messages
                .iter()
                .filter(|m| m.from == "codex")
                .count(),
            1
        );
    }
}
