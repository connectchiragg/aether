use clap::ValueEnum;
use serde::{Deserialize, Serialize};
use std::fs;
use std::io;
use std::path::PathBuf;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, ValueEnum)]
pub enum ProviderKind {
    Claude,
    Codex,
}

impl ProviderKind {
    pub const ALL: [ProviderKind; 2] = [ProviderKind::Claude, ProviderKind::Codex];

    pub fn id(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
        }
    }

    pub fn display_name(self) -> &'static str {
        match self {
            ProviderKind::Claude => "Claude Code",
            ProviderKind::Codex => "Codex",
        }
    }

    pub fn source_label(self) -> &'static str {
        self.id()
    }
}

#[derive(Clone, Debug)]
pub struct ProviderStatus {
    pub kind: ProviderKind,
    pub enabled: bool,
    pub available: bool,
    pub session_count: usize,
    pub last_modified: u64,
}

impl ProviderStatus {
    pub fn state_label(&self) -> &'static str {
        if self.enabled {
            "enabled"
        } else if self.available {
            "available"
        } else {
            "not found"
        }
    }
}

#[derive(Default, Deserialize, Serialize)]
pub struct AetherConfig {
    #[serde(default)]
    pub enabled_providers: Vec<String>,
}

impl AetherConfig {
    pub fn load() -> Self {
        let path = config_path();
        match fs::read_to_string(path) {
            Ok(data) => serde_json::from_str(&data).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    pub fn save(&self) -> io::Result<()> {
        let path = config_path();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let data = serde_json::to_string_pretty(self)?;
        fs::write(path, data)
    }

    pub fn enable(&mut self, provider: ProviderKind) {
        let id = provider.id();
        if !self.enabled_providers.iter().any(|p| p == id) {
            self.enabled_providers.push(id.to_string());
        }
    }

    pub fn is_enabled(&self, provider: ProviderKind) -> bool {
        self.enabled_providers.iter().any(|p| p == provider.id())
    }
}

pub fn config_path() -> PathBuf {
    home_dir()
        .join(".config")
        .join("aether")
        .join("config.json")
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn claude_threads_dir() -> PathBuf {
    home_dir().join(".claude").join("threads")
}

pub fn claude_projects_dir() -> PathBuf {
    home_dir().join(".claude").join("projects")
}

pub fn codex_sessions_dir() -> PathBuf {
    home_dir().join(".codex").join("sessions")
}
