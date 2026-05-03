pub mod scenarios;

use crate::model::{Agent, AgentStatus, Message, MessageType, Task, TaskState};

#[derive(Clone, Debug)]
pub enum ScenarioStep {
    AgentThinks {
        agent: String,
        duration_ticks: u64,
    },
    SendMessage {
        from: String,
        to: String,
        content: String,
        chars_per_tick: usize,
    },
    CreateTask {
        id: String,
        title: String,
        assigned_to: String,
        delegated_by: Option<String>,
    },
    UpdateTaskState {
        task_id: String,
        new_state: TaskState,
    },
    DelegateTask {
        from: String,
        to: String,
        parent_task_id: String,
        task_id: String,
        title: String,
    },
    Pause {
        ticks: u64,
    },
}

enum StepState {
    Idle,
    Thinking { agent: String, remaining: u64 },
    Streaming { message_idx: usize, chars_per_tick: usize },
    Pausing { remaining: u64 },
}

pub struct MockEngine {
    scenario: Vec<ScenarioStep>,
    current_step: usize,
    state: StepState,
    pub agents: Vec<Agent>,
    pub messages: Vec<Message>,
    pub tasks: Vec<Task>,
    next_message_id: usize,
    pub tick_count: u64,
}

impl MockEngine {
    pub fn new() -> Self {
        let agents = vec![
            Agent::new(
                "orchestrator",
                "Claude-Prime",
                "Orchestrator",
                crate::theme::AGENT_COLORS[0],
                vec!["planning", "delegation", "synthesis"],
            ),
            Agent::new(
                "coder",
                "Claude-Dev",
                "Coder",
                crate::theme::AGENT_COLORS[1],
                vec!["code generation", "schema design", "debugging"],
            ),
            Agent::new(
                "reviewer",
                "Claude-Review",
                "Reviewer",
                crate::theme::AGENT_COLORS[2],
                vec!["code review", "security audit", "best practices"],
            ),
        ];

        let scenario = scenarios::default_scenario();

        Self {
            scenario,
            current_step: 0,
            state: StepState::Idle,
            agents,
            messages: Vec::new(),
            tasks: Vec::new(),
            next_message_id: 0,
            tick_count: 0,
        }
    }

    pub fn reset(&mut self) {
        self.current_step = 0;
        self.state = StepState::Idle;
        self.messages.clear();
        self.tasks.clear();
        self.next_message_id = 0;
        self.tick_count = 0;
        for agent in &mut self.agents {
            agent.status = AgentStatus::Idle;
        }
    }

    pub fn is_finished(&self) -> bool {
        self.current_step >= self.scenario.len() && matches!(self.state, StepState::Idle)
    }

    pub fn tick(&mut self) {
        self.tick_count += 1;

        // Animate thinking dots for all thinking agents
        for agent in &mut self.agents {
            if let AgentStatus::Thinking { ref mut dots } = agent.status {
                *dots += 1;
            }
        }

        match &mut self.state {
            StepState::Idle => {
                self.advance_to_next_step();
            }
            StepState::Thinking { agent, remaining } => {
                *remaining -= 1;
                if *remaining == 0 {
                    let agent_id = agent.clone();
                    self.set_agent_status(&agent_id, AgentStatus::Idle);
                    self.state = StepState::Idle;
                }
            }
            StepState::Streaming {
                message_idx,
                chars_per_tick,
            } => {
                let cpt = *chars_per_tick;
                let idx = *message_idx;
                if let Some(msg) = self.messages.get_mut(idx) {
                    msg.revealed_chars += cpt;
                    if msg.is_fully_revealed() {
                        let from = msg.from.clone();
                        self.set_agent_status(&from, AgentStatus::Idle);
                        self.state = StepState::Idle;
                    }
                } else {
                    self.state = StepState::Idle;
                }
            }
            StepState::Pausing { remaining } => {
                *remaining -= 1;
                if *remaining == 0 {
                    self.state = StepState::Idle;
                }
            }
        }
    }

    fn advance_to_next_step(&mut self) {
        if self.current_step >= self.scenario.len() {
            return;
        }

        let step = self.scenario[self.current_step].clone();
        self.current_step += 1;

        match step {
            ScenarioStep::AgentThinks {
                agent,
                duration_ticks,
            } => {
                self.set_agent_status(&agent, AgentStatus::Thinking { dots: 0 });
                self.state = StepState::Thinking {
                    agent,
                    remaining: duration_ticks,
                };
            }
            ScenarioStep::SendMessage {
                from,
                to,
                content,
                chars_per_tick,
            } => {
                self.set_agent_status(&from, AgentStatus::Streaming);
                let msg_type = if from == "orchestrator" && to != "orchestrator" {
                    MessageType::Delegation
                } else {
                    MessageType::Response
                };
                let id = self.next_message_id;
                self.next_message_id += 1;
                let msg = Message::new(id, &from, &to, &content, msg_type);
                self.messages.push(msg);
                let message_idx = self.messages.len() - 1;
                self.state = StepState::Streaming {
                    message_idx,
                    chars_per_tick,
                };
            }
            ScenarioStep::CreateTask {
                id,
                title,
                assigned_to,
                delegated_by,
            } => {
                let task = Task::new(&id, &title, &assigned_to, delegated_by.as_deref());
                self.tasks.push(task);
                // Immediately advance to next step
                self.advance_to_next_step();
            }
            ScenarioStep::UpdateTaskState { task_id, new_state } => {
                self.update_task_state_recursive(&task_id, new_state);
                self.advance_to_next_step();
            }
            ScenarioStep::DelegateTask {
                from: _,
                to,
                parent_task_id,
                task_id,
                title,
            } => {
                let subtask = Task::new(&task_id, &title, &to, Some(&parent_task_id));
                // Find parent and add subtask
                if !Self::add_subtask_recursive(&mut self.tasks, &parent_task_id, subtask.clone()) {
                    // If parent not found, add at root level
                    self.tasks.push(subtask);
                }
                self.advance_to_next_step();
            }
            ScenarioStep::Pause { ticks } => {
                self.state = StepState::Pausing { remaining: ticks };
            }
        }
    }

    fn set_agent_status(&mut self, agent_id: &str, status: AgentStatus) {
        if let Some(agent) = self.agents.iter_mut().find(|a| a.id == agent_id) {
            agent.status = status;
        }
    }

    fn update_task_state_recursive(&mut self, task_id: &str, new_state: TaskState) {
        for task in &mut self.tasks {
            if task.id == task_id {
                task.state = new_state;
                return;
            }
            Self::update_subtask_state(&mut task.subtasks, task_id, new_state.clone());
        }
    }

    fn update_subtask_state(tasks: &mut [Task], task_id: &str, new_state: TaskState) {
        for task in tasks {
            if task.id == task_id {
                task.state = new_state;
                return;
            }
            Self::update_subtask_state(&mut task.subtasks, task_id, new_state.clone());
        }
    }

    fn add_subtask_recursive(tasks: &mut Vec<Task>, parent_id: &str, subtask: Task) -> bool {
        for task in tasks.iter_mut() {
            if task.id == parent_id {
                task.subtasks.push(subtask);
                return true;
            }
            if Self::add_subtask_recursive(&mut task.subtasks, parent_id, subtask.clone()) {
                return true;
            }
        }
        false
    }
}
