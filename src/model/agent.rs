use ratatui::style::Color;

#[derive(Clone, Debug)]
pub struct Agent {
    pub id: String,
    pub name: String,
    pub role: String,
    pub color: Color,
    pub status: AgentStatus,
    pub capabilities: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentStatus {
    Idle,
    Thinking { dots: usize },
    Streaming,
    WaitingForInput,
}

impl Agent {
    pub fn new(id: &str, name: &str, role: &str, color: Color, capabilities: Vec<&str>) -> Self {
        Self {
            id: id.to_string(),
            name: name.to_string(),
            role: role.to_string(),
            color,
            status: AgentStatus::Idle,
            capabilities: capabilities.into_iter().map(String::from).collect(),
        }
    }

    pub fn status_text(&self) -> String {
        match &self.status {
            AgentStatus::Idle => "● Idle".to_string(),
            AgentStatus::Thinking { dots } => {
                let d = ".".repeat(*dots % 4);
                format!("◌ Thinking{d}")
            }
            AgentStatus::Streaming => "◉ Streaming".to_string(),
            AgentStatus::WaitingForInput => "◎ Waiting".to_string(),
        }
    }
}
