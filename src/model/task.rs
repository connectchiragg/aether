#[derive(Clone, Debug)]
pub struct Task {
    pub id: String,
    pub title: String,
    pub state: TaskState,
    pub assigned_to: String,
    pub delegated_by: Option<String>,
    pub subtasks: Vec<Task>,
    pub collapsed: bool,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TaskState {
    Submitted,
    Working,
    InputRequired,
    Completed,
    Failed,
}

impl Task {
    pub fn new(id: &str, title: &str, assigned_to: &str, delegated_by: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            title: title.to_string(),
            state: TaskState::Submitted,
            assigned_to: assigned_to.to_string(),
            delegated_by: delegated_by.map(String::from),
            subtasks: Vec::new(),
            collapsed: false,
        }
    }

    pub fn state_icon(&self) -> &str {
        match self.state {
            TaskState::Submitted => "○",
            TaskState::Working => "◑",
            TaskState::InputRequired => "◎",
            TaskState::Completed => "●",
            TaskState::Failed => "✗",
        }
    }

    pub fn state_label(&self) -> &str {
        match self.state {
            TaskState::Submitted => "submitted",
            TaskState::Working => "working",
            TaskState::InputRequired => "input required",
            TaskState::Completed => "completed",
            TaskState::Failed => "failed",
        }
    }
}
