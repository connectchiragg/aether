pub mod event;

use crate::model::{
    compute_provider_cost_at, compute_provider_cost_with_cache_ttl_at, estimate_tokens,
    model_context_window_at, Agent, AgentCost, AgentStatus, AttributionCategory, Message,
    MessageType, RequestAttribution, TurnAttribution, TurnOutcome, TurnTelemetry, TurnUsage,
    UsageStats,
};
use crate::provider::{
    claude_projects_dir, codex_session_index_path, codex_sessions_dir, AetherConfig, ProviderKind,
    ProviderStatus,
};
use crate::theme;
use chrono::{NaiveDate, Utc};
use event::LiveEvent;
use serde::Deserialize;
use std::collections::{HashMap, HashSet, VecDeque};
use std::fs::{self, File};
use std::io::{BufRead, BufReader, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::SystemTime;

/// Represents a single session's state and file reader.
pub struct SessionState {
    pub session_id: String,
    pub name: String,
    pub project_path: Option<PathBuf>,
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
    /// Timestamp of the latest provider event that represents actual session work.
    pub last_activity: u64,
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
    current_codex_task_first_turn_idx: Option<usize>,
    current_codex_model: Option<String>,
    codex_total_usage: CodexUsageSnapshot,
    pending_codex_user_echo: Option<String>,
    last_codex_response: Option<String>,
    pending_codex_documents: Vec<(String, u64)>,
    pending_codex_attribution: Vec<PendingCodexAttribution>,
    codex_subagent_files: Vec<CodexSubagentFileState>,
    pending_claude_diffs: HashMap<String, (usize, FileChangeStats)>,
    seen_claude_usage_messages: HashSet<String>,
    seen_claude_complexity_messages: HashSet<String>,
    seen_claude_tool_uses: HashSet<String>,
    attribution_history_tokens: u64,
    attribution_compaction_tokens: u64,
    attribution_compaction_source: Option<String>,
    attribution_committed_turns: usize,
    attribution_compaction_pending: bool,
    codex_tools: HashMap<String, ToolAttributionDescriptor>,
    claude_tool_names: HashMap<String, String>,
    attribution_loaded: bool,
}

#[derive(Clone, Copy, Debug, Default)]
struct CodexUsageSnapshot {
    input: u64,
    output: u64,
    cached: u64,
    reasoning: u64,
    total: u64,
}

#[derive(Clone, Debug)]
struct ToolAttributionDescriptor {
    category: AttributionCategory,
    source: String,
    purpose: String,
}

#[derive(Clone, Debug)]
struct PendingCodexAttribution {
    category: AttributionCategory,
    source: String,
    invocation: String,
    tokens: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct FileChangeStats {
    lines_added: u64,
    lines_removed: u64,
    files_created: u32,
    files_deleted: u32,
}

impl FileChangeStats {
    fn add(&mut self, other: Self) {
        self.lines_added = self.lines_added.saturating_add(other.lines_added);
        self.lines_removed = self.lines_removed.saturating_add(other.lines_removed);
        self.files_created = self.files_created.saturating_add(other.files_created);
        self.files_deleted = self.files_deleted.saturating_add(other.files_deleted);
    }

    fn observe_claude_result(
        &mut self,
        result_block: &serde_json::Value,
        record: &serde_json::Value,
    ) {
        let operation = result_block
            .get("toolUseResult")
            .or_else(|| record.get("toolUseResult"))
            .and_then(|result| result.get("type"))
            .and_then(|kind| kind.as_str());
        match operation {
            Some("create" | "add") => self.files_created = self.files_created.saturating_add(1),
            Some("delete" | "remove") => self.files_deleted = self.files_deleted.saturating_add(1),
            _ => {}
        }
    }
}

#[derive(Clone, Debug)]
struct CodexSubagentMetadata {
    session_id: String,
    parent_session_id: String,
    nickname: String,
    role: String,
    started_at: String,
}

struct CodexSubagentFileState {
    path: PathBuf,
    metadata: CodexSubagentMetadata,
    file_pos: u64,
    partial_line: String,
    model: Option<String>,
    prompt: String,
    response_preview: String,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cost: f64,
    cost_known: bool,
    outcome: TurnOutcome,
    duration_ms: Option<u64>,
    tool_calls: u32,
    lines_added: u64,
    lines_removed: u64,
    files_created: u32,
    files_deleted: u32,
    first_timestamp: String,
    last_response: Option<String>,
    total_usage: CodexUsageSnapshot,
    inherited_turn_ids: HashSet<String>,
    observing_child_turns: bool,
    attribution: TurnAttribution,
}

impl CodexSubagentFileState {
    fn new(
        path: PathBuf,
        metadata: CodexSubagentMetadata,
        inherited_turn_ids: HashSet<String>,
    ) -> Self {
        let observing_child_turns = inherited_turn_ids.is_empty();
        Self {
            path,
            metadata,
            file_pos: 0,
            partial_line: String::new(),
            model: None,
            prompt: String::new(),
            response_preview: String::new(),
            input_tokens: 0,
            output_tokens: 0,
            cache_read_tokens: 0,
            cost: 0.0,
            cost_known: false,
            outcome: TurnOutcome::InProgress,
            duration_ms: None,
            tool_calls: 0,
            lines_added: 0,
            lines_removed: 0,
            files_created: 0,
            files_deleted: 0,
            first_timestamp: String::new(),
            last_response: None,
            total_usage: CodexUsageSnapshot::default(),
            inherited_turn_ids,
            observing_child_turns,
            attribution: TurnAttribution::default(),
        }
    }
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
            project_path: None,
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
            last_activity: 0,
            native_name_resolved: false,
            current_turn_prompt: String::new(),
            cumulative_context: 0,
            scanned_subagent_files: HashSet::new(),
            id_aliases: HashMap::new(),
            current_codex_turn_id: None,
            current_codex_task_first_turn_idx: None,
            current_codex_model: None,
            codex_total_usage: CodexUsageSnapshot::default(),
            pending_codex_user_echo: None,
            last_codex_response: None,
            pending_codex_documents: Vec::new(),
            pending_codex_attribution: Vec::new(),
            codex_subagent_files: Vec::new(),
            pending_claude_diffs: HashMap::new(),
            seen_claude_usage_messages: HashSet::new(),
            seen_claude_complexity_messages: HashSet::new(),
            seen_claude_tool_uses: HashSet::new(),
            attribution_history_tokens: 0,
            attribution_compaction_tokens: 0,
            attribution_compaction_source: None,
            attribution_committed_turns: 0,
            attribution_compaction_pending: false,
            codex_tools: HashMap::new(),
            claude_tool_names: HashMap::new(),
            attribution_loaded: false,
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
        self.current_codex_task_first_turn_idx = None;
        self.current_codex_model = None;
        self.codex_total_usage = CodexUsageSnapshot::default();
        self.pending_codex_user_echo = None;
        self.last_codex_response = None;
        self.pending_codex_documents.clear();
        self.pending_codex_attribution.clear();
        self.pending_claude_diffs.clear();
        self.seen_claude_usage_messages.clear();
        self.seen_claude_complexity_messages.clear();
        self.seen_claude_tool_uses.clear();
        self.attribution_history_tokens = 0;
        self.attribution_compaction_tokens = 0;
        self.attribution_compaction_source = None;
        self.attribution_committed_turns = 0;
        self.attribution_compaction_pending = false;
        self.codex_tools.clear();
        self.claude_tool_names.clear();
        self.attribution_loaded = false;
        for subagent in &mut self.codex_subagent_files {
            let path = subagent.path.clone();
            let metadata = subagent.metadata.clone();
            let inherited_turn_ids = subagent.inherited_turn_ids.clone();
            *subagent = CodexSubagentFileState::new(path, metadata, inherited_turn_ids);
        }
    }

    fn apply_native_title(&mut self, title: &str) {
        let title = title.trim();
        if self.name_override.is_none() && !title.is_empty() {
            self.name = title.to_string();
            self.native_name_resolved = true;
        }
    }

    fn set_project_path(&mut self, cwd: &str) {
        let cwd = cwd.trim();
        if !cwd.is_empty() {
            self.project_path = Some(PathBuf::from(cwd));
        }
    }

    fn observe_activity_timestamp(&mut self, timestamp: Option<&str>) {
        let Some(timestamp) = timestamp else {
            return;
        };
        let Ok(timestamp) = chrono::DateTime::parse_from_rfc3339(timestamp) else {
            return;
        };
        if let Ok(seconds) = u64::try_from(timestamp.timestamp()) {
            self.last_activity = self.last_activity.max(seconds);
        }
    }

    fn observe_activity_unix(&mut self, timestamp: u64) {
        let seconds = if timestamp > 10_000_000_000 {
            timestamp / 1_000
        } else {
            timestamp
        };
        self.last_activity = self.last_activity.max(seconds);
    }

    fn finish_previous_turn_if_needed(&mut self) {
        if let Some(turn) = self.usage.turns.last_mut() {
            if turn.telemetry.outcome == TurnOutcome::InProgress
                && !turn.response_text.trim().is_empty()
            {
                turn.telemetry.outcome = TurnOutcome::Completed;
            }
        }
    }

    fn commit_finished_attribution_history(&mut self) {
        while self.attribution_committed_turns < self.usage.turns.len() {
            let turn = &mut self.usage.turns[self.attribution_committed_turns];
            turn.attribution.flush_deferred();
            self.attribution_history_tokens = self
                .attribution_history_tokens
                .saturating_add(turn.attribution.uncommitted_history_delta())
                .saturating_add(estimate_tokens(&turn.response_text));
            self.attribution_committed_turns += 1;
        }
    }

    fn new_turn_attribution(&self, prompt: &str) -> TurnAttribution {
        let mut attribution = TurnAttribution::new(prompt, self.attribution_history_tokens);
        if self.attribution_compaction_tokens > 0 {
            attribution.replace_active_history_estimate(
                self.attribution_history_tokens,
                self.attribution_compaction_tokens,
                self.attribution_compaction_source
                    .as_deref()
                    .unwrap_or("Compacted summary"),
            );
        }
        attribution
    }

    fn record_parent_request_attribution(
        &mut self,
        input_tokens: u64,
        cached_input_tokens: u64,
        exact: bool,
    ) {
        self.attribution_loaded = true;
        let Some(turn_index) = self.usage.turns.len().checked_sub(1) else {
            return;
        };
        if self.attribution_compaction_pending {
            let compacted_tokens = self.attribution_history_tokens.min(input_tokens);
            let source = self
                .attribution_compaction_source
                .as_deref()
                .unwrap_or("Compacted summary");
            self.usage.turns[turn_index]
                .attribution
                .replace_active_history_estimate(compacted_tokens, compacted_tokens, source);
        }
        let request_number = self.usage.turns[turn_index].attribution.request_count() + 1;
        let id = format!("parent-{}-{request_number}", turn_index + 1);
        let context_tokens = self.usage.turns[turn_index]
            .attribution
            .record_parent_request(id, input_tokens, cached_input_tokens, exact);
        if self.attribution_compaction_pending {
            self.attribution_history_tokens = context_tokens;
            self.attribution_compaction_tokens = context_tokens;
            self.attribution_compaction_pending = false;
        }
    }

    fn mark_attribution_compaction(&mut self, summary: Option<&str>, source: String) {
        let Some(turn) = self.usage.turns.last_mut() else {
            return;
        };
        turn.attribution.mark_compaction();
        self.attribution_compaction_source = Some(source.clone());
        if let Some(tokens) = summary.map(estimate_tokens).filter(|tokens| *tokens > 0) {
            self.attribution_history_tokens = tokens;
            self.attribution_compaction_tokens = tokens;
            turn.attribution
                .replace_active_history_estimate(tokens, tokens, &source);
            self.attribution_compaction_pending = false;
        } else {
            self.attribution_compaction_tokens = 0;
            self.attribution_compaction_pending = true;
        }
    }

    fn drop_attribution(&mut self) {
        for turn in &mut self.usage.turns {
            turn.attribution = TurnAttribution::default();
        }
        for subagent in &mut self.codex_subagent_files {
            subagent.attribution = TurnAttribution::default();
        }
        self.attribution_history_tokens = 0;
        self.attribution_compaction_tokens = 0;
        self.attribution_compaction_source = None;
        self.attribution_committed_turns = 0;
        self.attribution_compaction_pending = false;
        self.codex_tools.clear();
        self.claude_tool_names.clear();
        self.attribution_loaded = false;
    }

    fn rebuild_attribution(&mut self) {
        let mut replay = SessionState::new(self.file_path.clone(), self.provider);
        replay.session_id = self.session_id.clone();
        replay.project_path = self.project_path.clone();
        for subagent in &self.codex_subagent_files {
            replay
                .codex_subagent_files
                .push(CodexSubagentFileState::new(
                    subagent.path.clone(),
                    subagent.metadata.clone(),
                    subagent.inherited_turn_ids.clone(),
                ));
        }
        replay.poll_file();
        for (turn, replayed) in self.usage.turns.iter_mut().zip(&replay.usage.turns) {
            turn.attribution = replayed.attribution.clone();
        }
        for subagent in &mut self.codex_subagent_files {
            if let Some(replayed) = replay
                .codex_subagent_files
                .iter()
                .find(|candidate| candidate.path == subagent.path)
            {
                subagent.attribution = replayed.attribution.clone();
            }
        }
        self.attribution_history_tokens = replay.attribution_history_tokens;
        self.attribution_compaction_tokens = replay.attribution_compaction_tokens;
        self.attribution_compaction_source = replay.attribution_compaction_source;
        self.attribution_committed_turns = replay.attribution_committed_turns;
        self.attribution_compaction_pending = replay.attribution_compaction_pending;
        self.codex_tools = replay.codex_tools;
        self.claude_tool_names = replay.claude_tool_names;
        self.attribution_loaded = true;
    }

    fn finish_previous_codex_turn_at(&mut self, end_timestamp: &str) {
        self.finish_previous_turn_if_needed();
        if let Some(turn) = self.usage.turns.last_mut() {
            if turn.telemetry.duration_ms.is_none() {
                turn.telemetry.duration_ms = elapsed_ms(&turn.timestamp, Some(end_timestamp));
            }
            if turn.telemetry.outcome == TurnOutcome::InProgress {
                turn.telemetry.outcome = TurnOutcome::Aborted;
            }
        }
    }

    pub fn project_name(&self) -> String {
        self.project_path
            .as_ref()
            .and_then(|path| path.file_name())
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("Unknown project")
            .to_string()
    }

    pub fn project_display_path(&self) -> String {
        self.project_path
            .as_ref()
            .map(|path| path.display().to_string())
            .unwrap_or_default()
    }

    fn attach_codex_subagent(
        &mut self,
        path: PathBuf,
        metadata: CodexSubagentMetadata,
        inherited_turn_ids: HashSet<String>,
    ) {
        if self
            .codex_subagent_files
            .iter()
            .any(|subagent| subagent.path == path)
        {
            return;
        }

        self.ensure_named_agent(&metadata.session_id, &metadata.nickname, &metadata.role);
        if let Some(agent) = self
            .agents
            .iter_mut()
            .find(|agent| agent.id == metadata.session_id)
        {
            agent.status = AgentStatus::Thinking { dots: 0 };
        }
        if let Ok(modified) = fs::metadata(&path).and_then(|metadata| metadata.modified()) {
            let modified = modified
                .duration_since(SystemTime::UNIX_EPOCH)
                .map(|duration| duration.as_secs())
                .unwrap_or(0);
            self.last_modified = self.last_modified.max(modified);
        }
        self.codex_subagent_files.push(CodexSubagentFileState::new(
            path,
            metadata,
            inherited_turn_ids,
        ));
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

        self.poll_codex_subagents();

        // Re-scan sub-agents when new ones appear
        if !self.usage.turns.is_empty() {
            self.scan_subagents();
        }
    }

    fn poll_codex_subagents(&mut self) {
        if self.provider != ProviderKind::Codex {
            return;
        }

        let mut pending_lines = Vec::new();
        for (index, subagent) in self.codex_subagent_files.iter_mut().enumerate() {
            let file = match File::open(&subagent.path) {
                Ok(file) => file,
                Err(_) => continue,
            };

            if let Ok(metadata) = file.metadata() {
                if let Ok(modified) = metadata.modified() {
                    let modified = modified
                        .duration_since(SystemTime::UNIX_EPOCH)
                        .map(|duration| duration.as_secs())
                        .unwrap_or(0);
                    self.last_modified = self.last_modified.max(modified);
                }
                if metadata.len() < subagent.file_pos {
                    let path = subagent.path.clone();
                    let metadata = subagent.metadata.clone();
                    let inherited_turn_ids = subagent.inherited_turn_ids.clone();
                    *subagent = CodexSubagentFileState::new(path, metadata, inherited_turn_ids);
                }
            }

            let mut reader = BufReader::new(file);
            if reader.seek(SeekFrom::Start(subagent.file_pos)).is_err() {
                continue;
            }

            let mut buf = String::new();
            while reader.read_line(&mut buf).unwrap_or(0) > 0 {
                if buf.ends_with('\n') {
                    let line = if subagent.partial_line.is_empty() {
                        buf.trim().to_string()
                    } else {
                        let full = format!("{}{}", subagent.partial_line, buf.trim());
                        subagent.partial_line.clear();
                        full
                    };
                    if !line.is_empty() {
                        pending_lines.push((index, line));
                    }
                } else {
                    subagent.partial_line.push_str(&buf);
                }
                buf.clear();
            }
            subagent.file_pos = reader.stream_position().unwrap_or(subagent.file_pos);
        }

        for (index, line) in pending_lines {
            self.process_codex_subagent_line(index, &line);
        }
        for index in 0..self.codex_subagent_files.len() {
            self.sync_codex_subagent(index);
        }
    }

    fn process_codex_subagent_line(&mut self, index: usize, line: &str) {
        let value: serde_json::Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => return,
        };
        let line_type = value
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("");
        let timestamp = value
            .get("timestamp")
            .and_then(|timestamp| timestamp.as_str())
            .unwrap_or("");

        let mut delegation = None;
        let mut response = None;
        let Some(subagent) = self.codex_subagent_files.get_mut(index) else {
            return;
        };
        if !subagent.observing_child_turns {
            let starts_child_turn = codex_record_turn_id(&value)
                .is_some_and(|turn_id| !subagent.inherited_turn_ids.contains(turn_id));
            if !starts_child_turn {
                return;
            }
            subagent.observing_child_turns = true;
        }
        if subagent.first_timestamp.is_empty() && !timestamp.is_empty() {
            subagent.first_timestamp = timestamp.to_string();
        }

        match line_type {
            "turn_context" => {
                let payload = value.get("payload").unwrap_or(&value);
                subagent.model = payload
                    .get("model")
                    .or_else(|| value.get("model"))
                    .and_then(|model| model.as_str())
                    .map(str::to_string);
            }
            "response_item" => {
                let Some(payload) = value.get("payload") else {
                    return;
                };
                match payload
                    .get("type")
                    .and_then(|kind| kind.as_str())
                    .unwrap_or("")
                {
                    "message" => {
                        let role = payload
                            .get("role")
                            .and_then(|role| role.as_str())
                            .unwrap_or("");
                        let text = content_text(payload.get("content"));
                        if role == "user"
                            && subagent.prompt.is_empty()
                            && !text.trim().is_empty()
                            && !is_synthetic_codex_user_message(&text)
                        {
                            subagent.prompt = text.clone();
                            subagent.attribution.set_prompt(&text);
                            delegation = Some(text);
                        } else if role == "assistant"
                            && record_codex_subagent_response(subagent, &text)
                        {
                            subagent.attribution.defer_after_request(
                                AttributionCategory::ProviderRuntime,
                                "Assistant state",
                                "Model response",
                                estimate_tokens(&text),
                                None,
                            );
                            response = Some(text);
                        }
                    }
                    "function_call" | "custom_tool_call" | "web_search_call" => {
                        subagent.tool_calls += 1;
                        let name = payload
                            .get("name")
                            .and_then(|name| name.as_str())
                            .unwrap_or("tool");
                        let (source, purpose) = tool_source_and_purpose(
                            name,
                            payload.get("arguments").or_else(|| payload.get("input")),
                        );
                        subagent.attribution.defer_after_request(
                            tool_attribution_category(name),
                            source,
                            format!("{} #{}", purpose, subagent.tool_calls),
                            estimate_tokens(&payload.to_string()),
                            Some(format!("After {purpose}")),
                        );
                    }
                    _ => {}
                }
            }
            "event_msg" => {
                let Some(payload) = value.get("payload") else {
                    return;
                };
                match payload
                    .get("type")
                    .and_then(|kind| kind.as_str())
                    .unwrap_or("")
                {
                    "task_started" => subagent.outcome = TurnOutcome::InProgress,
                    "user_message" => {
                        if let Some(message) =
                            payload.get("message").and_then(|message| message.as_str())
                        {
                            if subagent.prompt.is_empty()
                                && !is_synthetic_codex_user_message(message)
                            {
                                subagent.prompt = message.to_string();
                                subagent.attribution.set_prompt(message);
                                delegation = Some(message.to_string());
                            }
                        }
                    }
                    "agent_message" => {
                        if let Some(message) =
                            payload.get("message").and_then(|message| message.as_str())
                        {
                            if record_codex_subagent_response(subagent, message) {
                                subagent.attribution.defer_after_request(
                                    AttributionCategory::ProviderRuntime,
                                    "Assistant state",
                                    "Model response",
                                    estimate_tokens(message),
                                    None,
                                );
                                response = Some(message.to_string());
                            }
                        }
                    }
                    "token_count" => apply_codex_subagent_usage(subagent, payload, timestamp),
                    "patch_apply_end"
                        if payload.get("status").and_then(|status| status.as_str())
                            == Some("completed") =>
                    {
                        let change = codex_applied_file_change(payload);
                        subagent.lines_added =
                            subagent.lines_added.saturating_add(change.lines_added);
                        subagent.lines_removed =
                            subagent.lines_removed.saturating_add(change.lines_removed);
                        subagent.files_created =
                            subagent.files_created.saturating_add(change.files_created);
                        subagent.files_deleted =
                            subagent.files_deleted.saturating_add(change.files_deleted);
                    }
                    "task_complete" => {
                        subagent.outcome = TurnOutcome::Completed;
                        subagent.duration_ms = payload
                            .get("duration_ms")
                            .and_then(|duration| duration.as_u64())
                            .or_else(|| elapsed_ms(&subagent.first_timestamp, Some(timestamp)));
                        if let Some(message) = payload
                            .get("last_agent_message")
                            .or_else(|| payload.get("message"))
                            .and_then(|message| message.as_str())
                        {
                            if record_codex_subagent_response(subagent, message) {
                                response = Some(message.to_string());
                            }
                        }
                    }
                    "turn_aborted" => {
                        subagent.outcome = TurnOutcome::Aborted;
                        subagent.duration_ms = payload
                            .get("duration_ms")
                            .and_then(|duration| duration.as_u64())
                            .or_else(|| elapsed_ms(&subagent.first_timestamp, Some(timestamp)));
                    }
                    "task_failed" | "turn_failed" => {
                        subagent.outcome = TurnOutcome::Failed;
                        subagent.duration_ms = payload
                            .get("duration_ms")
                            .and_then(|duration| duration.as_u64())
                            .or_else(|| elapsed_ms(&subagent.first_timestamp, Some(timestamp)));
                    }
                    _ => {}
                }
            }
            _ => {}
        }

        let metadata = subagent.metadata.clone();
        let parent_endpoint = if metadata.parent_session_id == self.session_id {
            "codex".to_string()
        } else {
            metadata.parent_session_id.clone()
        };
        if let Some(prompt) = delegation {
            self.push_observed_message_typed(
                parent_endpoint.clone(),
                metadata.session_id.clone(),
                prompt,
                MessageType::Delegation,
            );
        }
        if let Some(response) = response {
            self.push_observed_message_typed(
                metadata.session_id,
                parent_endpoint,
                response,
                MessageType::Response,
            );
        }
    }

    fn sync_codex_subagent(&mut self, index: usize) {
        let Some(subagent) = self.codex_subagent_files.get(index) else {
            return;
        };
        let metadata = subagent.metadata.clone();
        let outcome = subagent.outcome.clone();
        let timestamp = if subagent.first_timestamp.is_empty() {
            metadata.started_at.clone()
        } else {
            subagent.first_timestamp.clone()
        };
        let agent_cost = AgentCost {
            id: metadata.session_id.clone(),
            name: metadata.nickname.clone(),
            role: metadata.role.clone(),
            model: subagent.model.clone(),
            cost: subagent.cost,
            cost_known: subagent.cost_known,
            input_tokens: subagent.input_tokens,
            output_tokens: subagent.output_tokens,
            cache_read_tokens: subagent.cache_read_tokens,
            outcome: outcome.clone(),
            duration_ms: subagent.duration_ms,
            tool_calls: subagent.tool_calls,
            lines_added: subagent.lines_added,
            lines_removed: subagent.lines_removed,
            files_created: subagent.files_created,
            files_deleted: subagent.files_deleted,
            prompt: subagent.prompt.clone(),
            response_preview: subagent.response_preview.clone(),
        };
        let agent_requests: Vec<RequestAttribution> = (0..subagent.attribution.request_count())
            .filter_map(|request| subagent.attribution.request(request).cloned())
            .collect();

        self.ensure_named_agent(&metadata.session_id, &metadata.nickname, &metadata.role);
        if let Some(agent) = self
            .agents
            .iter_mut()
            .find(|agent| agent.id == metadata.session_id)
        {
            agent.status = if outcome == TurnOutcome::InProgress {
                AgentStatus::Thinking { dots: 0 }
            } else {
                AgentStatus::Idle
            };
        }

        if self.usage.turns.is_empty() {
            return;
        }
        let mut target_turn = 0;
        for (turn_index, turn) in self.usage.turns.iter().enumerate() {
            if !turn.timestamp.is_empty() && turn.timestamp.as_str() <= timestamp.as_str() {
                target_turn = turn_index;
            }
        }

        if let Some(existing) = self
            .usage
            .turns
            .iter_mut()
            .flat_map(|turn| turn.agents.iter_mut())
            .find(|agent| agent.id == metadata.session_id)
        {
            *existing = agent_cost;
        } else if let Some(turn) = self.usage.turns.get_mut(target_turn) {
            turn.agents.push(agent_cost);
        }
        if let Some(turn) = self.usage.turns.get_mut(target_turn) {
            turn.attribution
                .set_agent_requests(metadata.session_id.clone(), agent_requests);
            self.attribution_loaded = true;
        }
        for turn in &mut self.usage.turns {
            turn.context_saved = turn
                .agents
                .iter()
                .map(|agent| agent.input_tokens + agent.cache_read_tokens)
                .sum();
        }
    }

    fn apply_claude_tool_results(&mut self, value: &serde_json::Value) {
        let Some(content) = value
            .get("message")
            .and_then(|message| message.get("content"))
            .and_then(|content| content.as_array())
        else {
            return;
        };

        for block in content {
            if block.get("type").and_then(|kind| kind.as_str()) != Some("tool_result") {
                continue;
            }
            let Some(tool_use_id) = block.get("tool_use_id").and_then(|id| id.as_str()) else {
                continue;
            };
            let tool_name = self
                .claude_tool_names
                .get(tool_use_id)
                .cloned()
                .unwrap_or_else(|| "tool".to_string());
            let (source, purpose) = tool_source_and_purpose(&tool_name, None);
            if let Some(turn) = self.usage.turns.last_mut() {
                turn.attribution.flush_deferred();
                let invocation = format!("{} result", purpose);
                turn.attribution.observe(
                    tool_attribution_category(&tool_name),
                    source,
                    invocation,
                    estimate_tool_result_tokens(block),
                );
            }
            let Some((turn_index, mut change)) = self.pending_claude_diffs.remove(tool_use_id)
            else {
                continue;
            };
            if block
                .get("is_error")
                .and_then(|is_error| is_error.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            change.observe_claude_result(block, value);
            if let Some(turn) = self.usage.turns.get_mut(turn_index) {
                turn.telemetry.lines_added = turn
                    .telemetry
                    .lines_added
                    .saturating_add(change.lines_added);
                turn.telemetry.lines_removed = turn
                    .telemetry
                    .lines_removed
                    .saturating_add(change.lines_removed);
                turn.telemetry.files_created = turn
                    .telemetry
                    .files_created
                    .saturating_add(change.files_created);
                turn.telemetry.files_deleted = turn
                    .telemetry
                    .files_deleted
                    .saturating_add(change.files_deleted);
            }
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
                session_id,
                name,
                cwd,
                ts,
            } => {
                self.observe_activity_unix(ts);
                // A new session_start in the same file means the old session was cleared
                if self.session_id != session_id && !self.messages.is_empty() {
                    self.clear_display();
                }
                self.session_id = session_id;
                if let Some(cwd) = cwd {
                    self.set_project_path(&cwd);
                }
                // Hook names are a fallback; native generated titles still win.
                if self.name_override.is_none() && !self.native_name_resolved {
                    if let Some(n) = name {
                        self.name = n;
                    }
                }
            }
            LiveEvent::TurnStart {
                turn_index,
                prompt,
                ts,
            } => {
                self.observe_activity_unix(ts);
                let marker = TurnMarker {
                    turn_index,
                    prompt: prompt.unwrap_or_default(),
                    message_start_idx: self.messages.len(),
                };
                self.turns.push(marker);
            }
            LiveEvent::SessionClear { ts } => {
                self.observe_activity_unix(ts);
                self.clear_display();
            }
            LiveEvent::AgentSpawn { id, name, role, ts } => {
                self.observe_activity_unix(ts);
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
                from,
                to,
                content,
                ts,
            } => {
                self.observe_activity_unix(ts);
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
            LiveEvent::AgentDone { id, ts } => {
                self.observe_activity_unix(ts);
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
            && !line.contains("\"turn_duration\"")
            && !line.contains("compact")
            && !line.contains("hookAdditionalContext")
            && !line.contains("additional_context")
            && !line.contains("\"memory\"")
            && !line.contains("custom-title")
            && !line.contains("ai-title")
        {
            return;
        }

        let v: serde_json::Value = match serde_json::from_str(line) {
            Ok(v) => v,
            Err(_) => return,
        };

        if let Some(cwd) = v.get("cwd").and_then(|cwd| cwd.as_str()) {
            self.set_project_path(cwd);
        }

        let line_type = v.get("type").and_then(|t| t.as_str()).unwrap_or("");

        if let Some((source, context)) = claude_hook_context(&v) {
            if let Some(turn) = self.usage.turns.last_mut() {
                turn.attribution.observe(
                    AttributionCategory::Hooks,
                    source.clone(),
                    "Injected context",
                    estimate_tokens(&context),
                );
                turn.attribution
                    .set_next_request_label(format!("After {source}"));
            }
        }
        if let Some((source, context)) = claude_memory_context(&v) {
            if let Some(turn) = self.usage.turns.last_mut() {
                turn.attribution.observe(
                    AttributionCategory::Memory,
                    source,
                    "Loaded memory",
                    estimate_tokens(&context),
                );
            }
        }
        if is_claude_compaction_record(&v) {
            let is_new = self
                .usage
                .turns
                .last_mut()
                .map(|turn| turn.telemetry.mark_context_compaction())
                .unwrap_or(false);
            if is_new {
                let summary = compact_summary(&v);
                self.mark_attribution_compaction(summary.as_deref(), compaction_source(&v));
            }
        }

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
                self.apply_claude_tool_results(&v);
                // Only process real user prompts, not tool results or system messages.
                // Real prompts have "userType": "external" and string content.
                let user_type = v.get("userType").and_then(|u| u.as_str()).unwrap_or("");
                if user_type != "external" {
                    return;
                }

                if let Some(content) = claude_external_prompt(&v) {
                    // Skip system/meta messages and tool call descriptions
                    let trimmed = content.trim_start();
                    if trimmed.starts_with('<')
                        || trimmed.starts_with("Tool: ")
                        || trimmed.starts_with("Working Directory:")
                    {
                        return;
                    }

                    self.observe_activity_timestamp(
                        v.get("timestamp").and_then(|timestamp| timestamp.as_str()),
                    );

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
                    self.finish_previous_turn_if_needed();
                    self.commit_finished_attribution_history();
                    self.current_turn_prompt = prompt;
                    // Create a new turn entry
                    let attribution = self.new_turn_attribution(&self.current_turn_prompt);
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
                        response_text: String::new(),
                        cost_known: false,
                        telemetry: TurnTelemetry::default(),
                        attribution,
                    });
                    if let Some(turn) = self.usage.turns.last_mut() {
                        for (name, tokens) in claude_direct_documents(&v) {
                            turn.attribution.observe(
                                AttributionCategory::DocumentsAndKbs,
                                name,
                                "Attached to prompt",
                                tokens,
                            );
                        }
                    }
                }
            }
            "assistant" => {
                self.observe_activity_timestamp(
                    v.get("timestamp").and_then(|timestamp| timestamp.as_str()),
                );
                let message = v.get("message");
                let model = v
                    .get("message")
                    .and_then(|message| message.get("model"))
                    .or_else(|| v.get("model"))
                    .and_then(|model| model.as_str());
                let message_id = message
                    .and_then(|message| message.get("id"))
                    .and_then(|id| id.as_str());
                let content_has_thinking = message
                    .and_then(|message| message.get("content"))
                    .and_then(|content| content.as_array())
                    .is_some_and(|blocks| {
                        blocks.iter().any(|block| {
                            block.get("type").and_then(|kind| kind.as_str()) == Some("thinking")
                        })
                    });
                // Accumulate usage from assistant messages
                if let Some(usage) = message.and_then(|message| message.get("usage")) {
                    let input = usage
                        .get("input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let output = usage
                        .get("output_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);
                    let (cache_write, cache_write_5m, cache_write_1h) =
                        claude_cache_write_tokens(usage);
                    let cache_read = usage
                        .get("cache_read_input_tokens")
                        .and_then(|v| v.as_u64())
                        .unwrap_or(0);

                    let pricing_date = event_pricing_date(
                        v.get("timestamp").and_then(|timestamp| timestamp.as_str()),
                    );
                    let usage_is_new = message_id
                        .map(|id| self.seen_claude_usage_messages.insert(id.to_string()))
                        .unwrap_or(true);
                    let exact_thinking = usage
                        .get("output_tokens_details")
                        .and_then(|details| details.get("thinking_tokens"))
                        .and_then(|tokens| tokens.as_u64());
                    if exact_thinking.is_some() || content_has_thinking {
                        let complexity_is_new = message_id
                            .map(|id| self.seen_claude_complexity_messages.insert(id.to_string()))
                            .unwrap_or(true);
                        if complexity_is_new {
                            if let Some(turn) = self.usage.turns.last_mut() {
                                if let Some(thinking_tokens) = exact_thinking {
                                    turn.telemetry.reasoning_tokens = turn
                                        .telemetry
                                        .reasoning_tokens
                                        .saturating_add(thinking_tokens);
                                    turn.telemetry.reasoning_tokens_emitted = true;
                                } else {
                                    turn.telemetry.complexity_proxy_tokens = turn
                                        .telemetry
                                        .complexity_proxy_tokens
                                        .saturating_add(output);
                                    turn.telemetry.complexity_proxy_emitted = true;
                                }
                            }
                        }
                    }
                    if usage_is_new {
                        let cost = model.and_then(|model| {
                            compute_provider_cost_with_cache_ttl_at(
                                self.provider,
                                model,
                                input,
                                output,
                                cache_write_5m,
                                cache_write_1h,
                                cache_read,
                                pricing_date,
                            )
                        });
                        self.cumulative_context += input + cache_read + cache_write;
                        let context_window = model.and_then(|model| {
                            model_context_window_at(self.provider, model, pricing_date)
                        });

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
                            turn.telemetry.model = model.map(str::to_string);
                            turn.telemetry
                                .observe_context(input + cache_read + cache_write, context_window);
                        }
                        self.record_parent_request_attribution(
                            input + cache_read + cache_write,
                            cache_read + cache_write,
                            true,
                        );
                    }
                }
                // Capture assistant response text for metrics analysis
                if let Some(content) = v.get("message").and_then(|m| m.get("content")) {
                    if let Some(arr) = content.as_array() {
                        for block in arr {
                            match block.get("type").and_then(|t| t.as_str()).unwrap_or("") {
                                "text" => {
                                    if let Some(text) = block.get("text").and_then(|t| t.as_str()) {
                                        if let Some(turn) = self.usage.turns.last_mut() {
                                            if !turn.response_text.is_empty() {
                                                turn.response_text.push('\n');
                                            }
                                            let remaining =
                                                2000usize.saturating_sub(turn.response_text.len());
                                            if remaining > 0 {
                                                turn.response_text
                                                    .extend(text.chars().take(remaining));
                                            }
                                        }
                                    }
                                }
                                "tool_use" => {
                                    let name = block
                                        .get("name")
                                        .and_then(|name| name.as_str())
                                        .unwrap_or("");
                                    let tool_use_is_new = block
                                        .get("id")
                                        .and_then(|id| id.as_str())
                                        .map(|id| self.seen_claude_tool_uses.insert(id.to_string()))
                                        .unwrap_or(true);
                                    if !tool_use_is_new {
                                        continue;
                                    }
                                    if let Some(id) = block.get("id").and_then(|id| id.as_str()) {
                                        self.claude_tool_names
                                            .insert(id.to_string(), name.to_string());
                                    }
                                    let (source, purpose) =
                                        tool_source_and_purpose(name, block.get("input"));
                                    if let Some(turn) = self.usage.turns.last_mut() {
                                        turn.telemetry.tool_calls += 1;
                                        let invocation =
                                            format!("{} #{}", purpose, turn.telemetry.tool_calls);
                                        turn.attribution.observe(
                                            tool_attribution_category(name),
                                            source,
                                            invocation,
                                            estimate_tokens(&block.to_string()),
                                        );
                                        turn.attribution
                                            .set_next_request_label(format!("After {purpose}"));
                                        if matches!(
                                            name,
                                            "Edit"
                                                | "Write"
                                                | "NotebookEdit"
                                                | "MultiEdit"
                                                | "Delete"
                                                | "apply_patch"
                                        ) {
                                            turn.telemetry.patches += 1;
                                        } else if name.to_ascii_lowercase().contains("search") {
                                            turn.telemetry.web_searches += 1;
                                        }
                                    }
                                    if let (Some(id), Some(input), Some(turn_index)) = (
                                        block.get("id").and_then(|id| id.as_str()),
                                        block.get("input"),
                                        self.usage.turns.len().checked_sub(1),
                                    ) {
                                        if let Some(change) = claude_tool_file_change(name, input) {
                                            self.pending_claude_diffs
                                                .insert(id.to_string(), (turn_index, change));
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    }
                }
                if let Some(turn) = self.usage.turns.last_mut() {
                    turn.telemetry.model = model.map(str::to_string);
                    let stop_reason = v
                        .get("message")
                        .and_then(|message| message.get("stop_reason"))
                        .and_then(|reason| reason.as_str())
                        .unwrap_or("");
                    if let Some(outcome) = claude_terminal_outcome(model, stop_reason) {
                        turn.telemetry.outcome = outcome;
                        turn.telemetry.duration_ms = elapsed_ms(
                            &turn.timestamp,
                            v.get("timestamp").and_then(|timestamp| timestamp.as_str()),
                        );
                    }
                }
            }
            "system"
                if v.get("subtype").and_then(|subtype| subtype.as_str())
                    == Some("turn_duration") =>
            {
                self.observe_activity_timestamp(
                    v.get("timestamp").and_then(|timestamp| timestamp.as_str()),
                );
                if let (Some(turn), Some(duration_ms)) = (
                    self.usage.turns.last_mut(),
                    v.get("durationMs").and_then(|duration| duration.as_u64()),
                ) {
                    turn.telemetry.duration_ms = Some(duration_ms);
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
                    if let Some(cwd) = payload.get("cwd").and_then(|cwd| cwd.as_str()) {
                        self.set_project_path(cwd);
                        if self.name_override.is_none() && !self.native_name_resolved {
                            self.name = PathBuf::from(cwd)
                                .file_name()
                                .and_then(|name| name.to_str())
                                .unwrap_or("Codex session")
                                .to_string();
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
                if let Some(cwd) = payload
                    .get("cwd")
                    .or_else(|| v.get("cwd"))
                    .and_then(|cwd| cwd.as_str())
                {
                    self.set_project_path(cwd);
                    if self.name_override.is_none() && !self.native_name_resolved {
                        self.name = PathBuf::from(cwd)
                            .file_name()
                            .and_then(|name| name.to_str())
                            .unwrap_or("Codex session")
                            .to_string();
                    }
                }
            }
            "response_item" => {
                self.observe_activity_timestamp(Some(&timestamp));
                self.process_codex_response_item(v.get("payload"), &timestamp);
            }
            "event_msg" => {
                self.observe_activity_timestamp(Some(&timestamp));
                self.process_codex_event_msg(v.get("payload"), &timestamp);
            }
            "compacted" => {
                self.observe_activity_timestamp(Some(&timestamp));
                let is_new = self
                    .usage
                    .turns
                    .last_mut()
                    .map(|turn| turn.telemetry.mark_context_compaction())
                    .unwrap_or(false);
                if is_new {
                    let summary = compact_summary(&v);
                    self.mark_attribution_compaction(summary.as_deref(), compaction_source(&v));
                }
            }
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
        let event_type = payload.get("type").and_then(|t| t.as_str()).unwrap_or("");
        if event_type.contains("hook") {
            if let Some(context) = nested_text(
                payload,
                &[
                    "additional_context",
                    "additionalContext",
                    "hookAdditionalContext",
                ],
            ) {
                let source = readable_event_name(event_type);
                self.observe_or_queue_codex_attribution(
                    AttributionCategory::Hooks,
                    source.clone(),
                    "Injected context",
                    estimate_tokens(&context),
                );
                if let Some(turn) = self.usage.turns.last_mut() {
                    turn.attribution
                        .set_next_request_label(format!("After {source}"));
                }
            }
        }
        match event_type {
            "message" => {
                let role = payload.get("role").and_then(|r| r.as_str()).unwrap_or("");
                let text = if role == "user" {
                    codex_user_message_text(payload)
                } else {
                    content_text(payload.get("content"))
                };
                if text.trim().is_empty() {
                    return;
                }
                match role {
                    "user" => {
                        if let Some(context) = codex_agents_md_context(payload) {
                            self.observe_or_queue_codex_attribution(
                                AttributionCategory::Memory,
                                "AGENTS.md",
                                "Loaded project guidance",
                                estimate_tokens(&context),
                            );
                        }
                        self.pending_codex_documents = codex_direct_documents(payload);
                        if self.usage.turns.is_empty() && !is_synthetic_codex_user_message(&text) {
                            self.start_codex_turn(text.clone(), timestamp.to_string(), None);
                            self.pending_codex_user_echo = Some(text);
                        }
                    }
                    "assistant" => {
                        if let Some(turn) = self.usage.turns.last_mut() {
                            turn.attribution.defer_after_request(
                                AttributionCategory::ProviderRuntime,
                                "Assistant state",
                                "Model response",
                                estimate_tokens(&text),
                                None,
                            );
                        }
                        self.push_codex_response(text)
                    }
                    "developer" => {
                        let lower = text.to_ascii_lowercase();
                        let (category, source, invocation) = if lower.contains("hook") {
                            (AttributionCategory::Hooks, "Codex hook", "Injected context")
                        } else if lower.contains("memory") || lower.contains("agents.md") {
                            (
                                AttributionCategory::Memory,
                                "Project memory",
                                "Loaded memory",
                            )
                        } else {
                            (
                                AttributionCategory::ProviderRuntime,
                                "Developer instructions",
                                "Runtime instruction",
                            )
                        };
                        self.observe_or_queue_codex_attribution(
                            category,
                            source,
                            invocation,
                            estimate_tokens(&text),
                        );
                    }
                    _ => {}
                }
            }
            "function_call"
                if payload.get("name").and_then(|name| name.as_str()) == Some("spawn_agent") =>
            {
                self.process_codex_spawn(payload);
            }
            "agent_message" => self.process_codex_agent_message(payload),
            "function_call" | "custom_tool_call" | "web_search_call" => {
                let name = payload
                    .get("name")
                    .or_else(|| payload.get("status"))
                    .and_then(|n| n.as_str())
                    .unwrap_or("tool call");
                let call_id = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(|id| id.as_str());
                let arguments = payload.get("arguments").or_else(|| payload.get("input"));
                let (source, purpose) = tool_source_and_purpose(name, arguments);
                let category = tool_attribution_category_for_call(name, arguments);
                if let Some(call_id) = call_id {
                    self.codex_tools.insert(
                        call_id.to_string(),
                        ToolAttributionDescriptor {
                            category,
                            source: source.clone(),
                            purpose: purpose.clone(),
                        },
                    );
                }
                if let Some(turn) = self.usage.turns.last_mut() {
                    turn.telemetry.tool_calls += 1;
                    turn.attribution.defer_after_request(
                        category,
                        source,
                        format!("{} #{}", purpose, turn.telemetry.tool_calls),
                        estimate_tokens(&payload.to_string()),
                        Some(format!("After {purpose}")),
                    );
                    if payload.get("type").and_then(|kind| kind.as_str()) == Some("web_search_call")
                    {
                        turn.telemetry.web_searches += 1;
                    }
                }
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
                let call_id = payload
                    .get("call_id")
                    .or_else(|| payload.get("id"))
                    .and_then(|id| id.as_str());
                let descriptor = call_id
                    .and_then(|id| self.codex_tools.get(id))
                    .cloned()
                    .unwrap_or_else(|| ToolAttributionDescriptor {
                        category: AttributionCategory::ToolsAndMcps,
                        source: "Provider tool".to_string(),
                        purpose: "Tool call".to_string(),
                    });
                if let Some(turn) = self.usage.turns.last_mut() {
                    turn.attribution.flush_deferred();
                    turn.attribution.observe(
                        descriptor.category,
                        descriptor.source,
                        format!("{} result", descriptor.purpose),
                        estimate_tool_result_tokens(payload),
                    );
                }
                if let Some(output) = payload.get("output").and_then(value_preview) {
                    self.push_observed_message("tool".to_string(), "codex".to_string(), output);
                }
            }
            "reasoning" => {
                if let Some(text) = content_text_opt(payload.get("summary"))
                    .or_else(|| content_text_opt(payload.get("content")))
                {
                    if !text.trim().is_empty() {
                        if let Some(turn) = self.usage.turns.last_mut() {
                            turn.attribution.defer_after_request(
                                AttributionCategory::ProviderRuntime,
                                "Reasoning state",
                                "Carried reasoning",
                                estimate_tokens(&text),
                                None,
                            );
                        }
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

    fn process_codex_spawn(&mut self, payload: &serde_json::Value) {
        let call_id = payload
            .get("call_id")
            .or_else(|| payload.get("id"))
            .and_then(|id| id.as_str())
            .unwrap_or("subagent");
        let task_name = payload
            .get("arguments")
            .and_then(|arguments| arguments.as_str())
            .and_then(|arguments| serde_json::from_str::<serde_json::Value>(arguments).ok())
            .and_then(|arguments| {
                arguments
                    .get("task_name")
                    .and_then(|name| name.as_str())
                    .map(str::to_string)
            })
            .unwrap_or_else(|| "subagent".to_string());

        self.ensure_named_agent(call_id, &task_name, "Subagent");
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == call_id) {
            agent.status = AgentStatus::Thinking { dots: 0 };
        }
        if let Some(turn) = self.usage.turns.last_mut() {
            turn.telemetry.tool_calls += 1;
            turn.attribution.defer_after_request(
                AttributionCategory::Agents,
                task_name.clone(),
                "Delegated task",
                estimate_tokens(&payload.to_string()),
                Some(format!("After agent {task_name}")),
            );
            turn.agents.push(AgentCost {
                id: call_id.to_string(),
                name: task_name.clone(),
                role: "Subagent".to_string(),
                model: None,
                cost: 0.0,
                cost_known: false,
                input_tokens: 0,
                output_tokens: 0,
                cache_read_tokens: 0,
                outcome: TurnOutcome::InProgress,
                duration_ms: None,
                tool_calls: 0,
                lines_added: 0,
                lines_removed: 0,
                files_created: 0,
                files_deleted: 0,
                prompt: String::new(),
                response_preview: String::new(),
            });
        }
        self.push_observed_message(
            "codex".to_string(),
            call_id.to_string(),
            format!("started {task_name}"),
        );
    }

    fn process_codex_agent_message(&mut self, payload: &serde_json::Value) {
        let content = content_text(payload.get("content"));
        if content.trim().is_empty() {
            return;
        }

        let author = payload
            .get("author")
            .and_then(|author| author.as_str())
            .unwrap_or("subagent");
        let recipient = payload
            .get("recipient")
            .and_then(|recipient| recipient.as_str())
            .unwrap_or("/root");
        let from = self.codex_agent_endpoint(author);
        let to = self.codex_agent_endpoint(recipient);
        let agent_name = self
            .agents
            .iter()
            .find(|agent| agent.id == from)
            .map(|agent| agent.name.clone());
        if let Some(agent_name) = agent_name {
            if let Some(turn) = self.usage.turns.last_mut() {
                turn.attribution.flush_deferred();
                turn.attribution.observe(
                    AttributionCategory::Agents,
                    agent_name.clone(),
                    "Returned summary",
                    estimate_tokens(&content),
                );
                turn.attribution
                    .set_next_request_label(format!("After agent {agent_name}"));
            }
            if let Some(agent_cost) = self
                .usage
                .turns
                .iter_mut()
                .rev()
                .flat_map(|turn| turn.agents.iter_mut().rev())
                .find(|agent| agent.name == agent_name && agent.response_preview.is_empty())
            {
                agent_cost.response_preview = content.clone();
            }
        }
        self.push_observed_message(from.clone(), to, content);
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == from) {
            agent.status = AgentStatus::Idle;
        }
    }

    fn codex_agent_endpoint(&mut self, endpoint: &str) -> String {
        if endpoint == "/root" || endpoint == "root" {
            return "codex".to_string();
        }
        let resolved = self.resolve_id(endpoint);
        if !self.agents.iter().any(|agent| agent.id == resolved) {
            let display_name = endpoint
                .rsplit('/')
                .find(|part| !part.is_empty())
                .unwrap_or("subagent");
            self.ensure_named_agent(&resolved, display_name, "Subagent");
        }
        resolved
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
                self.current_codex_task_first_turn_idx = Some(self.usage.turns.len());
            }
            "user_message" => {
                let emitted_documents = codex_event_documents(payload);
                let documents = if emitted_documents.is_empty() {
                    std::mem::take(&mut self.pending_codex_documents)
                } else {
                    self.pending_codex_documents.clear();
                    emitted_documents
                };
                if let Some(message) = payload.get("message").and_then(|m| m.as_str()) {
                    let echoes_response_item =
                        self.pending_codex_user_echo.as_deref() == Some(message);
                    self.pending_codex_user_echo = None;
                    if !is_synthetic_codex_user_message(message) && !echoes_response_item {
                        self.start_codex_turn(
                            message.to_string(),
                            timestamp.to_string(),
                            self.current_codex_turn_id.clone(),
                        );
                    }
                    if self
                        .usage
                        .turns
                        .last()
                        .is_some_and(|turn| turn.prompt == message)
                    {
                        if let Some(turn) = self.usage.turns.last_mut() {
                            for (name, tokens) in documents {
                                turn.attribution.observe(
                                    AttributionCategory::DocumentsAndKbs,
                                    name,
                                    "Attached to prompt",
                                    tokens,
                                );
                            }
                        }
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
            "sub_agent_activity" => self.process_codex_subagent_activity(payload),
            "token_count" => self.apply_codex_usage(payload, timestamp),
            "task_complete" => {
                self.finish_current_codex_task(payload, timestamp, TurnOutcome::Completed);
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
            "turn_aborted" => {
                self.finish_current_codex_task(payload, timestamp, TurnOutcome::Aborted);
            }
            "task_failed" | "turn_failed" => {
                self.finish_current_codex_task(payload, timestamp, TurnOutcome::Failed);
            }
            "context_compacted" => {
                let is_new = self
                    .usage
                    .turns
                    .last_mut()
                    .map(|turn| turn.telemetry.mark_context_compaction())
                    .unwrap_or(false);
                if is_new {
                    let summary = compact_summary(payload);
                    self.mark_attribution_compaction(
                        summary.as_deref(),
                        compaction_source(payload),
                    );
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
                if label == "patch_apply_end" && status == "completed" {
                    if let Some(turn) = self.usage.turns.last_mut() {
                        turn.telemetry.patches += 1;
                        let change = codex_applied_file_change(payload);
                        turn.telemetry.lines_added = turn
                            .telemetry
                            .lines_added
                            .saturating_add(change.lines_added);
                        turn.telemetry.lines_removed = turn
                            .telemetry
                            .lines_removed
                            .saturating_add(change.lines_removed);
                        turn.telemetry.files_created = turn
                            .telemetry
                            .files_created
                            .saturating_add(change.files_created);
                        turn.telemetry.files_deleted = turn
                            .telemetry
                            .files_deleted
                            .saturating_add(change.files_deleted);
                    }
                }
                self.push_observed_message(
                    "tool".to_string(),
                    "codex".to_string(),
                    format!("{label} ({status})"),
                );
            }
            _ => {}
        }
    }

    fn finish_current_codex_task(
        &mut self,
        payload: &serde_json::Value,
        timestamp: &str,
        outcome: TurnOutcome,
    ) {
        let current_turn_idx = self.usage.turns.len().saturating_sub(1);
        let task_was_steered = self
            .current_codex_task_first_turn_idx
            .map(|first_turn_idx| first_turn_idx < current_turn_idx)
            .unwrap_or(false);
        let native_duration = payload.get("duration_ms").and_then(|value| value.as_u64());

        if let Some(turn) = self.usage.turns.last_mut() {
            let observed_duration = elapsed_ms(&turn.timestamp, Some(timestamp));
            turn.telemetry.outcome = outcome;
            turn.telemetry.duration_ms = if task_was_steered {
                observed_duration.or(native_duration)
            } else {
                native_duration.or(observed_duration)
            };
        }

        self.current_codex_turn_id = None;
        self.current_codex_task_first_turn_idx = None;
    }

    fn process_codex_subagent_activity(&mut self, payload: &serde_json::Value) {
        let event_id = payload
            .get("event_id")
            .and_then(|id| id.as_str())
            .unwrap_or("");
        let agent_path = payload
            .get("agent_path")
            .and_then(|path| path.as_str())
            .unwrap_or("");
        if event_id.is_empty() && agent_path.is_empty() {
            return;
        }

        let agent_id = if !event_id.is_empty() {
            event_id
        } else {
            agent_path
        };
        let existed = self.agents.iter().any(|agent| agent.id == agent_id);
        let display_name = agent_path
            .rsplit('/')
            .find(|part| !part.is_empty())
            .unwrap_or("subagent");
        self.ensure_named_agent(agent_id, display_name, "Subagent");
        if !agent_path.is_empty() {
            self.id_aliases
                .insert(agent_path.to_string(), agent_id.to_string());
        }

        let kind = payload
            .get("kind")
            .and_then(|kind| kind.as_str())
            .unwrap_or("started");
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == agent_id) {
            agent.status = if kind == "started" {
                AgentStatus::Thinking { dots: 0 }
            } else {
                AgentStatus::Idle
            };
        }
        if !existed {
            self.push_observed_message(
                "codex".to_string(),
                agent_id.to_string(),
                format!("{kind} {display_name}"),
            );
        }
    }

    fn start_codex_turn(&mut self, prompt: String, timestamp: String, turn_id: Option<String>) {
        self.finish_previous_codex_turn_at(&timestamp);
        self.commit_finished_attribution_history();
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
        let attribution = self.new_turn_attribution(&prompt);
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
            response_text: String::new(),
            cost_known: false,
            telemetry: TurnTelemetry {
                model: self.current_codex_model.clone(),
                ..TurnTelemetry::default()
            },
            attribution,
        });
        if let Some(turn) = self.usage.turns.last_mut() {
            for pending in std::mem::take(&mut self.pending_codex_attribution) {
                turn.attribution.observe(
                    pending.category,
                    pending.source,
                    pending.invocation,
                    pending.tokens,
                );
            }
        }
        self.push_observed_message("user".to_string(), "codex".to_string(), prompt);
    }

    fn observe_or_queue_codex_attribution(
        &mut self,
        category: AttributionCategory,
        source: impl Into<String>,
        invocation: impl Into<String>,
        tokens: u64,
    ) {
        if tokens == 0 {
            return;
        }
        let source = source.into();
        let invocation = invocation.into();
        let awaiting_user_turn = self.current_codex_task_first_turn_idx
            == Some(self.usage.turns.len())
            || self.usage.turns.is_empty();
        if awaiting_user_turn {
            self.pending_codex_attribution
                .push(PendingCodexAttribution {
                    category,
                    source,
                    invocation,
                    tokens,
                });
        } else if let Some(turn) = self.usage.turns.last_mut() {
            turn.attribution
                .observe(category, source, invocation, tokens);
        }
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

    fn apply_codex_usage(&mut self, payload: &serde_json::Value, timestamp: &str) {
        let Some(info) = payload.get("info") else {
            return;
        };
        let (usage, is_cumulative) = if let Some(usage) = info.get("last_token_usage") {
            (usage, false)
        } else if let Some(usage) = info.get("total_token_usage") {
            (usage, true)
        } else {
            return;
        };

        let mut input = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mut output = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mut cached = usage
            .get("cached_input_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let mut total = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or(0);
        let emitted_reasoning = usage
            .get("reasoning_output_tokens")
            .and_then(|v| v.as_u64());
        let mut reasoning = emitted_reasoning.unwrap_or(0);
        let context_window = info
            .get("model_context_window")
            .and_then(|value| value.as_u64());

        if is_cumulative {
            let current = CodexUsageSnapshot {
                input,
                output,
                cached,
                reasoning,
                total,
            };
            input = usage_delta(current.input, self.codex_total_usage.input);
            output = usage_delta(current.output, self.codex_total_usage.output);
            cached = usage_delta(current.cached, self.codex_total_usage.cached);
            reasoning = usage_delta(current.reasoning, self.codex_total_usage.reasoning);
            total = usage_delta(current.total, self.codex_total_usage.total);
            self.codex_total_usage = current;
        }

        let has_priced_token_split = input > 0 || output > 0 || cached > 0;
        let cost = if has_priced_token_split {
            self.current_codex_model.as_deref().and_then(|model| {
                compute_provider_cost_at(
                    self.provider,
                    model,
                    input,
                    output,
                    0,
                    cached,
                    event_pricing_date(Some(timestamp)),
                )
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
                // Codex reports cached input as a subset of input tokens.
                self.cumulative_context += input;
            }
            turn.cumulative_context = self.cumulative_context;
            turn.telemetry.model = self.current_codex_model.clone();
            turn.telemetry.reasoning_tokens += reasoning;
            turn.telemetry.reasoning_tokens_emitted |= emitted_reasoning.is_some();
            turn.telemetry.observe_context(input, context_window);
            if let Some(cost) = cost {
                turn.cost += cost;
                turn.cost_known = true;
            }
        }
        if input > 0 {
            self.record_parent_request_attribution(input, cached, true);
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
            let mut cache_write_5m: u64 = 0;
            let mut cache_write_1h: u64 = 0;
            let mut model = String::from("sonnet");
            let mut agent_prompt = String::new();
            let mut response_parts: Vec<String> = Vec::new();
            let mut outcome = TurnOutcome::InProgress;
            let mut terminal_ts: Option<String> = None;
            let mut tool_calls = 0_u32;
            let mut lines_added = 0_u64;
            let mut lines_removed = 0_u64;
            let mut files_created = 0_u32;
            let mut files_deleted = 0_u32;
            let mut pending_diffs: HashMap<String, FileChangeStats> = HashMap::new();
            let mut seen_usage_messages: HashSet<String> = HashSet::new();
            let mut subagent_tool_names: HashMap<String, String> = HashMap::new();
            let mut attribution = TurnAttribution::default();
            let attribution_actor = format!("Agent: {agent_type}");

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
                                attribution.set_prompt(s);
                            }
                        }
                    }
                    if line_type == "user" {
                        if let Some(content) = v
                            .get("message")
                            .and_then(|message| message.get("content"))
                            .and_then(|content| content.as_array())
                        {
                            for block in content {
                                let Some(tool_use_id) = block
                                    .get("tool_use_id")
                                    .filter(|_| {
                                        block.get("type").and_then(|kind| kind.as_str())
                                            == Some("tool_result")
                                    })
                                    .and_then(|id| id.as_str())
                                else {
                                    continue;
                                };
                                let tool_name = subagent_tool_names
                                    .get(tool_use_id)
                                    .cloned()
                                    .unwrap_or_else(|| "tool".to_string());
                                let (source, purpose) = tool_source_and_purpose(&tool_name, None);
                                attribution.flush_deferred();
                                attribution.observe(
                                    tool_attribution_category(&tool_name),
                                    source,
                                    format!("{} result", purpose),
                                    estimate_tool_result_tokens(block),
                                );
                                let Some(mut change) = pending_diffs.remove(tool_use_id) else {
                                    continue;
                                };
                                if !block
                                    .get("is_error")
                                    .and_then(|is_error| is_error.as_bool())
                                    .unwrap_or(false)
                                {
                                    change.observe_claude_result(block, &v);
                                    lines_added = lines_added.saturating_add(change.lines_added);
                                    lines_removed =
                                        lines_removed.saturating_add(change.lines_removed);
                                    files_created =
                                        files_created.saturating_add(change.files_created);
                                    files_deleted =
                                        files_deleted.saturating_add(change.files_deleted);
                                }
                            }
                        }
                    }

                    if line_type == "assistant" {
                        let message = v.get("message");
                        if let Some(m) = message
                            .and_then(|message| message.get("model"))
                            .or_else(|| v.get("model"))
                            .and_then(|model| model.as_str())
                        {
                            model = m.to_string();
                        }
                        if let Some(usage) = message.and_then(|message| message.get("usage")) {
                            let usage_is_new = message
                                .and_then(|message| message.get("id"))
                                .and_then(|id| id.as_str())
                                .map(|id| seen_usage_messages.insert(id.to_string()))
                                .unwrap_or(true);
                            if usage_is_new {
                                let request_input = usage
                                    .get("input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let request_cache_read = usage
                                    .get("cache_read_input_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                let (request_cache_write, five_minute, one_hour) =
                                    claude_cache_write_tokens(usage);
                                input_tokens += request_input;
                                output_tokens += usage
                                    .get("output_tokens")
                                    .and_then(|v| v.as_u64())
                                    .unwrap_or(0);
                                cache_read += request_cache_read;
                                cache_write_5m += five_minute;
                                cache_write_1h += one_hour;
                                attribution.record_request_for_actor(
                                    format!("claude-agent-{}", attribution.request_count() + 1),
                                    attribution_actor.clone(),
                                    request_input + request_cache_read + request_cache_write,
                                    request_cache_read + request_cache_write,
                                    true,
                                );
                            }
                        }
                        let stop_reason = message
                            .and_then(|message| message.get("stop_reason"))
                            .and_then(|reason| reason.as_str())
                            .unwrap_or("");
                        if let Some(terminal_outcome) =
                            claude_terminal_outcome(Some(model.as_str()), stop_reason)
                        {
                            outcome = terminal_outcome;
                            terminal_ts = v
                                .get("timestamp")
                                .and_then(|timestamp| timestamp.as_str())
                                .map(str::to_string);
                        }
                        // Extract text from assistant response
                        if let Some(content) = message.and_then(|message| message.get("content")) {
                            if let Some(arr) = content.as_array() {
                                for block in arr {
                                    match block.get("type").and_then(|kind| kind.as_str()) {
                                        Some("text") => {
                                            if let Some(text) =
                                                block.get("text").and_then(|text| text.as_str())
                                            {
                                                if !text.is_empty() {
                                                    response_parts.push(text.to_string());
                                                }
                                            }
                                        }
                                        Some("tool_use") => {
                                            tool_calls += 1;
                                            let name = block
                                                .get("name")
                                                .and_then(|name| name.as_str())
                                                .unwrap_or("tool");
                                            if let Some(id) =
                                                block.get("id").and_then(|id| id.as_str())
                                            {
                                                subagent_tool_names
                                                    .insert(id.to_string(), name.to_string());
                                            }
                                            let (source, purpose) =
                                                tool_source_and_purpose(name, block.get("input"));
                                            attribution.observe(
                                                tool_attribution_category(name),
                                                source,
                                                format!("{} #{}", purpose, tool_calls),
                                                estimate_tokens(&block.to_string()),
                                            );
                                            attribution
                                                .set_next_request_label(format!("After {purpose}"));
                                            if let (Some(id), Some(input), Some(name)) = (
                                                block.get("id").and_then(|id| id.as_str()),
                                                block.get("input"),
                                                block.get("name").and_then(|name| name.as_str()),
                                            ) {
                                                if let Some(change) =
                                                    claude_tool_file_change(name, input)
                                                {
                                                    pending_diffs.insert(id.to_string(), change);
                                                }
                                            }
                                        }
                                        _ => {}
                                    }
                                }
                            }
                        }
                    }
                }
            }

            // Don't mark as scanned until we have an assistant response
            if outcome == TurnOutcome::InProgress
                || (response_parts.is_empty() && output_tokens == 0)
            {
                continue;
            }

            // Mark as scanned now that we have data
            self.scanned_subagent_files.insert(path.clone());

            let cost = compute_provider_cost_with_cache_ttl_at(
                self.provider,
                &model,
                input_tokens,
                output_tokens,
                cache_write_5m,
                cache_write_1h,
                cache_read,
                event_pricing_date(first_ts.as_deref()),
            )
            .unwrap_or(0.0);
            let agent_requests: Vec<RequestAttribution> = (0..attribution.request_count())
                .filter_map(|request| attribution.request(request).cloned())
                .collect();
            let agent_cost = AgentCost {
                id: path.display().to_string(),
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
                role: "Subagent".to_string(),
                model: Some(model.clone()),
                cost,
                cost_known: compute_provider_cost_with_cache_ttl_at(
                    self.provider,
                    &model,
                    input_tokens,
                    output_tokens,
                    cache_write_5m,
                    cache_write_1h,
                    cache_read,
                    event_pricing_date(first_ts.as_deref()),
                )
                .is_some(),
                input_tokens,
                output_tokens,
                cache_read_tokens: cache_read,
                outcome,
                duration_ms: elapsed_ms(first_ts.as_deref().unwrap_or(""), terminal_ts.as_deref()),
                tool_calls,
                lines_added,
                lines_removed,
                files_created,
                files_deleted,
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
                    turn.attribution
                        .set_agent_requests(path.display().to_string(), agent_requests);
                    self.attribution_loaded = true;
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

    fn ensure_named_agent(&mut self, id: &str, name: &str, role: &str) {
        if let Some(agent) = self.agents.iter_mut().find(|agent| agent.id == id) {
            agent.name = name.to_string();
            agent.role = role.to_string();
            return;
        }
        let color = theme::AGENT_COLORS[self.color_idx % theme::AGENT_COLORS.len()];
        self.color_idx += 1;
        self.agents.push(Agent::new(id, name, role, color, vec![]));
    }

    fn push_observed_message(&mut self, from: String, to: String, content: String) {
        self.push_observed_message_typed(from, to, content, MessageType::Response);
    }

    fn push_observed_message_typed(
        &mut self,
        from: String,
        to: String,
        content: String,
        message_type: MessageType,
    ) {
        if self.messages.iter().any(|message| {
            message.from == from
                && message.to == to
                && message.content == content
                && message.message_type == message_type
        }) {
            return;
        }
        self.ensure_agent(&from, "Observed");
        self.ensure_agent(&to, "Observed");
        let id = self.next_message_id;
        self.next_message_id += 1;
        let mut message = Message::new(id, &from, &to, &content, message_type);
        message.revealed_chars = message.content.len();
        self.messages.push(message);
    }
}

/// Maximum number of sessions to track per provider (most recent by mtime).
const MAX_SESSIONS: usize = 50;
const ATTRIBUTION_CACHE_SESSIONS: usize = 8;

#[derive(Deserialize)]
struct CodexSessionIndexEntry {
    id: String,
    thread_name: String,
}

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
    codex_titles: HashMap<String, String>,
    codex_subagent_files: HashMap<PathBuf, CodexSubagentMetadata>,
    attribution_cache: VecDeque<PathBuf>,
    config: AetherConfig,
}

impl LiveEngine {
    pub fn new(provider: Option<ProviderKind>, dir_override: Option<PathBuf>) -> Self {
        let config = AetherConfig::load();
        let name_overrides = Self::load_name_overrides();
        let codex_titles = Self::load_codex_titles();
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
            codex_titles,
            codex_subagent_files: HashMap::new(),
            attribution_cache: VecDeque::new(),
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

    fn load_codex_titles() -> HashMap<String, String> {
        fs::read_to_string(codex_session_index_path())
            .map(|data| parse_codex_titles(&data))
            .unwrap_or_default()
    }

    fn apply_codex_titles(&mut self) {
        for session in self
            .sessions
            .iter_mut()
            .filter(|session| session.provider == ProviderKind::Codex)
        {
            if let Some(title) = self.codex_titles.get(&session.session_id) {
                session.apply_native_title(title);
            }
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

        let cached_paths: HashSet<PathBuf> = self.attribution_cache.iter().cloned().collect();
        // Poll active session every tick, others every ~5 seconds. Detailed attribution is
        // retained only for the bounded recent-session cache.
        for (i, session) in self.sessions.iter_mut().enumerate() {
            if i == self.active_idx || self.scan_cooldown % 100 == 0 {
                if i == self.active_idx && session.file_pos > 0 && !session.attribution_loaded {
                    session.rebuild_attribution();
                }
                session.poll_file();
                if i == self.active_idx {
                    session.attribution_loaded = true;
                } else if !cached_paths.contains(&session.file_path) {
                    session.drop_attribution();
                }
            }
        }
        if let Some(path) = self
            .sessions
            .get(self.active_idx)
            .map(|session| session.file_path.clone())
        {
            self.touch_attribution_cache(path);
        }
        self.apply_codex_titles();

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

    fn touch_attribution_cache(&mut self, path: PathBuf) {
        self.attribution_cache
            .retain(|candidate| candidate != &path);
        self.attribution_cache.push_back(path);
        while self.attribution_cache.len() > ATTRIBUTION_CACHE_SESSIONS {
            let Some(evicted) = self.attribution_cache.pop_front() else {
                break;
            };
            if let Some(session) = self
                .sessions
                .iter_mut()
                .find(|session| session.file_path == evicted)
            {
                session.drop_attribution();
            }
        }
    }

    pub fn reset(&mut self) {
        if let Some(session) = self.sessions.get_mut(self.active_idx) {
            session.clear_display();
            session.file_pos = 0;
            session.partial_line.clear();
            for subagent in &mut session.codex_subagent_files {
                let path = subagent.path.clone();
                let metadata = subagent.metadata.clone();
                let inherited_turn_ids = subagent.inherited_turn_ids.clone();
                *subagent = CodexSubagentFileState::new(path, metadata, inherited_turn_ids);
            }
        }
    }

    fn scan_sessions(&mut self) {
        // Setup may be run in another terminal while this watcher remains open.
        self.config = AetherConfig::load();

        let active_path = self
            .sessions
            .get(self.active_idx)
            .map(|session| session.file_path.clone());
        let providers: Vec<ProviderKind> = if let Some(provider) = self.active_provider {
            vec![provider]
        } else {
            ProviderKind::ALL.to_vec()
        };

        if providers.contains(&ProviderKind::Codex) {
            self.codex_titles = Self::load_codex_titles();
        }

        for provider in providers {
            if let Some(dir) = self.dir_override.clone() {
                self.scan_directory(&dir, provider, provider == ProviderKind::Codex);
                continue;
            }

            match provider {
                ProviderKind::Claude => {
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
        self.codex_subagent_files.retain(|path, _| path.exists());
        self.attach_codex_subagents();

        // Keep the backing vector stable so UI cursors do not silently change
        // identity on each rescan. Display ordering is handled separately.
        self.prune_sessions(active_path.as_deref());

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

    fn prune_sessions(&mut self, active_path: Option<&Path>) {
        let mut keep_paths = HashSet::new();
        let active_provider = active_path.and_then(|path| {
            self.sessions
                .iter()
                .find(|session| session.file_path == path)
                .map(|session| session.provider)
        });
        for provider in ProviderKind::ALL {
            let mut candidates: Vec<&SessionState> = self
                .sessions
                .iter()
                .filter(|session| session.provider == provider)
                .collect();
            candidates.sort_by(|a, b| b.last_modified.cmp(&a.last_modified));
            let mut selected: Vec<PathBuf> = candidates
                .into_iter()
                .take(MAX_SESSIONS)
                .map(|session| session.file_path.clone())
                .collect();
            if active_provider == Some(provider) {
                if let Some(path) = active_path.filter(|path| path.exists()) {
                    if !selected.iter().any(|candidate| candidate == path) {
                        if selected.len() == MAX_SESSIONS {
                            selected.pop();
                        }
                        selected.push(path.to_path_buf());
                    }
                }
            }
            keep_paths.extend(selected);
        }
        self.sessions
            .retain(|session| keep_paths.contains(&session.file_path));
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

            if provider == ProviderKind::Codex {
                if let Some(metadata) = codex_subagent_metadata(&path) {
                    self.sessions.retain(|session| session.file_path != path);
                    self.codex_subagent_files.insert(path, metadata);
                    continue;
                }
            }
            let already_tracked = self.sessions.iter().any(|s| s.file_path == path);
            if !already_tracked {
                let session = self.new_session(&path, provider);
                self.sessions.push(session);
            }
        }
    }

    fn attach_codex_subagents(&mut self) {
        let discovered: Vec<(PathBuf, CodexSubagentMetadata)> = self
            .codex_subagent_files
            .iter()
            .map(|(path, metadata)| (path.clone(), metadata.clone()))
            .collect();
        let mut thread_paths: HashMap<String, PathBuf> = self
            .sessions
            .iter()
            .filter(|session| session.provider == ProviderKind::Codex)
            .map(|session| (session.session_id.clone(), session.file_path.clone()))
            .collect();
        thread_paths.extend(
            discovered
                .iter()
                .map(|(path, metadata)| (metadata.session_id.clone(), path.clone())),
        );
        let mut turn_id_cache: HashMap<PathBuf, HashSet<String>> = HashMap::new();

        for (path, metadata) in discovered {
            if self.sessions.iter().any(|session| {
                session
                    .codex_subagent_files
                    .iter()
                    .any(|subagent| subagent.path == path)
            }) {
                continue;
            }
            let Some(parent_session_id) = self.codex_root_parent_id(&metadata) else {
                continue;
            };
            let inherited_turn_ids = thread_paths
                .get(&metadata.parent_session_id)
                .map(|parent_path| {
                    turn_id_cache
                        .entry(parent_path.clone())
                        .or_insert_with(|| codex_turn_ids(parent_path))
                        .clone()
                })
                .unwrap_or_default();
            if let Some(parent) = self.sessions.iter_mut().find(|session| {
                session.provider == ProviderKind::Codex && session.session_id == parent_session_id
            }) {
                parent.attach_codex_subagent(path, metadata, inherited_turn_ids);
            }
        }
    }

    fn codex_root_parent_id(&self, metadata: &CodexSubagentMetadata) -> Option<String> {
        let mut parent_id = metadata.parent_session_id.clone();
        let mut visited = HashSet::new();
        while visited.insert(parent_id.clone()) {
            if self.sessions.iter().any(|session| {
                session.provider == ProviderKind::Codex && session.session_id == parent_id
            }) {
                return Some(parent_id);
            }
            let parent_subagent = self
                .codex_subagent_files
                .values()
                .find(|candidate| candidate.session_id == parent_id)?;
            parent_id = parent_subagent.parent_session_id.clone();
        }
        None
    }

    fn new_session(&self, path: &PathBuf, provider: ProviderKind) -> SessionState {
        let discovery = session_file_metadata(path, provider);
        let mut session = SessionState::new(path.clone(), provider);
        if let Some(session_id) = discovery.session_id {
            session.session_id = session_id;
        }
        session.project_path = discovery.project_path;
        if let Ok(meta) = fs::metadata(path) {
            if let Ok(modified) = meta.modified() {
                session.last_modified = modified
                    .duration_since(SystemTime::UNIX_EPOCH)
                    .map(|duration| duration.as_secs())
                    .unwrap_or(0);
            }
        }
        let key = Self::session_override_key(provider, path);
        if let Some(custom_name) = self.name_overrides.get(&key) {
            session.name = custom_name.clone();
            session.name_override = Some(custom_name.clone());
        } else if provider == ProviderKind::Codex {
            if let Some(title) = self.codex_titles.get(&session.session_id) {
                session.apply_native_title(title);
            } else if let Some(title) = discovery.native_title {
                session.apply_native_title(&title);
            }
        } else if let Some(title) = discovery.native_title {
            session.apply_native_title(&title);
        }
        if session.name
            == session
                .file_path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("")
        {
            if let Some(project_name) = session
                .project_path
                .as_ref()
                .and_then(|project| project.file_name())
                .and_then(|name| name.to_str())
            {
                session.name = project_name.to_string();
            }
        }
        session
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
            if !s.usage.turns.is_empty() || !s.agents.is_empty() || !s.messages.is_empty() {
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
            if !s.usage.turns.is_empty() || !s.agents.is_empty() || !s.messages.is_empty() {
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

    /// Sessions grouped by project recency, then by session recency within each project.
    pub fn active_sessions(&self) -> impl Iterator<Item = (usize, &SessionState)> {
        self.active_session_indices()
            .into_iter()
            .map(|idx| (idx, &self.sessions[idx]))
    }

    pub fn active_session_name(&self) -> &str {
        self.active_session()
            .map(|s| s.name.as_str())
            .unwrap_or("none")
    }

    pub fn active_session_position(&self) -> Option<usize> {
        self.active_session_indices()
            .iter()
            .position(|idx| *idx == self.active_idx)
            .map(|position| position + 1)
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
                let last_activity = self
                    .sessions
                    .iter()
                    .filter(|session| session.provider == *provider)
                    .map(|session| session.last_activity)
                    .max()
                    .unwrap_or(0);
                ProviderStatus {
                    kind: *provider,
                    enabled: self.config.is_enabled(*provider),
                    available: self.provider_available(*provider),
                    session_count,
                    last_activity,
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
        let provider = self.active_provider;
        let mut indices: Vec<usize> = self
            .sessions
            .iter()
            .enumerate()
            .filter(|(_, session)| provider.map(|p| session.provider == p).unwrap_or(true))
            .map(|(idx, _)| idx)
            .collect();
        let mut project_recency: HashMap<String, u64> = HashMap::new();
        for idx in &indices {
            let session = &self.sessions[*idx];
            let key = session.project_display_path();
            project_recency
                .entry(key)
                .and_modify(|modified| *modified = (*modified).max(session.last_modified))
                .or_insert(session.last_modified);
        }
        indices.sort_by(|a, b| {
            let a_session = &self.sessions[*a];
            let b_session = &self.sessions[*b];
            let a_project = a_session.project_display_path();
            let b_project = b_session.project_display_path();
            project_recency
                .get(&b_project)
                .cmp(&project_recency.get(&a_project))
                .then_with(|| a_project.cmp(&b_project))
                .then_with(|| b_session.last_modified.cmp(&a_session.last_modified))
        });
        indices
    }

    fn provider_available(&self, provider: ProviderKind) -> bool {
        match provider {
            ProviderKind::Claude => claude_projects_dir().exists(),
            ProviderKind::Codex => codex_sessions_dir().exists(),
        }
    }
}

fn codex_record_turn_id(value: &serde_json::Value) -> Option<&str> {
    let payload = value.get("payload")?;
    let is_turn_boundary = value.get("type").and_then(|kind| kind.as_str()) == Some("turn_context")
        || (value.get("type").and_then(|kind| kind.as_str()) == Some("event_msg")
            && payload.get("type").and_then(|kind| kind.as_str()) == Some("task_started"));
    is_turn_boundary
        .then(|| payload.get("turn_id").and_then(|turn_id| turn_id.as_str()))
        .flatten()
}

fn codex_turn_ids(path: &Path) -> HashSet<String> {
    let Ok(file) = File::open(path) else {
        return HashSet::new();
    };
    BufReader::new(file)
        .lines()
        .map_while(Result::ok)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .filter_map(|value| codex_record_turn_id(&value).map(str::to_string))
        .collect()
}

fn codex_subagent_metadata(path: &Path) -> Option<CodexSubagentMetadata> {
    let file = File::open(path).ok()?;
    let value = BufReader::new(file)
        .lines()
        .take(8)
        .filter_map(Result::ok)
        .filter_map(|line| serde_json::from_str::<serde_json::Value>(&line).ok())
        .find(|value| value.get("type").and_then(|kind| kind.as_str()) == Some("session_meta"))?;
    if !codex_session_meta_is_subagent(&value) {
        return None;
    }

    let payload = value.get("payload").unwrap_or(&value);
    let spawn = payload
        .get("source")
        .and_then(|source| source.get("subagent"))
        .and_then(|subagent| subagent.get("thread_spawn"));
    let session_id = payload.get("id").and_then(|id| id.as_str())?;
    let parent_session_id = payload
        .get("parent_thread_id")
        .and_then(|id| id.as_str())
        .or_else(|| {
            spawn
                .and_then(|spawn| spawn.get("parent_thread_id"))
                .and_then(|id| id.as_str())
        })?;
    let nickname = payload
        .get("agent_nickname")
        .and_then(|nickname| nickname.as_str())
        .or_else(|| {
            spawn
                .and_then(|spawn| spawn.get("agent_nickname"))
                .and_then(|nickname| nickname.as_str())
        })
        .filter(|nickname| !nickname.trim().is_empty())
        .unwrap_or("subagent");
    let role = payload
        .get("agent_role")
        .and_then(|role| role.as_str())
        .or_else(|| {
            spawn
                .and_then(|spawn| spawn.get("agent_role"))
                .and_then(|role| role.as_str())
        })
        .filter(|role| !role.trim().is_empty())
        .unwrap_or("Subagent");

    Some(CodexSubagentMetadata {
        session_id: session_id.to_string(),
        parent_session_id: parent_session_id.to_string(),
        nickname: nickname.to_string(),
        role: role.to_string(),
        started_at: value
            .get("timestamp")
            .and_then(|timestamp| timestamp.as_str())
            .unwrap_or("")
            .to_string(),
    })
}

#[derive(Default)]
struct SessionFileMetadata {
    session_id: Option<String>,
    project_path: Option<PathBuf>,
    native_title: Option<String>,
}

fn session_file_metadata(path: &Path, provider: ProviderKind) -> SessionFileMetadata {
    let file = match File::open(path) {
        Ok(file) => file,
        Err(_) => return SessionFileMetadata::default(),
    };
    let mut metadata = SessionFileMetadata::default();
    let mut claude_custom_title = None;
    let mut claude_ai_title = None;
    let mut claude_fallback_title = None;

    for line in BufReader::new(file).lines().filter_map(Result::ok) {
        let value = match serde_json::from_str::<serde_json::Value>(&line) {
            Ok(value) => value,
            Err(_) => continue,
        };
        let line_type = value
            .get("type")
            .and_then(|kind| kind.as_str())
            .unwrap_or("");

        match provider {
            ProviderKind::Codex => {
                match line_type {
                    "session_meta" => {
                        let payload = value.get("payload").unwrap_or(&value);
                        metadata.session_id = payload
                            .get("id")
                            .and_then(|id| id.as_str())
                            .map(str::to_string);
                        metadata.project_path = payload
                            .get("cwd")
                            .and_then(|cwd| cwd.as_str())
                            .filter(|cwd| !cwd.trim().is_empty())
                            .map(PathBuf::from);
                    }
                    "response_item" if metadata.native_title.is_none() => {
                        let Some(payload) = value.get("payload") else {
                            continue;
                        };
                        if payload.get("type").and_then(|kind| kind.as_str()) == Some("message")
                            && payload.get("role").and_then(|role| role.as_str()) == Some("user")
                        {
                            let prompt = codex_user_message_text(payload);
                            if !prompt.trim().is_empty()
                                && !is_synthetic_codex_user_message(&prompt)
                            {
                                metadata.native_title = Some(title_preview(&prompt, 40));
                            }
                        }
                    }
                    "event_msg" if metadata.native_title.is_none() => {
                        let Some(payload) = value.get("payload") else {
                            continue;
                        };
                        if payload.get("type").and_then(|kind| kind.as_str())
                            == Some("user_message")
                        {
                            if let Some(prompt) =
                                payload.get("message").and_then(|text| text.as_str())
                            {
                                if !prompt.trim().is_empty()
                                    && !is_synthetic_codex_user_message(prompt)
                                {
                                    metadata.native_title = Some(title_preview(prompt, 40));
                                }
                            }
                        }
                    }
                    _ => {}
                }
                if metadata.session_id.is_some()
                    && metadata.project_path.is_some()
                    && metadata.native_title.is_some()
                {
                    break;
                }
            }
            ProviderKind::Claude => {
                if metadata.project_path.is_none() {
                    metadata.project_path = value
                        .get("cwd")
                        .and_then(|cwd| cwd.as_str())
                        .filter(|cwd| !cwd.trim().is_empty())
                        .map(PathBuf::from);
                }
                if metadata.session_id.is_none() {
                    metadata.session_id = value
                        .get("sessionId")
                        .or_else(|| value.get("session_id"))
                        .and_then(|id| id.as_str())
                        .map(str::to_string);
                }

                match line_type {
                    "session_start" => {
                        if claude_fallback_title.is_none() {
                            claude_fallback_title = value
                                .get("name")
                                .and_then(|title| title.as_str())
                                .and_then(nonempty_title);
                        }
                    }
                    "custom-title" => {
                        claude_custom_title = value
                            .get("customTitle")
                            .and_then(|title| title.as_str())
                            .and_then(nonempty_title);
                    }
                    "ai-title" => {
                        claude_ai_title = value
                            .get("aiTitle")
                            .and_then(|title| title.as_str())
                            .and_then(nonempty_title);
                    }
                    "user" if claude_fallback_title.is_none() => {
                        let is_external = value
                            .get("userType")
                            .and_then(|user_type| user_type.as_str())
                            == Some("external");
                        if is_external {
                            claude_fallback_title = value
                                .get("message")
                                .and_then(|message| message.get("content"))
                                .and_then(|content| content.as_str())
                                .filter(|content| !is_synthetic_codex_user_message(content))
                                .map(|content| title_preview(content, 40));
                        }
                    }
                    _ => {}
                }
            }
        }
    }

    if provider == ProviderKind::Claude {
        metadata.native_title = claude_custom_title
            .or(claude_ai_title)
            .or(claude_fallback_title);
    }
    metadata
}

fn nonempty_title(title: &str) -> Option<String> {
    let title = title.trim();
    (!title.is_empty()).then(|| title.to_string())
}

fn title_preview(title: &str, max_chars: usize) -> String {
    title
        .chars()
        .filter(|character| !character.is_control())
        .take(max_chars)
        .collect()
}

fn codex_session_meta_is_subagent(value: &serde_json::Value) -> bool {
    let payload = value.get("payload").unwrap_or(value);
    payload
        .get("thread_source")
        .and_then(|source| source.as_str())
        == Some("subagent")
        || payload.get("source").and_then(|source| source.as_str()) == Some("subagent")
        || payload
            .get("source")
            .and_then(|source| source.get("subagent"))
            .is_some()
}

fn tool_attribution_category(name: &str) -> AttributionCategory {
    let normalized = name.to_ascii_lowercase();
    if [
        "task",
        "agent",
        "spawn_agent",
        "send_input",
        "wait_agent",
        "close_agent",
    ]
    .iter()
    .any(|operation| {
        normalized == *operation
            || normalized.ends_with(&format!("__{operation}"))
            || normalized.ends_with(&format!("_{operation}"))
    }) {
        AttributionCategory::Agents
    } else {
        AttributionCategory::ToolsAndMcps
    }
}

fn tool_attribution_category_for_call(
    name: &str,
    input: Option<&serde_json::Value>,
) -> AttributionCategory {
    if name.eq_ignore_ascii_case("exec") {
        let nested = input
            .and_then(|value| value.as_str())
            .map(nested_tool_names)
            .unwrap_or_default();
        if !nested.is_empty()
            && nested
                .iter()
                .all(|tool| tool_attribution_category(tool) == AttributionCategory::Agents)
        {
            return AttributionCategory::Agents;
        }
    }
    tool_attribution_category(name)
}

fn tool_source_and_purpose(name: &str, input: Option<&serde_json::Value>) -> (String, String) {
    if name.eq_ignore_ascii_case("exec") {
        return codex_exec_source_and_purpose(input);
    }
    if let Some(rest) = name.strip_prefix("mcp__") {
        let mut parts = rest.splitn(2, "__");
        let server = parts.next().unwrap_or("MCP");
        let tool = parts.next().unwrap_or("call");
        return (
            format!("MCP: {}", readable_event_name(server)),
            readable_event_name(tool),
        );
    }

    let readable_name = readable_event_name(name);
    let normalized = name.to_ascii_lowercase();
    let purpose = if let Some(purpose) = agent_tool_purpose(&normalized) {
        purpose.to_string()
    } else if matches!(
        normalized.as_str(),
        "read" | "write" | "edit" | "multiedit" | "notebookedit" | "delete"
    ) {
        safe_input_field(input, &["file_path", "path", "notebook_path"])
            .and_then(|path| safe_basename(&path))
            .map(|file| format!("{readable_name} {file}"))
            .unwrap_or_else(|| readable_name.clone())
    } else if matches!(normalized.as_str(), "bash" | "shell" | "exec_command") {
        safe_input_field(input, &["command", "cmd"])
            .and_then(|command| safe_command_purpose(&command))
            .unwrap_or_else(|| readable_name.clone())
    } else {
        readable_name.clone()
    };

    let source = if tool_attribution_category(name) == AttributionCategory::Agents {
        "Agent orchestration".to_string()
    } else if matches!(normalized.as_str(), "bash" | "shell" | "exec_command") {
        "Terminal".to_string()
    } else if normalized == "apply_patch" {
        "File changes".to_string()
    } else {
        readable_name
    };
    (source, purpose)
}

fn agent_tool_purpose(normalized: &str) -> Option<&'static str> {
    if normalized.ends_with("spawn_agent") || normalized == "task" || normalized == "agent" {
        Some("Start agent")
    } else if normalized.ends_with("send_input") {
        Some("Message agent")
    } else if normalized.ends_with("wait_agent") {
        Some("Wait for agent")
    } else if normalized.ends_with("close_agent") {
        Some("Close agent")
    } else {
        None
    }
}

fn codex_exec_source_and_purpose(input: Option<&serde_json::Value>) -> (String, String) {
    let Some(script) = input.and_then(|value| value.as_str()) else {
        return ("Codex tools".to_string(), "Tool operation".to_string());
    };
    let nested_tools = nested_tool_names(script);
    if nested_tools.len() != 1 {
        return if nested_tools.is_empty() {
            ("Codex tools".to_string(), "Tool operation".to_string())
        } else if nested_tools
            .iter()
            .all(|tool| tool_attribution_category(tool) == AttributionCategory::Agents)
        {
            (
                "Agent orchestration".to_string(),
                format!("{} agent operation types", nested_tools.len()),
            )
        } else {
            (
                "Tool batch".to_string(),
                format!("{} tool types", nested_tools.len()),
            )
        };
    }

    match nested_tools[0].as_str() {
        "exec_command" => (
            "Terminal".to_string(),
            safe_script_command_purpose(script).unwrap_or_else(|| "Run command".to_string()),
        ),
        "write_stdin" => ("Terminal".to_string(), "Continue command".to_string()),
        "wait" => ("Terminal".to_string(), "Wait for command".to_string()),
        "apply_patch" => ("File changes".to_string(), "Apply patch".to_string()),
        "web__run" => ("Web".to_string(), "Web request".to_string()),
        "view_image" => ("Images".to_string(), "Inspect image".to_string()),
        "image_gen__imagegen" => ("Images".to_string(), "Generate image".to_string()),
        "request_user_input" => ("User interaction".to_string(), "Ask user".to_string()),
        nested => tool_source_and_purpose(nested, None),
    }
}

fn nested_tool_names(script: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut remaining = script;
    while let Some(offset) = remaining.find("tools.") {
        let after_prefix = &remaining[offset + "tools.".len()..];
        let name_len = after_prefix
            .chars()
            .take_while(|character| character.is_ascii_alphanumeric() || *character == '_')
            .map(char::len_utf8)
            .sum::<usize>();
        if name_len == 0 {
            remaining = &after_prefix[after_prefix.len().min(1)..];
            continue;
        }
        let name = &after_prefix[..name_len];
        if after_prefix[name_len..].trim_start().starts_with('(')
            && !names.iter().any(|candidate| candidate == name)
        {
            names.push(name.to_string());
        }
        remaining = &after_prefix[name_len..];
    }
    names
}

fn safe_script_command_purpose(script: &str) -> Option<String> {
    js_property_string(script, "cmd")
        .or_else(|| js_array_first_string_after(script, "const cmds"))
        .or_else(|| js_array_first_string_after(script, "const commands"))
        .and_then(|command| safe_command_purpose(&command))
}

fn js_property_string(script: &str, property: &str) -> Option<String> {
    for (offset, _) in script.match_indices(property) {
        let before = script[..offset].chars().next_back();
        let after_name = &script[offset + property.len()..];
        if before.is_some_and(|character| character.is_ascii_alphanumeric() || character == '_') {
            continue;
        }
        let after_name = after_name.trim_start();
        let Some(after_colon) = after_name.strip_prefix(':') else {
            continue;
        };
        if let Some(value) = parse_js_quoted_string(after_colon.trim_start()) {
            return Some(value);
        }
    }
    None
}

fn js_array_first_string_after(script: &str, marker: &str) -> Option<String> {
    let tail = script.split_once(marker)?.1;
    let array = tail.split_once('[')?.1.trim_start();
    let value = array.strip_prefix('[').unwrap_or(array).trim_start();
    parse_js_quoted_string(value)
}

fn parse_js_quoted_string(value: &str) -> Option<String> {
    let quote = value.chars().next()?;
    if !matches!(quote, '\'' | '"' | '`') {
        return None;
    }
    let mut escaped = false;
    let mut output = String::new();
    for character in value[quote.len_utf8()..].chars() {
        if escaped {
            output.push(match character {
                'n' => '\n',
                'r' => '\r',
                't' => '\t',
                other => other,
            });
            escaped = false;
        } else if character == '\\' {
            escaped = true;
        } else if character == quote {
            return Some(output);
        } else {
            output.push(character);
        }
    }
    None
}

fn safe_input_field(input: Option<&serde_json::Value>, keys: &[&str]) -> Option<String> {
    let input = input?;
    if let Some(object) = input.as_object() {
        return keys.iter().find_map(|key| {
            object
                .get(*key)
                .and_then(|value| value.as_str())
                .map(str::to_string)
        });
    }
    let encoded = input.as_str()?;
    let parsed: serde_json::Value = serde_json::from_str(encoded).ok()?;
    safe_input_field(Some(&parsed), keys)
}

fn safe_basename(path: &str) -> Option<String> {
    Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .map(|name| {
            name.chars()
                .filter(|ch| !ch.is_control())
                .take(40)
                .collect()
        })
}

fn safe_command_purpose(command: &str) -> Option<String> {
    const SAFE_SUBCOMMANDS: &[&str] = &[
        "build", "check", "test", "run", "status", "diff", "log", "install", "lint", "fmt",
    ];
    let mut words = command.split_whitespace();
    let executable = words.next()?;
    let executable = Path::new(executable)
        .file_name()
        .and_then(|name| name.to_str())?;
    if !executable
        .chars()
        .all(|character| character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.'))
    {
        return None;
    }
    let second = words.next().filter(|word| SAFE_SUBCOMMANDS.contains(word));
    Some(match second {
        Some(subcommand) => format!("{executable} {subcommand}"),
        None => executable.to_string(),
    })
}

fn readable_event_name(name: &str) -> String {
    let value = name
        .replace("__", " ")
        .replace(['_', '-'], " ")
        .split_whitespace()
        .take(4)
        .collect::<Vec<_>>()
        .join(" ");
    if value.is_empty() {
        "provider event".to_string()
    } else {
        value
    }
}

fn nested_text(value: &serde_json::Value, keys: &[&str]) -> Option<String> {
    for key in keys {
        if let Some(found) = value.get(*key) {
            if let Some(text) = content_text_opt(Some(found)) {
                if !text.trim().is_empty() {
                    return Some(text);
                }
            }
            if found.is_array() || found.is_object() {
                return Some(found.to_string());
            }
        }
    }
    for container in ["payload", "message", "compactMetadata", "metadata"] {
        if let Some(child) = value.get(container) {
            if let Some(text) = nested_text(child, keys) {
                return Some(text);
            }
        }
    }
    None
}

fn compact_summary(value: &serde_json::Value) -> Option<String> {
    nested_text(
        value,
        &[
            "compact_summary",
            "compactSummary",
            "summary",
            "replacement_history",
            "replacementHistory",
        ],
    )
}

fn compaction_source(value: &serde_json::Value) -> String {
    let mut candidates = vec![value];
    for key in ["payload", "compactMetadata", "metadata"] {
        if let Some(child) = value.get(key) {
            candidates.push(child);
        }
    }
    for candidate in candidates {
        if candidate.get("manual").and_then(|flag| flag.as_bool()) == Some(true) {
            return "Manual compaction".to_string();
        }
        for key in ["trigger", "reason", "initiator", "mode"] {
            let Some(kind) = candidate.get(key).and_then(|kind| kind.as_str()) else {
                continue;
            };
            let kind = kind.to_ascii_lowercase();
            if kind.contains("manual") || kind.contains("user") {
                return "Manual compaction".to_string();
            }
            if kind.contains("auto") || kind.contains("limit") || kind.contains("threshold") {
                return "Automatic compaction".to_string();
            }
        }
    }
    "Compacted summary".to_string()
}

fn is_claude_compaction_record(value: &serde_json::Value) -> bool {
    value
        .get("subtype")
        .and_then(|subtype| subtype.as_str())
        .is_some_and(|subtype| subtype.contains("compact"))
        || value
            .get("isCompactSummary")
            .and_then(|flag| flag.as_bool())
            .unwrap_or(false)
}

fn claude_hook_context(value: &serde_json::Value) -> Option<(String, String)> {
    let context = nested_text(
        value,
        &[
            "hookAdditionalContext",
            "additional_context",
            "additionalContext",
        ],
    )?;
    let event = value
        .get("hook_event_name")
        .or_else(|| value.get("hookEventName"))
        .or_else(|| value.get("subtype"))
        .and_then(|event| event.as_str())
        .map(readable_event_name)
        .unwrap_or_else(|| "Claude hook".to_string());
    Some((event, context))
}

fn claude_memory_context(value: &serde_json::Value) -> Option<(String, String)> {
    let subtype = value.get("subtype").and_then(|subtype| subtype.as_str())?;
    if !subtype.to_ascii_lowercase().contains("memory") {
        return None;
    }
    let context = nested_text(value, &["content", "memory", "message"])?;
    let source = nested_text(value, &["path", "file"])
        .and_then(|path| safe_basename(&path))
        .unwrap_or_else(|| "Claude memory".to_string());
    Some((source, context))
}

fn claude_external_prompt(value: &serde_json::Value) -> Option<String> {
    let content = value.get("message")?.get("content")?;
    if let Some(text) = content.as_str() {
        return Some(text.to_string());
    }
    let parts = content
        .as_array()?
        .iter()
        .filter(|block| block.get("type").and_then(|kind| kind.as_str()) != Some("tool_result"))
        .filter_map(|block| {
            block
                .get("text")
                .or_else(|| block.get("content"))
                .and_then(|text| text.as_str())
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        (!claude_direct_documents(value).is_empty()).then(|| "Attached document".to_string())
    } else {
        Some(parts.join("\n"))
    }
}

const NATIVE_IMAGE_ESTIMATE_TOKENS: u64 = 1_024;

fn estimate_tool_result_tokens(value: &serde_json::Value) -> u64 {
    let visible = value
        .get("output")
        .or_else(|| value.get("content"))
        .unwrap_or(value);
    estimate_model_visible_value(visible)
}

fn estimate_model_visible_value(value: &serde_json::Value) -> u64 {
    match value {
        serde_json::Value::Null => 0,
        serde_json::Value::Bool(_) | serde_json::Value::Number(_) => {
            estimate_tokens(&value.to_string())
        }
        serde_json::Value::String(text) => {
            if text.starts_with("data:image/") {
                NATIVE_IMAGE_ESTIMATE_TOKENS
            } else {
                estimate_tokens(text)
            }
        }
        serde_json::Value::Array(items) => items
            .iter()
            .map(estimate_model_visible_value)
            .fold(0_u64, u64::saturating_add),
        serde_json::Value::Object(fields) => {
            if matches!(
                fields.get("type").and_then(|kind| kind.as_str()),
                Some("image" | "input_image" | "local_image")
            ) {
                return NATIVE_IMAGE_ESTIMATE_TOKENS;
            }
            fields
                .iter()
                .filter(|(key, _)| {
                    !matches!(
                        key.as_str(),
                        "type"
                            | "id"
                            | "call_id"
                            | "tool_use_id"
                            | "is_error"
                            | "internal_chat_message_metadata_passthrough"
                    )
                })
                .map(|(_, value)| estimate_model_visible_value(value))
                .fold(0_u64, u64::saturating_add)
        }
    }
}

fn claude_direct_documents(value: &serde_json::Value) -> Vec<(String, u64)> {
    let Some(content) = value
        .get("message")
        .and_then(|message| message.get("content"))
        .and_then(|content| content.as_array())
    else {
        return Vec::new();
    };
    content
        .iter()
        .enumerate()
        .filter(|(_, block)| {
            matches!(
                block.get("type").and_then(|kind| kind.as_str()),
                Some("document" | "file" | "image")
            )
        })
        .map(|(index, block)| {
            let is_image = block.get("type").and_then(|kind| kind.as_str()) == Some("image");
            let name = ["title", "name", "filename"]
                .into_iter()
                .find_map(|key| block.get(key).and_then(|value| value.as_str()))
                .and_then(safe_basename)
                .unwrap_or_else(|| {
                    format!(
                        "{} {}",
                        if is_image { "Image" } else { "Document" },
                        index + 1
                    )
                });
            (
                name,
                if is_image {
                    NATIVE_IMAGE_ESTIMATE_TOKENS
                } else {
                    estimate_tokens(&block.to_string())
                },
            )
        })
        .collect()
}

fn codex_direct_documents(payload: &serde_json::Value) -> Vec<(String, u64)> {
    let Some(content) = payload
        .get("content")
        .and_then(|content| content.as_array())
    else {
        return Vec::new();
    };
    content
        .iter()
        .enumerate()
        .filter(|(_, block)| {
            matches!(
                block.get("type").and_then(|kind| kind.as_str()),
                Some("document" | "input_file" | "file" | "input_image" | "image")
            )
        })
        .map(|(index, block)| {
            let is_image = matches!(
                block.get("type").and_then(|kind| kind.as_str()),
                Some("input_image" | "image")
            );
            let name = ["filename", "name", "title"]
                .into_iter()
                .find_map(|key| block.get(key).and_then(|value| value.as_str()))
                .and_then(safe_basename)
                .unwrap_or_else(|| {
                    format!(
                        "{} {}",
                        if is_image { "Image" } else { "Document" },
                        index + 1
                    )
                });
            (
                name,
                if is_image {
                    NATIVE_IMAGE_ESTIMATE_TOKENS
                } else {
                    estimate_tokens(&block.to_string())
                },
            )
        })
        .collect()
}

fn codex_event_documents(payload: &serde_json::Value) -> Vec<(String, u64)> {
    let local_images = payload
        .get("local_images")
        .and_then(|images| images.as_array())
        .filter(|images| !images.is_empty());
    let images = local_images.or_else(|| {
        payload
            .get("images")
            .and_then(|images| images.as_array())
            .filter(|images| !images.is_empty())
    });
    let Some(images) = images else {
        return Vec::new();
    };

    images
        .iter()
        .enumerate()
        .map(|(index, image)| {
            let name = image
                .as_str()
                .filter(|value| !value.starts_with("data:") && !value.starts_with("http"))
                .and_then(safe_basename)
                .or_else(|| {
                    ["filename", "name", "path"]
                        .into_iter()
                        .find_map(|key| image.get(key).and_then(|value| value.as_str()))
                        .and_then(safe_basename)
                })
                .unwrap_or_else(|| format!("Image {}", index + 1));
            (name, NATIVE_IMAGE_ESTIMATE_TOKENS)
        })
        .collect()
}

fn codex_user_message_text(payload: &serde_json::Value) -> String {
    let Some(content) = payload
        .get("content")
        .and_then(|content| content.as_array())
    else {
        return content_text(payload.get("content"));
    };
    let parts = content
        .iter()
        .filter_map(|block| block.get("text").and_then(|text| text.as_str()))
        .filter(|text| {
            let trimmed = text.trim();
            !(trimmed.starts_with("<image ") || trimmed == "</image>")
        })
        .collect::<Vec<_>>();
    if parts.is_empty() {
        content_text(payload.get("content"))
    } else {
        parts.join("\n")
    }
}

fn codex_agents_md_context(payload: &serde_json::Value) -> Option<String> {
    payload
        .get("content")?
        .as_array()?
        .iter()
        .filter_map(|block| block.get("text").and_then(|text| text.as_str()))
        .find(|text| text.contains("# AGENTS.md instructions for "))
        .map(ToOwned::to_owned)
}

fn content_text(value: Option<&serde_json::Value>) -> String {
    content_text_opt(value).unwrap_or_default()
}

fn claude_terminal_outcome(model: Option<&str>, stop_reason: &str) -> Option<TurnOutcome> {
    if model == Some("<synthetic>") || matches!(stop_reason, "error" | "refusal") {
        Some(TurnOutcome::Failed)
    } else if matches!(stop_reason, "end_turn" | "stop_sequence") {
        Some(TurnOutcome::Completed)
    } else if matches!(stop_reason, "max_tokens" | "model_context_window_exceeded") {
        Some(TurnOutcome::Aborted)
    } else {
        None
    }
}

fn text_line_count(value: &str) -> u64 {
    if value.is_empty() {
        0
    } else {
        value.lines().count() as u64
    }
}

fn claude_tool_file_change(name: &str, input: &serde_json::Value) -> Option<FileChangeStats> {
    match name {
        "Edit" => {
            let lines_added = input
                .get("new_string")
                .and_then(|value| value.as_str())
                .map(text_line_count)
                .unwrap_or(0);
            let lines_removed = input
                .get("old_string")
                .and_then(|value| value.as_str())
                .map(text_line_count)
                .unwrap_or(0);
            Some(FileChangeStats {
                lines_added,
                lines_removed,
                ..FileChangeStats::default()
            })
        }
        "Write" => input
            .get("content")
            .and_then(|value| value.as_str())
            .map(|content| FileChangeStats {
                lines_added: text_line_count(content),
                ..FileChangeStats::default()
            }),
        "NotebookEdit" => input
            .get("new_source")
            .and_then(|value| value.as_str())
            .map(|content| FileChangeStats {
                lines_added: text_line_count(content),
                ..FileChangeStats::default()
            }),
        "MultiEdit" => {
            let edits = input.get("edits").and_then(|value| value.as_array())?;
            Some(
                edits
                    .iter()
                    .fold(FileChangeStats::default(), |mut total, edit| {
                        if let Some(change) = claude_tool_file_change("Edit", edit) {
                            total.add(change);
                        }
                        total
                    }),
            )
        }
        "Delete" => Some(FileChangeStats::default()),
        _ => None,
    }
}

fn unified_diff_line_counts(diff: &str) -> (u64, u64) {
    diff.lines().fold((0_u64, 0_u64), |(added, removed), line| {
        if line.starts_with("+++") || line.starts_with("---") {
            (added, removed)
        } else if line.starts_with('+') {
            (added.saturating_add(1), removed)
        } else if line.starts_with('-') {
            (added, removed.saturating_add(1))
        } else {
            (added, removed)
        }
    })
}

fn codex_applied_file_change(payload: &serde_json::Value) -> FileChangeStats {
    payload
        .get("changes")
        .and_then(|changes| changes.as_object())
        .map(|changes| {
            changes
                .values()
                .fold(FileChangeStats::default(), |mut total, change| {
                    let operation = change.get("type").and_then(|kind| kind.as_str());
                    let (mut lines_added, mut lines_removed) = change
                        .get("unified_diff")
                        .and_then(|diff| diff.as_str())
                        .map(unified_diff_line_counts)
                        .unwrap_or((0, 0));
                    let content_lines = change
                        .get("content")
                        .and_then(|content| content.as_str())
                        .map(text_line_count)
                        .unwrap_or(0);
                    match operation {
                        Some("add" | "create") => {
                            total.files_created = total.files_created.saturating_add(1);
                            if lines_added == 0 {
                                lines_added = content_lines;
                            }
                        }
                        Some("delete" | "remove") => {
                            total.files_deleted = total.files_deleted.saturating_add(1);
                            if lines_removed == 0 {
                                lines_removed = content_lines;
                            }
                        }
                        _ => {}
                    }
                    total.add(FileChangeStats {
                        lines_added,
                        lines_removed,
                        ..FileChangeStats::default()
                    });
                    total
                })
        })
        .unwrap_or_default()
}

fn claude_cache_write_tokens(usage: &serde_json::Value) -> (u64, u64, u64) {
    let reported_total = usage
        .get("cache_creation_input_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let cache_creation = usage.get("cache_creation");
    let one_hour = cache_creation
        .and_then(|value| value.get("ephemeral_1h_input_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let five_minute = cache_creation
        .and_then(|value| value.get("ephemeral_5m_input_tokens"))
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let classified = one_hour.saturating_add(five_minute);
    let total = reported_total.max(classified);

    // Older Claude records only expose the total; 5 minutes was the default TTL.
    let unclassified = total.saturating_sub(classified);
    (total, five_minute.saturating_add(unclassified), one_hour)
}

fn event_pricing_date(timestamp: Option<&str>) -> NaiveDate {
    timestamp
        .and_then(|value| value.get(..10))
        .and_then(|value| NaiveDate::parse_from_str(value, "%Y-%m-%d").ok())
        .unwrap_or_else(|| Utc::now().date_naive())
}

fn elapsed_ms(start: &str, end: Option<&str>) -> Option<u64> {
    let start = chrono::DateTime::parse_from_rfc3339(start).ok()?;
    let end = chrono::DateTime::parse_from_rfc3339(end?).ok()?;
    u64::try_from((end - start).num_milliseconds()).ok()
}

fn usage_delta(current: u64, previous: u64) -> u64 {
    if current >= previous {
        current - previous
    } else {
        current
    }
}

fn record_codex_subagent_response(subagent: &mut CodexSubagentFileState, response: &str) -> bool {
    let response = response.trim();
    if response.is_empty()
        || subagent.last_response.as_deref() == Some(response)
        || subagent.response_preview.ends_with(response)
    {
        return false;
    }
    if !subagent.response_preview.is_empty() {
        subagent.response_preview.push('\n');
    }
    subagent.response_preview.push_str(response);
    subagent.last_response = Some(response.to_string());
    true
}

fn apply_codex_subagent_usage(
    subagent: &mut CodexSubagentFileState,
    payload: &serde_json::Value,
    timestamp: &str,
) {
    let Some(info) = payload.get("info") else {
        return;
    };
    let (usage, is_cumulative) = if let Some(usage) = info.get("last_token_usage") {
        (usage, false)
    } else if let Some(usage) = info.get("total_token_usage") {
        (usage, true)
    } else {
        return;
    };

    let mut input = usage
        .get("input_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let mut output = usage
        .get("output_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let mut cached = usage
        .get("cached_input_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let reasoning = usage
        .get("reasoning_output_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let mut total = usage
        .get("total_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);

    if is_cumulative {
        let current = CodexUsageSnapshot {
            input,
            output,
            cached,
            reasoning,
            total,
        };
        input = usage_delta(current.input, subagent.total_usage.input);
        output = usage_delta(current.output, subagent.total_usage.output);
        cached = usage_delta(current.cached, subagent.total_usage.cached);
        total = usage_delta(current.total, subagent.total_usage.total);
        subagent.total_usage = current;
    }

    if input == 0 && output == 0 && cached == 0 && total > 0 {
        output = total;
    }
    subagent.input_tokens += input;
    subagent.output_tokens += output;
    subagent.cache_read_tokens += cached;

    if let Some(model) = subagent.model.as_deref() {
        if let Some(cost) = compute_provider_cost_at(
            ProviderKind::Codex,
            model,
            input,
            output,
            0,
            cached,
            event_pricing_date(Some(timestamp)),
        ) {
            subagent.cost += cost;
            subagent.cost_known = true;
        }
    }
    if input > 0 {
        let request_number = subagent.attribution.request_count() + 1;
        subagent.attribution.record_request_for_actor(
            format!("agent-{}-{request_number}", subagent.metadata.session_id),
            format!("Agent: {}", subagent.metadata.nickname),
            input,
            cached,
            true,
        );
    }
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
        || trimmed.starts_with("# AGENTS.md instructions for ")
        || trimmed.starts_with("Tool: ")
        || trimmed.starts_with("Working Directory:")
}

fn parse_codex_titles(data: &str) -> HashMap<String, String> {
    data.lines()
        .filter_map(|line| serde_json::from_str::<CodexSessionIndexEntry>(line).ok())
        .filter_map(|entry| {
            let title = entry.thread_name.trim();
            (!entry.id.is_empty() && !title.is_empty()).then(|| (entry.id, title.to_string()))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn attributed_tokens(
        root: &crate::model::AttributionNode,
        category: AttributionCategory,
    ) -> u64 {
        root.children
            .iter()
            .find(|node| node.category == Some(category))
            .map(|node| node.tokens)
            .unwrap_or(0)
    }

    #[test]
    fn codex_session_metadata_distinguishes_subagents_from_user_tasks() {
        let user: serde_json::Value = serde_json::from_str(
            r#"{"type":"session_meta","payload":{"thread_source":"user","source":"vscode"}}"#,
        )
        .unwrap();
        let subagent: serde_json::Value = serde_json::from_str(
            r#"{"type":"session_meta","payload":{"thread_source":"subagent","source":{"subagent":{"thread_spawn":{"parent_thread_id":"parent"}}}}}"#,
        )
        .unwrap();

        assert!(!codex_session_meta_is_subagent(&user));
        assert!(codex_session_meta_is_subagent(&subagent));
    }

    #[test]
    fn codex_identity_and_title_are_available_when_session_is_discovered() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("aether-project-{unique}.jsonl"));
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-parent\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Prompt fallback\"}}\n"
            ),
        )
        .unwrap();

        let mut engine = LiveEngine::new(Some(ProviderKind::Codex), None);
        engine
            .codex_titles
            .insert("codex-parent".to_string(), "Resolved title".to_string());
        let session = engine.new_session(&path, ProviderKind::Codex);

        assert_eq!(session.session_id, "codex-parent");
        assert_eq!(session.project_path, Some(PathBuf::from("/tmp/project")));
        assert_eq!(session.name, "Resolved title");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn codex_prompt_title_is_hydrated_before_the_session_is_rendered() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("aether-codex-title-{unique}.jsonl"));
        fs::write(
            &path,
            concat!(
                "{\"type\":\"session_meta\",\"payload\":{\"id\":\"codex-title\",\"cwd\":\"/tmp/aether\"}}\n",
                "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"# AGENTS.md instructions for /tmp/aether\"}]}}\n",
                "{\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"recognizable session title\"}}\n"
            ),
        )
        .unwrap();

        let engine = LiveEngine::new(Some(ProviderKind::Codex), None);
        let session = engine.new_session(&path, ProviderKind::Codex);

        assert_eq!(session.name, "recognizable session title");
        assert!(session.native_name_resolved);
        assert_eq!(session.usage.turn_count(), 0);

        let _ = fs::remove_file(path);
    }

    #[test]
    fn claude_title_is_available_when_session_is_discovered() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("aether-claude-{unique}.jsonl"));
        fs::write(
            &path,
            concat!(
                "{\"type\":\"user\",\"sessionId\":\"claude-parent\",\"cwd\":\"/tmp/project\",\"userType\":\"external\",\"message\":{\"content\":\"Long fallback prompt\"}}\n",
                "{\"type\":\"ai-title\",\"aiTitle\":\"Generated Claude title\"}\n"
            ),
        )
        .unwrap();

        let engine = LiveEngine::new(Some(ProviderKind::Claude), None);
        let session = engine.new_session(&path, ProviderKind::Claude);

        assert_eq!(session.session_id, "claude-parent");
        assert_eq!(session.project_path, Some(PathBuf::from("/tmp/project")));
        assert_eq!(session.name, "Generated Claude title");

        let _ = fs::remove_file(path);
    }

    #[test]
    fn codex_parent_rollout_renders_subagent_activity_in_parent() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-parent.jsonl"), ProviderKind::Codex);
        session.process_line(
            r#"{"type":"session_meta","payload":{"id":"parent","cwd":"/tmp/project","thread_source":"user"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":"delegate this work"}}"#,
        );
        session.process_line(
            r#"{"type":"response_item","payload":{"type":"function_call","name":"spawn_agent","arguments":"{\"task_name\":\"researcher\"}","call_id":"call-1"}}"#,
        );
        session.process_line(
            r#"{"type":"event_msg","payload":{"type":"sub_agent_activity","event_id":"call-1","agent_path":"/root/researcher","kind":"started"}}"#,
        );
        session.process_line(
            r#"{"type":"response_item","payload":{"type":"agent_message","author":"/root/researcher","recipient":"/root","content":[{"type":"input_text","text":"research complete"}]}}"#,
        );

        assert_eq!(session.project_name(), "project");
        assert!(session
            .agents
            .iter()
            .any(|agent| agent.id == "call-1" && agent.name == "researcher"));
        assert!(session.messages.iter().any(|message| {
            message.from == "call-1"
                && message.to == "codex"
                && message.content == "research complete"
        }));
        assert_eq!(session.usage.turns[0].agents.len(), 1);
        assert_eq!(
            session.usage.turns[0].agents[0].response_preview,
            "research complete"
        );
    }

    #[test]
    fn codex_child_rollout_is_a_live_nested_run_not_a_standalone_session() {
        use std::io::Write as _;

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aether-codex-subagent-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        let parent_path = dir.join("rollout-parent.jsonl");
        let child_path = dir.join("rollout-child.jsonl");
        fs::write(
            &parent_path,
            concat!(
                "{\"timestamp\":\"2026-07-14T17:28:10.000Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"parent\",\"cwd\":\"/tmp/project\",\"thread_source\":\"user\"}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:10.500Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"parent-turn\"}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:11.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"delegate this work\"}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:12.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\",\"turn_id\":\"parent-turn\",\"status\":\"completed\",\"changes\":{\"/tmp/parent.rs\":{\"type\":\"update\",\"unified_diff\":\"@@ -1 +1 @@\\n-old\\n+new\"}}}}\n"
            ),
        )
        .unwrap();
        fs::write(
            &child_path,
            concat!(
                "{\"timestamp\":\"2026-07-14T17:28:25.073Z\",\"type\":\"session_meta\",\"payload\":{\"id\":\"child\",\"parent_thread_id\":\"parent\",\"agent_nickname\":\"Herschel\",\"agent_role\":\"default\",\"thread_source\":\"subagent\",\"source\":{\"subagent\":{\"thread_spawn\":{\"parent_thread_id\":\"parent\",\"agent_nickname\":\"Herschel\",\"agent_role\":\"default\"}}}}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:25.074Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"parent-turn\"}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:25.075Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"delegate this work\"}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:25.076Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"patch_apply_end\",\"turn_id\":\"parent-turn\",\"status\":\"completed\",\"changes\":{\"/tmp/parent.rs\":{\"type\":\"update\",\"unified_diff\":\"@@ -1 +1 @@\\n-old\\n+new\"}}}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:26.000Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\",\"turn_id\":\"child-turn\"}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:28.269Z\",\"type\":\"turn_context\",\"payload\":{\"turn_id\":\"child-turn\",\"model\":\"gpt-5.6-sol\",\"cwd\":\"/tmp/project\"}}\n",
                "{\"timestamp\":\"2026-07-14T17:28:28.273Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"user_message\",\"message\":\"Print exactly this text and nothing else: hello worls\"}}\n"
            ),
        )
        .unwrap();

        let mut engine = LiveEngine::new(Some(ProviderKind::Codex), Some(dir.clone()));
        engine.scan_sessions();
        assert_eq!(engine.sessions.len(), 1);
        assert_eq!(engine.sessions[0].session_id, "parent");

        engine.sessions[0].poll_file();
        assert_eq!(engine.sessions[0].usage.turns[0].agents.len(), 1);
        assert_eq!(
            engine.sessions[0].usage.turns[0].agents[0].outcome,
            TurnOutcome::InProgress
        );
        assert!(engine.sessions[0].messages.iter().any(|message| {
            message.from == "codex"
                && message.to == "child"
                && message.message_type == MessageType::Delegation
        }));

        let mut child = fs::OpenOptions::new()
            .append(true)
            .open(&child_path)
            .unwrap();
        child
            .write_all(
                concat!(
                    "{\"timestamp\":\"2026-07-14T17:28:29.940Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"agent_message\",\"phase\":\"final_answer\",\"message\":\"hello worls\"}}\n",
                    "{\"timestamp\":\"2026-07-14T17:28:29.940Z\",\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"phase\":\"final_answer\",\"content\":[{\"type\":\"output_text\",\"text\":\"hello worls\"}]}}\n",
                    "{\"timestamp\":\"2026-07-14T17:28:30.080Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"token_count\",\"info\":{\"last_token_usage\":{\"input_tokens\":15707,\"cached_input_tokens\":9984,\"output_tokens\":7,\"reasoning_output_tokens\":0,\"total_tokens\":15714}}}}\n",
                    "{\"timestamp\":\"2026-07-14T17:28:30.094Z\",\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\",\"message\":\"hello worls\"}}\n"
                )
                .as_bytes(),
            )
            .unwrap();
        drop(child);

        engine.sessions[0].poll_file();
        let nested = &engine.sessions[0].usage.turns[0].agents[0];
        assert_eq!(nested.id, "child");
        assert_eq!(nested.name, "Herschel");
        assert_eq!(nested.role, "default");
        assert_eq!(nested.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(nested.outcome, TurnOutcome::Completed);
        assert_eq!(nested.response_preview, "hello worls");
        assert_eq!(nested.input_tokens, 15_707);
        assert_eq!(nested.cache_read_tokens, 9_984);
        assert_eq!(nested.output_tokens, 7);
        assert_eq!(nested.lines_added, 0);
        assert_eq!(nested.lines_removed, 0);
        assert_eq!(engine.sessions[0].usage.turns[0].lines_added(), 1);
        assert_eq!(engine.sessions[0].usage.turns[0].lines_removed(), 1);
        assert!(nested.cost_known);
        assert!(nested.duration_ms.is_some());
        assert_eq!(engine.sessions[0].usage.total_input(), 15_707);
        assert_eq!(engine.sessions[0].usage.total_output(), 7);
        assert!(engine.sessions[0].usage.total_cost() > 0.0);
        assert_eq!(
            engine.sessions[0]
                .messages
                .iter()
                .filter(|message| message.from == "child" && message.content == "hello worls")
                .count(),
            1
        );
        let agent_attribution = engine.sessions[0].usage.turns[0]
            .attribution
            .aggregate
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::Agents))
            .expect("agents attribution");
        assert!(agent_attribution
            .children
            .iter()
            .any(|source| source.label == "Herschel"));

        let metadata = codex_subagent_metadata(&child_path).unwrap();
        assert_eq!(metadata.parent_session_id, "parent");
        assert_eq!(metadata.nickname, "Herschel");

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn active_sessions_keep_projects_together_by_project_recency() {
        let mut engine = LiveEngine::new(Some(ProviderKind::Codex), None);
        let mut project_a = SessionState::new(PathBuf::from("a.jsonl"), ProviderKind::Codex);
        project_a.set_project_path("/tmp/project-a");
        project_a.last_modified = 20;
        let mut project_b_old =
            SessionState::new(PathBuf::from("b-old.jsonl"), ProviderKind::Codex);
        project_b_old.set_project_path("/tmp/project-b");
        project_b_old.last_modified = 10;
        let mut project_b_new =
            SessionState::new(PathBuf::from("b-new.jsonl"), ProviderKind::Codex);
        project_b_new.set_project_path("/tmp/project-b");
        project_b_new.last_modified = 30;
        engine.sessions = vec![project_a, project_b_old, project_b_new];

        let ordered: Vec<String> = engine
            .active_sessions()
            .map(|(_, session)| session.file_path.display().to_string())
            .collect();
        assert_eq!(ordered, vec!["b-new.jsonl", "b-old.jsonl", "a.jsonl"]);

        engine.active_idx = 2;
        assert_eq!(engine.active_session_position(), Some(1));
        engine.active_idx = 0;
        assert_eq!(engine.active_session_position(), Some(3));
    }

    #[test]
    fn codex_index_titles_replace_fallbacks_but_not_manual_renames() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"session_meta","payload":{"id":"codex-session","cwd":"/tmp/project"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"the full first prompt"}}"#,
        );

        session.apply_native_title("Generated task title");
        assert_eq!(session.name, "Generated task title");

        session.name = "My Aether name".to_string();
        session.name_override = Some(session.name.clone());
        session.apply_native_title("Renamed in Codex");
        assert_eq!(session.name, "My Aether name");
    }

    #[test]
    fn codex_title_index_uses_latest_valid_nonempty_title() {
        let titles = parse_codex_titles(
            r#"{"id":"session-1","thread_name":"First title"}
not json
{"id":"session-2","thread_name":"  "}
{"id":"session-1","thread_name":" Updated title "}"#,
        );

        assert_eq!(titles.len(), 1);
        assert_eq!(
            titles.get("session-1").map(String::as_str),
            Some("Updated title")
        );
    }

    #[test]
    fn claude_generated_and_custom_titles_replace_prompt_fallback() {
        let mut session =
            SessionState::new(PathBuf::from("claude-session.jsonl"), ProviderKind::Claude);
        session.process_line(
            r#"{"type":"user","userType":"external","message":{"content":"the full first prompt"}}"#,
        );
        assert_eq!(session.name, "the full first prompt");

        session.process_line(r#"{"type":"ai-title","aiTitle":"Generated Claude title"}"#);
        assert_eq!(session.name, "Generated Claude title");

        session.process_line(r#"{"type":"custom-title","customTitle":"My Claude title"}"#);
        assert_eq!(session.name, "My Claude title");
    }

    #[test]
    fn claude_synthetic_assistant_response_marks_turn_failed() {
        let mut session =
            SessionState::new(PathBuf::from("claude-session.jsonl"), ProviderKind::Claude);
        session.process_line(
            r#"{"type":"user","userType":"external","timestamp":"2026-07-14T00:00:00.000Z","message":{"content":"hi"}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:00.389Z","message":{"model":"<synthetic>","content":[{"type":"text","text":"Credit balance is too low"}],"stop_reason":"stop_sequence","usage":{"input_tokens":0,"output_tokens":0}}}"#,
        );

        assert_eq!(session.usage.turns.len(), 1);
        let turn = &session.usage.turns[0];
        assert_eq!(turn.telemetry.model.as_deref(), Some("<synthetic>"));
        assert_eq!(turn.telemetry.outcome, TurnOutcome::Failed);
        assert_eq!(turn.telemetry.duration_ms, Some(389));
        assert_eq!(turn.response_text, "Credit balance is too low");
    }

    #[test]
    fn claude_usage_preserves_one_hour_cache_write_pricing() {
        let mut session =
            SessionState::new(PathBuf::from("claude-session.jsonl"), ProviderKind::Claude);
        session.process_line(
            r#"{"type":"user","userType":"external","timestamp":"2026-07-14T00:00:00.000Z","message":{"content":"test cache pricing"}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01.000Z","message":{"model":"claude-fable-5","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":1000,"output_tokens":100,"cache_creation_input_tokens":200,"cache_read_input_tokens":300,"cache_creation":{"ephemeral_1h_input_tokens":200,"ephemeral_5m_input_tokens":0}}}}"#,
        );

        let turn = &session.usage.turns[0];
        let expected = (1_000.0 * 10.0 + 100.0 * 50.0 + 200.0 * 20.0 + 300.0 * 1.0) / 1_000_000.0;
        assert!((turn.cost - expected).abs() < f64::EPSILON);
        assert_eq!(turn.cache_write_tokens, 200);
        assert!(turn.cost_known);
    }

    #[test]
    fn claude_native_metrics_dedupe_message_usage_and_fill_context_complexity_and_duration() {
        let mut session =
            SessionState::new(PathBuf::from("claude-session.jsonl"), ProviderKind::Claude);
        session.process_line(
            r#"{"type":"user","userType":"external","timestamp":"2026-07-14T00:00:00.000Z","message":{"content":"inspect telemetry"}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01.000Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"consider"}],"stop_reason":null,"usage":{"input_tokens":2,"output_tokens":800,"cache_creation_input_tokens":100,"cache_read_input_tokens":50000}}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01.100Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[{"type":"text","text":"working"}],"stop_reason":null,"usage":{"input_tokens":2,"output_tokens":800,"cache_creation_input_tokens":100,"cache_read_input_tokens":50000}}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:02.000Z","message":{"id":"msg-2","model":"claude-opus-4-8","content":[{"type":"thinking","thinking":"finish"},{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":450,"cache_creation_input_tokens":50,"cache_read_input_tokens":51000,"output_tokens_details":{"thinking_tokens":400}}}}"#,
        );
        session.process_line(
            r#"{"type":"system","subtype":"turn_duration","durationMs":2345,"timestamp":"2026-07-14T00:00:02.345Z"}"#,
        );

        let turn = &session.usage.turns[0];
        assert_eq!(turn.input_tokens, 5);
        assert_eq!(turn.output_tokens, 1_250);
        assert_eq!(turn.cache_read_tokens, 101_000);
        assert_eq!(turn.cache_write_tokens, 150);
        assert_eq!(turn.telemetry.latest_input_tokens, 51_053);
        assert_eq!(turn.telemetry.context_window, Some(1_000_000));
        assert_eq!(turn.telemetry.context_percent(), Some(5.1053));
        assert_eq!(turn.telemetry.reasoning_tokens, 400);
        assert_eq!(turn.telemetry.complexity_proxy_tokens, 800);
        assert_eq!(turn.telemetry.complexity_percent(), Some(7.5));
        assert_eq!(
            turn.telemetry.complexity_basis(),
            Some("reasoning + thinking-output proxy")
        );
        assert_eq!(turn.telemetry.duration_ms, Some(2_345));
        assert_eq!(turn.response_text, "working\ndone");
    }

    #[test]
    fn claude_metadata_records_do_not_refresh_session_activity() {
        let mut session =
            SessionState::new(PathBuf::from("claude-session.jsonl"), ProviderKind::Claude);
        session.process_line(
            r#"{"type":"user","userType":"external","timestamp":"2026-07-15T10:05:08.185Z","message":{"content":"resume"}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","timestamp":"2026-07-15T10:05:15.841Z","message":{"model":"claude-opus-4-8","content":[{"type":"text","text":"done"}],"stop_reason":"end_turn","usage":{"input_tokens":3,"output_tokens":2}}}"#,
        );
        session.process_line(
            r#"{"type":"system","subtype":"turn_duration","durationMs":7995,"timestamp":"2026-07-15T10:05:16.260Z"}"#,
        );
        let completed_activity = session.last_activity;

        for metadata in [
            r#"{"type":"bridge-session","timestamp":"2026-07-15T13:00:00Z"}"#,
            r#"{"type":"last-prompt","timestamp":"2026-07-15T13:00:01Z"}"#,
            r#"{"type":"mode","timestamp":"2026-07-15T13:00:02Z"}"#,
            r#"{"type":"permission-mode","timestamp":"2026-07-15T13:00:03Z"}"#,
            r#"{"type":"file-history-snapshot","timestamp":"2026-07-15T13:00:04Z"}"#,
            r#"{"type":"attachment","timestamp":"2026-07-15T13:00:05Z"}"#,
        ] {
            session.process_line(metadata);
        }

        assert!(completed_activity > 0);
        assert_eq!(session.last_activity, completed_activity);
        assert_eq!(
            session.usage.turns[0].telemetry.outcome,
            TurnOutcome::Completed
        );
        assert_eq!(session.usage.turns[0].telemetry.duration_ms, Some(7_995));
    }

    #[test]
    fn claude_diff_lines_commit_only_after_successful_tool_results() {
        let mut session =
            SessionState::new(PathBuf::from("claude-session.jsonl"), ProviderKind::Claude);
        session.process_line(
            r#"{"type":"user","userType":"external","message":{"content":"edit files"}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"tool_use","id":"edit-ok","name":"Edit","input":{"old_string":"one\ntwo","new_string":"one\nthree\nfour"}}],"stop_reason":"tool_use"}}"#,
        );
        session.process_line(
            r#"{"type":"user","userType":"internal","message":{"content":[{"type":"tool_result","tool_use_id":"edit-ok","content":"updated"}]}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"tool_use","id":"edit-fail","name":"Write","input":{"content":"ignored\nlines"}}],"stop_reason":"tool_use"}}"#,
        );
        session.process_line(
            r#"{"type":"user","userType":"internal","message":{"content":[{"type":"tool_result","tool_use_id":"edit-fail","is_error":true,"content":"failed"}]}}"#,
        );
        session.process_line(
            r#"{"type":"assistant","message":{"model":"claude-opus-4-8","content":[{"type":"tool_use","id":"write-created","name":"Write","input":{"content":"new\nfile"}}],"stop_reason":"tool_use"}}"#,
        );
        session.process_line(
            r#"{"type":"user","userType":"internal","message":{"content":[{"type":"tool_result","tool_use_id":"write-created","content":"created"}]},"toolUseResult":{"type":"create"}}"#,
        );

        let turn = &session.usage.turns[0];
        assert_eq!(turn.lines_added(), 5);
        assert_eq!(turn.lines_removed(), 2);
        assert_eq!(turn.diff_lines(), 7);
        assert_eq!(turn.files_created(), 1);
        assert_eq!(turn.files_deleted(), 0);
    }

    #[test]
    fn unified_diff_line_counts_ignore_file_headers() {
        let diff =
            "--- a/src/main.rs\n+++ b/src/main.rs\n@@ -1,2 +1,3 @@\n-old\n+new\n+extra\n context";
        assert_eq!(unified_diff_line_counts(diff), (2, 1));
    }

    #[test]
    fn codex_file_changes_use_native_add_and_delete_operations() {
        let payload = serde_json::json!({
            "changes": {
                "/tmp/created.rs": {"type": "add", "content": "one\ntwo"},
                "/tmp/empty.rs": {"type": "add", "content": ""},
                "/tmp/deleted.rs": {"type": "delete", "content": "old\nfile"},
                "/tmp/updated.rs": {
                    "type": "update",
                    "unified_diff": "@@ -1 +1 @@\n-old\n+new"
                }
            }
        });

        assert_eq!(
            codex_applied_file_change(&payload),
            FileChangeStats {
                lines_added: 3,
                lines_removed: 3,
                files_created: 2,
                files_deleted: 1,
            }
        );
    }

    #[test]
    fn claude_subagent_is_attached_to_parent_with_native_telemetry() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aether-claude-subagent-{unique}"));
        let parent_path = dir.join("parent.jsonl");
        let subagents_dir = dir.join("parent").join("subagents");
        fs::create_dir_all(&subagents_dir).unwrap();
        fs::write(
            subagents_dir.join("agent-test.meta.json"),
            r#"{"agentType":"Explore","description":"Inspect native Claude telemetry"}"#,
        )
        .unwrap();
        fs::write(
            subagents_dir.join("agent-test.jsonl"),
            concat!(
                "{\"type\":\"user\",\"timestamp\":\"2026-07-14T00:00:01.000Z\",\"message\":{\"content\":\"inspect nested work\"}}\n",
                "{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:02.000Z\",\"message\":{\"model\":\"claude-fable-5\",\"content\":[{\"type\":\"tool_use\",\"name\":\"Read\"}],\"stop_reason\":\"tool_use\",\"usage\":{\"input_tokens\":100,\"output_tokens\":20,\"cache_creation_input_tokens\":10,\"cache_read_input_tokens\":30,\"cache_creation\":{\"ephemeral_1h_input_tokens\":10,\"ephemeral_5m_input_tokens\":0}}}}\n",
                "{\"type\":\"assistant\",\"timestamp\":\"2026-07-14T00:00:05.000Z\",\"message\":{\"model\":\"claude-fable-5\",\"content\":[{\"type\":\"text\",\"text\":\"nested result\"}],\"stop_reason\":\"end_turn\",\"usage\":{\"input_tokens\":10,\"output_tokens\":5,\"cache_creation_input_tokens\":0,\"cache_read_input_tokens\":40,\"cache_creation\":{\"ephemeral_1h_input_tokens\":0,\"ephemeral_5m_input_tokens\":0}}}}\n"
            ),
        )
        .unwrap();

        let mut session = SessionState::new(parent_path, ProviderKind::Claude);
        session.process_line(
            r#"{"type":"user","userType":"external","timestamp":"2026-07-14T00:00:00.000Z","message":{"content":"delegate this"}}"#,
        );
        session.scan_subagents();

        assert_eq!(session.usage.turns[0].agents.len(), 1);
        let agent = &session.usage.turns[0].agents[0];
        assert_eq!(agent.model.as_deref(), Some("claude-fable-5"));
        assert_eq!(agent.outcome, TurnOutcome::Completed);
        assert_eq!(agent.duration_ms, Some(4_000));
        assert_eq!(agent.tool_calls, 1);
        assert_eq!(agent.input_tokens, 110);
        assert_eq!(agent.output_tokens, 25);
        assert_eq!(agent.cache_read_tokens, 70);
        assert_eq!(agent.response_preview, "nested result");
        assert!(agent.cost_known);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn session_navigation_includes_usage_only_claude_sessions() {
        let mut engine = LiveEngine::new(Some(ProviderKind::Claude), None);
        for (path, prompt, modified) in
            [("first.jsonl", "first", 20), ("second.jsonl", "second", 10)]
        {
            let mut session = SessionState::new(PathBuf::from(path), ProviderKind::Claude);
            session.process_line(&format!(
                r#"{{"type":"user","userType":"external","timestamp":"2026-07-14T00:00:00Z","message":{{"content":"{prompt}"}}}}"#
            ));
            session.last_modified = modified;
            engine.sessions.push(session);
        }

        engine.active_idx = 0;
        engine.next_session();
        assert_eq!(engine.active_idx, 1);
        engine.prev_session();
        assert_eq!(engine.active_idx, 0);
    }

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
        assert_eq!(session.name, "write a test");
        assert_eq!(session.usage.turn_count(), 1);
        assert_eq!(session.usage.turns[0].prompt, "write a test");
        assert_eq!(session.usage.turns[0].response_text, "done");
        assert_eq!(session.usage.turns[0].input_tokens, 10);
        assert_eq!(session.usage.turns[0].cache_read_tokens, 2);
        assert_eq!(session.usage.turns[0].output_tokens, 5);
        assert!(!session.usage.turns[0].cost_known);
    }

    #[test]
    fn codex_project_name_is_only_a_fallback_until_the_first_prompt() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);

        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"session_meta","payload":{"id":"codex-session","cwd":"/tmp/project"}}"#,
        );

        assert_eq!(session.name, "project");
        assert!(!session.native_name_resolved);

        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"recognizable session name"}}"#,
        );

        assert_eq!(session.name, "recognizable session name");
        assert!(session.native_name_resolved);

        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:02Z","type":"session_meta","payload":{"id":"codex-session","cwd":"/tmp/project"}}"#,
        );

        assert_eq!(session.name, "recognizable session name");
    }

    #[test]
    fn codex_synthetic_user_records_do_not_replace_the_project_fallback() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);

        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:00Z","type":"session_meta","payload":{"id":"codex-session","cwd":"/tmp/project"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-05-31T00:00:01Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<environment_context>\n  <cwd>/tmp/project</cwd>\n</environment_context>"}]}}"#,
        );

        assert_eq!(session.name, "project");
        assert!(!session.native_name_resolved);
        assert_eq!(session.usage.turn_count(), 0);
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
    fn codex_native_telemetry_tracks_turn_outcome_and_actions() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);

        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"turn_context","payload":{"cwd":"/tmp/project","model":"gpt-5.6-sol"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"inspect telemetry"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:02Z","type":"response_item","payload":{"type":"function_call","name":"exec_command"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:03Z","type":"response_item","payload":{"type":"web_search_call","status":"completed"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:04Z","type":"event_msg","payload":{"type":"patch_apply_end","status":"completed","changes":{"/tmp/main.rs":{"unified_diff":"--- a/main.rs\n+++ b/main.rs\n@@ -1 +1,2 @@\n-old\n+new\n+extra"}}}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:05Z","type":"event_msg","payload":{"type":"context_compacted"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:10Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":200,"output_tokens":100,"reasoning_output_tokens":20,"total_tokens":1100},"model_context_window":200000}}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:13Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":12000}}"#,
        );

        let turn = &session.usage.turns[0];
        assert_eq!(turn.telemetry.model.as_deref(), Some("gpt-5.6-sol"));
        assert_eq!(turn.telemetry.latest_input_tokens, 1000);
        assert_eq!(turn.telemetry.reasoning_tokens, 20);
        assert!(turn.telemetry.reasoning_tokens_emitted);
        assert_eq!(turn.telemetry.context_window, Some(200_000));
        assert_eq!(turn.telemetry.duration_ms, Some(12_000));
        assert_eq!(turn.telemetry.outcome, TurnOutcome::Completed);
        assert_eq!(turn.telemetry.tool_calls, 2);
        assert_eq!(turn.telemetry.patches, 1);
        assert_eq!(turn.telemetry.web_searches, 1);
        assert_eq!(turn.telemetry.compactions, 1);
        assert_eq!(turn.lines_added(), 2);
        assert_eq!(turn.lines_removed(), 1);
        assert_eq!(turn.cumulative_context, 1000);
        assert!(turn.cost_known);
    }

    #[test]
    fn codex_compaction_anchors_use_native_samples_and_dedupe_paired_events() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);

        for line in [
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"turn_context","payload":{"cwd":"/tmp/project","model":"gpt-5.6-sol"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"inspect compaction"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":229899,"cached_input_tokens":200000,"output_tokens":100,"total_tokens":229999},"model_context_window":258400}}}"#,
            r#"{"timestamp":"2026-07-14T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":0,"cached_input_tokens":0,"output_tokens":0,"total_tokens":0},"model_context_window":258400}}}"#,
            r#"{"timestamp":"2026-07-14T00:00:04Z","type":"compacted","payload":{}}"#,
            r#"{"timestamp":"2026-07-14T00:00:04.020Z","type":"event_msg","payload":{"type":"context_compacted"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:05Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":25330,"cached_input_tokens":10000,"output_tokens":100,"total_tokens":25430},"model_context_window":258400}}}"#,
            r#"{"timestamp":"2026-07-14T00:00:06Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":97978,"cached_input_tokens":90000,"output_tokens":100,"total_tokens":98078},"model_context_window":258400}}}"#,
        ] {
            session.process_line(line);
        }

        let telemetry = &session.usage.turns[0].telemetry;
        assert_eq!(telemetry.compactions, 1);
        assert_eq!(telemetry.context_samples.len(), 4);
        let ranges = telemetry.context_compaction_ranges();
        assert_eq!(ranges.len(), 1);
        assert!((ranges[0].0 - 88.970_201).abs() < 0.001);
        assert!((ranges[0].1 - 9.802_632).abs() < 0.001);
        assert!((telemetry.context_percent().unwrap() - 37.917_183).abs() < 0.001);
    }

    #[test]
    fn codex_steering_messages_partition_the_provider_task_duration() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);

        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"task-1"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"initial request"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:05Z","type":"event_msg","payload":{"type":"agent_message","phase":"commentary","message":"working"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:10Z","type":"event_msg","payload":{"type":"user_message","message":"steer the active task"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:30Z","type":"event_msg","payload":{"type":"task_complete","duration_ms":30000,"turn_id":"task-1"}}"#,
        );

        assert_eq!(session.usage.turn_count(), 2);
        assert_eq!(session.usage.turns[0].telemetry.duration_ms, Some(9_000));
        assert_eq!(session.usage.turns[1].telemetry.duration_ms, Some(20_000));
        assert_eq!(
            session.usage.turns[0].telemetry.outcome,
            TurnOutcome::Completed
        );
        assert_eq!(
            session.usage.turns[1].telemetry.outcome,
            TurnOutcome::Completed
        );
    }

    #[test]
    fn codex_cumulative_usage_fallback_adds_only_deltas() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"turn_context","payload":{"model":"gpt-5.6-sol"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"count deltas"}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"reasoning_output_tokens":2,"total_tokens":110}}}}"#,
        );
        session.process_line(
            r#"{"timestamp":"2026-07-14T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":150,"cached_input_tokens":30,"output_tokens":30,"reasoning_output_tokens":7,"total_tokens":180}}}}"#,
        );

        let turn = &session.usage.turns[0];
        assert_eq!(turn.input_tokens, 150);
        assert_eq!(turn.cache_read_tokens, 30);
        assert_eq!(turn.output_tokens, 30);
        assert_eq!(turn.telemetry.reasoning_tokens, 7);
        assert!(turn.telemetry.reasoning_tokens_emitted);
        assert_eq!(turn.telemetry.latest_input_tokens, 50);
        assert_eq!(turn.cumulative_context, 150);
    }

    #[test]
    fn codex_attribution_aggregates_tool_loop_requests_and_uses_safe_purposes() {
        let mut session =
            SessionState::new(PathBuf::from("rollout-test.jsonl"), ProviderKind::Codex);
        for line in [
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":"inspect the project"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test --all\"}","call_id":"call-1"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"total_tokens":110}}}}"#,
            r#"{"timestamp":"2026-07-14T00:00:03Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"tests passed"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":150,"cached_input_tokens":30,"output_tokens":20,"total_tokens":170}}}}"#,
        ] {
            session.process_line(line);
        }

        let attribution = &session.usage.turns[0].attribution;
        assert_eq!(attribution.request_count(), 2);
        assert_eq!(attribution.aggregate.tokens, 250);
        assert_eq!(attribution.request(0).unwrap().label, "Initial request");
        assert_eq!(attribution.request(1).unwrap().label, "After cargo test");
        assert_eq!(
            attribution
                .aggregate
                .children
                .iter()
                .map(|node| node.tokens)
                .sum::<u64>(),
            250
        );
        assert!(
            attributed_tokens(
                &attribution.request(1).unwrap().root,
                AttributionCategory::ToolsAndMcps
            ) > 0
        );
    }

    #[test]
    fn codex_startup_hook_and_agents_memory_attach_to_first_user_turn() {
        let mut session = SessionState::new(
            PathBuf::from("codex-startup-context.jsonl"),
            ProviderKind::Codex,
        );
        for line in [
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:00.1Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"ordinary provider instructions"}]}}"#,
            r##"{"timestamp":"2026-07-14T00:00:00.2Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"<recommended_plugins>unrelated startup data</recommended_plugins>"},{"type":"input_text","text":"# AGENTS.md instructions for /tmp/project\n\n<INSTRUCTIONS>\nRemember ember-lantern.\n</INSTRUCTIONS>"}]}}"##,
            r#"{"timestamp":"2026-07-14T00:00:00.3Z","type":"response_item","payload":{"type":"message","role":"developer","content":[{"type":"input_text","text":"Aether test hook marker: copper-comet."}]}}"#,
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"event_msg","payload":{"type":"user_message","message":"show startup context"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":1,"total_tokens":1001}}}}"#,
        ] {
            session.process_line(line);
        }

        assert_eq!(session.usage.turn_count(), 1);
        assert_eq!(session.usage.turns[0].prompt, "show startup context");
        let root = &session.usage.turns[0].attribution.aggregate;
        assert!(attributed_tokens(root, AttributionCategory::Hooks) > 0);
        assert!(attributed_tokens(root, AttributionCategory::Memory) > 0);
        assert!(attributed_tokens(root, AttributionCategory::ProviderRuntime) > 0);

        let memory = root
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::Memory))
            .expect("memory category");
        assert!(memory.children.iter().any(|node| node.label == "AGENTS.md"));
    }

    #[test]
    fn codex_repeated_prompt_in_a_new_task_is_a_new_turn() {
        let mut session = SessionState::new(
            PathBuf::from("codex-repeated-prompt.jsonl"),
            ProviderKind::Codex,
        );
        for line in [
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-1"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:00.1Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"repeat this"}]}}"#,
            r#"{"timestamp":"2026-07-14T00:00:00.2Z","type":"event_msg","payload":{"type":"user_message","message":"repeat this"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"event_msg","payload":{"type":"task_complete","turn_id":"turn-1"}}"#,
            r#"{"timestamp":"2026-07-14T00:01:00Z","type":"event_msg","payload":{"type":"task_started","turn_id":"turn-2"}}"#,
            r#"{"timestamp":"2026-07-14T00:01:00.1Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"repeat this"}]}}"#,
            r#"{"timestamp":"2026-07-14T00:01:00.2Z","type":"event_msg","payload":{"type":"user_message","message":"repeat this"}}"#,
        ] {
            session.process_line(line);
        }

        assert_eq!(session.usage.turn_count(), 2);
        assert!(session
            .usage
            .turns
            .iter()
            .all(|turn| turn.prompt == "repeat this"));
    }

    #[test]
    fn claude_attribution_deduplicates_usage_and_tool_blocks_by_native_ids() {
        let mut session =
            SessionState::new(PathBuf::from("claude-test.jsonl"), ProviderKind::Claude);
        for line in [
            r#"{"type":"user","timestamp":"2026-07-14T00:00:00Z","userType":"external","message":{"content":"inspect the project"}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"file_path":"/private/project/README.md"}}],"usage":{"input_tokens":100,"output_tokens":10}}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01.1Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"tool-1","name":"Read","input":{"file_path":"/private/project/README.md"}}],"usage":{"input_tokens":100,"output_tokens":10}}}"#,
            r#"{"type":"user","timestamp":"2026-07-14T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":"document contents"}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:03Z","message":{"id":"msg-2","model":"claude-opus-4-8","content":[{"type":"text","text":"done"}],"usage":{"input_tokens":150,"output_tokens":20}}}"#,
        ] {
            session.process_line(line);
        }

        let turn = &session.usage.turns[0];
        assert_eq!(turn.telemetry.tool_calls, 1);
        assert_eq!(turn.attribution.request_count(), 2);
        assert_eq!(turn.attribution.aggregate.tokens, 250);
        assert_eq!(
            turn.attribution.request(1).unwrap().label,
            "After Read README.md"
        );
    }

    #[test]
    fn attribution_keeps_direct_documents_separate_from_tool_delivered_content() {
        let mut direct = SessionState::new(PathBuf::from("claude-doc.jsonl"), ProviderKind::Claude);
        direct.process_line(
            r#"{"type":"user","timestamp":"2026-07-14T00:00:00Z","userType":"external","message":{"content":[{"type":"text","text":"review this"},{"type":"document","title":"design.pdf","source":{"data":"encoded"}}]}}"#,
        );
        direct.process_line(
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[],"usage":{"input_tokens":100,"output_tokens":1}}}"#,
        );
        assert!(
            attributed_tokens(
                &direct.usage.turns[0].attribution.aggregate,
                AttributionCategory::DocumentsAndKbs
            ) > 0
        );

        let mut tool = SessionState::new(PathBuf::from("claude-tool.jsonl"), ProviderKind::Claude);
        for line in [
            r#"{"type":"user","timestamp":"2026-07-14T00:00:00Z","userType":"external","message":{"content":"fetch the document"}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[{"type":"tool_use","id":"tool-1","name":"mcp__drive__get_document","input":{}}],"usage":{"input_tokens":100,"output_tokens":1}}}"#,
            r#"{"type":"user","timestamp":"2026-07-14T00:00:02Z","message":{"content":[{"type":"tool_result","tool_use_id":"tool-1","content":{"type":"document","text":"contents"}}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:03Z","message":{"id":"msg-2","model":"claude-opus-4-8","content":[],"usage":{"input_tokens":150,"output_tokens":1}}}"#,
        ] {
            tool.process_line(line);
        }
        let second = &tool.usage.turns[0].attribution.request(1).unwrap().root;
        assert!(attributed_tokens(second, AttributionCategory::ToolsAndMcps) > 0);
        assert_eq!(
            attributed_tokens(second, AttributionCategory::DocumentsAndKbs),
            0
        );
    }

    #[test]
    fn tool_image_results_use_visual_tokens_instead_of_base64_length() {
        let encoded = "a".repeat(300_000);
        let payload = serde_json::json!({
            "type": "custom_tool_call_output",
            "call_id": "call-1",
            "output": [
                {"type": "input_text", "text": "image loaded"},
                {
                    "type": "input_image",
                    "image_url": format!("data:image/png;base64,{encoded}")
                }
            ]
        });

        let tokens = estimate_tool_result_tokens(&payload);

        assert!(tokens >= NATIVE_IMAGE_ESTIMATE_TOKENS);
        assert!(tokens < NATIVE_IMAGE_ESTIMATE_TOKENS + 20);
    }

    #[test]
    fn tool_text_results_count_only_model_visible_output() {
        let payload = serde_json::json!({
            "type": "function_call_output",
            "call_id": "call-1",
            "internal_chat_message_metadata_passthrough": "x".repeat(10_000),
            "output": "tests passed"
        });

        assert_eq!(
            estimate_tool_result_tokens(&payload),
            estimate_tokens("tests passed")
        );
    }

    #[test]
    fn codex_pasted_image_is_attached_once_to_the_canonical_turn() {
        let mut session =
            SessionState::new(PathBuf::from("codex-image.jsonl"), ProviderKind::Codex);
        for line in [
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"inspect image"},{"type":"input_text","text":"<image name=[Image #1]>"},{"type":"input_image","image_url":"data:image/png;base64,abc"},{"type":"input_text","text":"</image>"}]}}"#,
            r#"{"timestamp":"2026-07-14T00:00:00.1Z","type":"event_msg","payload":{"type":"user_message","message":"inspect image","images":[],"local_images":["/tmp/screenshot.png"]}}"#,
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":0,"output_tokens":1,"total_tokens":101}}}}"#,
        ] {
            session.process_line(line);
        }

        assert_eq!(session.usage.turn_count(), 1);
        let documents = session.usage.turns[0]
            .attribution
            .aggregate
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::DocumentsAndKbs))
            .expect("documents category");
        assert!(documents.tokens > 0);
        assert!(documents
            .children
            .iter()
            .any(|source| source.label == "screenshot.png"));
    }

    #[test]
    fn codex_exec_wrapper_uses_the_nested_tool_and_command_as_safe_labels() {
        let mut session = SessionState::new(PathBuf::from("codex-exec.jsonl"), ProviderKind::Codex);
        for line in [
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":"run tests"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"response_item","payload":{"type":"custom_tool_call","call_id":"call-1","name":"exec","input":"const r = await tools.exec_command({cmd:\"cargo test\"}); text(r.output);"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:02Z","type":"response_item","payload":{"type":"custom_tool_call_output","call_id":"call-1","output":"tests passed"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:03Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":1000,"cached_input_tokens":0,"output_tokens":1,"total_tokens":1001}}}}"#,
        ] {
            session.process_line(line);
        }

        let tools = session.usage.turns[0]
            .attribution
            .aggregate
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::ToolsAndMcps))
            .expect("tools category");
        assert!(tools.children.iter().all(|source| source.label != "Exec"));
        let terminal = tools
            .children
            .iter()
            .find(|source| source.label == "Terminal")
            .expect("terminal source");
        assert!(terminal
            .children
            .iter()
            .any(|invocation| invocation.label.starts_with("cargo test #")));
        assert!(terminal
            .children
            .iter()
            .any(|invocation| invocation.label == "cargo test result"));
    }

    #[test]
    fn codex_namespaced_agent_tools_are_attributed_to_agents() {
        let input = serde_json::json!(
            "const agents = await Promise.all(tasks.map(message => tools.multi_agent_v1__spawn_agent({message})));"
        );

        assert_eq!(
            tool_attribution_category_for_call("exec", Some(&input)),
            AttributionCategory::Agents
        );
        assert_eq!(
            tool_source_and_purpose("exec", Some(&input)),
            ("Agent orchestration".to_string(), "Start agent".to_string())
        );
        assert_eq!(
            tool_attribution_category("multi_agent_v1__close_agent"),
            AttributionCategory::Agents
        );
    }

    #[test]
    fn claude_native_image_is_a_direct_document() {
        let mut session =
            SessionState::new(PathBuf::from("claude-image.jsonl"), ProviderKind::Claude);
        for line in [
            r#"{"type":"user","timestamp":"2026-07-14T00:00:00Z","userType":"external","message":{"content":[{"type":"text","text":"inspect image"},{"type":"image","source":{"type":"base64","media_type":"image/png","data":"abc"}}]}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[],"usage":{"input_tokens":100,"output_tokens":1}}}"#,
        ] {
            session.process_line(line);
        }

        let documents = session.usage.turns[0]
            .attribution
            .aggregate
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::DocumentsAndKbs))
            .expect("documents category");
        assert!(documents.tokens > 0);
        assert!(documents
            .children
            .iter()
            .any(|source| source.label.starts_with("Image ")));
    }

    #[test]
    fn claude_compaction_summary_replaces_active_context_for_later_requests() {
        let mut session =
            SessionState::new(PathBuf::from("claude-compact.jsonl"), ProviderKind::Claude);
        for line in [
            r#"{"type":"user","timestamp":"2026-07-14T00:00:00Z","userType":"external","message":{"content":"first turn with a substantial prompt"}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:01Z","message":{"id":"msg-1","model":"claude-opus-4-8","content":[{"type":"text","text":"a substantial response retained in history"}],"usage":{"input_tokens":100,"output_tokens":20},"stop_reason":"end_turn"}}"#,
            r#"{"type":"user","timestamp":"2026-07-14T00:00:02Z","userType":"external","message":{"content":"continue"}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:03Z","message":{"id":"msg-2","model":"claude-opus-4-8","content":[],"usage":{"input_tokens":200,"output_tokens":10}}}"#,
            r#"{"type":"system","subtype":"compact_boundary","summary":"short summary","compactMetadata":{"trigger":"manual"},"timestamp":"2026-07-14T00:00:04Z"}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:05Z","message":{"id":"msg-3","model":"claude-opus-4-8","content":[],"usage":{"input_tokens":50,"output_tokens":10}}}"#,
            r#"{"type":"user","timestamp":"2026-07-14T00:00:06Z","userType":"external","message":{"content":"after compact"}}"#,
            r#"{"type":"assistant","timestamp":"2026-07-14T00:00:07Z","message":{"id":"msg-4","model":"claude-opus-4-8","content":[],"usage":{"input_tokens":80,"output_tokens":10}}}"#,
        ] {
            session.process_line(line);
        }

        let turn = &session.usage.turns[1];
        assert_eq!(turn.telemetry.compactions, 1);
        assert_eq!(turn.attribution.request_count(), 2);
        let before = attributed_tokens(
            &turn.attribution.request(0).unwrap().root,
            AttributionCategory::Context,
        );
        let context_after = attributed_tokens(
            &turn.attribution.request(1).unwrap().root,
            AttributionCategory::Context,
        );
        let compaction_after = attributed_tokens(
            &turn.attribution.request(1).unwrap().root,
            AttributionCategory::Compaction,
        );
        assert!(context_after < before);
        assert!(compaction_after > 0);
        let compaction = turn
            .attribution
            .request(1)
            .unwrap()
            .root
            .children
            .iter()
            .find(|node| node.category == Some(AttributionCategory::Compaction))
            .expect("compaction category");
        assert_eq!(compaction.children[0].label, "Manual compaction");

        let later_request = &session.usage.turns[2].attribution.request(0).unwrap().root;
        assert!(attributed_tokens(later_request, AttributionCategory::Compaction) > 0);
    }

    #[test]
    fn attribution_is_identical_after_cold_replay_and_incremental_tail() {
        use std::io::Write as _;

        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let incremental_path =
            std::env::temp_dir().join(format!("aether-attribution-incremental-{unique}.jsonl"));
        let cold_path =
            std::env::temp_dir().join(format!("aether-attribution-cold-{unique}.jsonl"));
        let lines = [
            r#"{"timestamp":"2026-07-14T00:00:00Z","type":"event_msg","payload":{"type":"user_message","message":"inspect"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:01Z","type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"cargo test\"}","call_id":"call-1"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:02Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":100,"cached_input_tokens":20,"output_tokens":10,"total_tokens":110}}}}"#,
            r#"{"timestamp":"2026-07-14T00:00:03Z","type":"response_item","payload":{"type":"function_call_output","call_id":"call-1","output":"passed"}}"#,
            r#"{"timestamp":"2026-07-14T00:00:04Z","type":"event_msg","payload":{"type":"token_count","info":{"last_token_usage":{"input_tokens":150,"cached_input_tokens":30,"output_tokens":20,"total_tokens":170}}}}"#,
        ];
        fs::write(&incremental_path, format!("{}\n", lines[..3].join("\n"))).unwrap();
        fs::write(&cold_path, format!("{}\n", lines.join("\n"))).unwrap();

        let mut incremental = SessionState::new(incremental_path.clone(), ProviderKind::Codex);
        incremental.poll_file();
        let mut file = fs::OpenOptions::new()
            .append(true)
            .open(&incremental_path)
            .unwrap();
        writeln!(file, "{}", lines[3..].join("\n")).unwrap();
        incremental.poll_file();

        let mut cold = SessionState::new(cold_path.clone(), ProviderKind::Codex);
        cold.poll_file();

        assert_eq!(
            incremental.usage.turns[0].attribution.aggregate,
            cold.usage.turns[0].attribution.aggregate
        );
        assert_eq!(
            incremental.usage.turns[0].attribution.request_count(),
            cold.usage.turns[0].attribution.request_count()
        );

        let _ = fs::remove_file(incremental_path);
        let _ = fs::remove_file(cold_path);
    }

    #[test]
    fn attribution_cache_keeps_only_the_bounded_recent_session_set() {
        let mut engine = LiveEngine::new(Some(ProviderKind::Codex), None);
        for index in 0..(ATTRIBUTION_CACHE_SESSIONS + 2) {
            let path = PathBuf::from(format!("session-{index}.jsonl"));
            let mut session = SessionState::new(path.clone(), ProviderKind::Codex);
            session.usage.turns.push(TurnUsage {
                prompt: "inspect".to_string(),
                timestamp: String::new(),
                input_tokens: 10,
                output_tokens: 0,
                cache_read_tokens: 0,
                cache_write_tokens: 0,
                cost: 0.0,
                agents: Vec::new(),
                cumulative_context: 0,
                context_saved: 0,
                response_text: String::new(),
                cost_known: false,
                telemetry: TurnTelemetry::default(),
                attribution: {
                    let mut value = TurnAttribution::new("inspect", 0);
                    value.record_parent_request("request-1", 10, 0, true);
                    value
                },
            });
            session.attribution_loaded = true;
            engine.sessions.push(session);
            engine.touch_attribution_cache(path);
        }

        assert_eq!(engine.attribution_cache.len(), ATTRIBUTION_CACHE_SESSIONS);
        assert!(!engine.sessions[0].attribution_loaded);
        assert!(!engine.sessions[1].attribution_loaded);
        assert!(engine.sessions[2..]
            .iter()
            .all(|session| session.attribution_loaded));
    }

    #[test]
    fn session_pruning_is_per_provider_and_preserves_the_active_session() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aether-prune-{unique}"));
        fs::create_dir_all(&dir).unwrap();

        let mut engine = LiveEngine::new(None, None);
        for provider in ProviderKind::ALL {
            for index in 0..55 {
                let path = dir.join(format!("{}-{index}.jsonl", provider.id()));
                fs::write(&path, "{}\n").unwrap();
                let mut session = SessionState::new(path, provider);
                session.last_modified = index;
                engine.sessions.push(session);
            }
        }
        let active_path = engine.sessions[0].file_path.clone();

        engine.prune_sessions(Some(&active_path));

        for provider in ProviderKind::ALL {
            assert_eq!(
                engine
                    .sessions
                    .iter()
                    .filter(|session| session.provider == provider)
                    .count(),
                MAX_SESSIONS
            );
        }
        assert!(engine
            .sessions
            .iter()
            .any(|session| session.file_path == active_path));
        assert_eq!(engine.sessions[0].file_path, active_path);

        let _ = fs::remove_dir_all(dir);
    }

    #[test]
    fn rescanning_keeps_active_session_identity_when_recency_changes() {
        let unique = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("aether-rescan-{unique}"));
        fs::create_dir_all(&dir).unwrap();
        let first = dir.join("first.jsonl");
        let second = dir.join("second.jsonl");
        fs::write(
            &first,
            r#"{"type":"session_meta","payload":{"id":"first","cwd":"/tmp/project"}}
"#,
        )
        .unwrap();
        fs::write(
            &second,
            r#"{"type":"session_meta","payload":{"id":"second","cwd":"/tmp/project"}}
"#,
        )
        .unwrap();

        let mut engine = LiveEngine::new(Some(ProviderKind::Codex), Some(dir.clone()));
        engine.scan_sessions();
        let target_idx = engine
            .sessions
            .iter()
            .position(|session| session.file_path == first)
            .unwrap();
        engine.active_idx = target_idx;
        for (index, session) in engine.sessions.iter_mut().enumerate() {
            session.last_modified = if index == target_idx {
                if target_idx == 0 {
                    1
                } else {
                    100
                }
            } else if target_idx == 0 {
                100
            } else {
                1
            };
        }

        engine.scan_sessions();

        assert_eq!(engine.sessions[engine.active_idx].file_path, first);
        let _ = fs::remove_dir_all(dir);
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
