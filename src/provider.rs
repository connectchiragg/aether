use std::env;
use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq)]
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

    fn executable_name(self) -> &'static str {
        match self {
            ProviderKind::Claude => "claude",
            ProviderKind::Codex => "codex",
        }
    }
}

#[derive(Clone, Debug)]
pub struct ProviderStatus {
    pub kind: ProviderKind,
    pub available: bool,
    pub session_count: usize,
    pub last_activity: u64,
}

impl ProviderStatus {
    pub fn state_label(&self) -> &'static str {
        if self.available {
            "tracked"
        } else {
            "not found"
        }
    }
}

pub fn config_dir() -> PathBuf {
    home_dir().join(".config").join("aether")
}

pub fn home_dir() -> PathBuf {
    std::env::var("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("."))
}

pub fn claude_projects_dir() -> PathBuf {
    home_dir().join(".claude").join("projects")
}

pub fn codex_sessions_dir() -> PathBuf {
    home_dir().join(".codex").join("sessions")
}

pub fn codex_session_index_path() -> PathBuf {
    home_dir().join(".codex").join("session_index.jsonl")
}

pub fn provider_present(provider: ProviderKind) -> bool {
    let native_data_exists = match provider {
        ProviderKind::Claude => claude_projects_dir().exists(),
        ProviderKind::Codex => codex_sessions_dir().exists() || codex_session_index_path().exists(),
    };
    native_data_exists || command_available(provider.executable_name())
}

fn command_available(command: &str) -> bool {
    let Some(path) = env::var_os("PATH") else {
        return false;
    };
    let extensions: &[&str] = if cfg!(windows) {
        &["", ".exe", ".cmd", ".bat"]
    } else {
        &[""]
    };

    env::split_paths(&path).any(|directory| {
        extensions
            .iter()
            .any(|extension| executable_file(&directory.join(format!("{command}{extension}"))))
    })
}

fn executable_file(path: &Path) -> bool {
    let Ok(metadata) = path.metadata() else {
        return false;
    };
    if !metadata.is_file() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        metadata.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        true
    }
}

#[cfg(test)]
mod tests {
    use super::{ProviderKind, ProviderStatus};

    #[test]
    fn present_providers_are_always_tracked() {
        let status = ProviderStatus {
            kind: ProviderKind::Codex,
            available: true,
            session_count: 0,
            last_activity: 0,
        };
        assert_eq!(status.state_label(), "tracked");
    }
}
