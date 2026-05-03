use crate::live::LiveEngine;
use crate::mock::MockEngine;
use crate::model::{Agent, Message};

pub enum Engine {
    Mock(MockEngine),
    Live(LiveEngine),
}

impl Engine {
    pub fn agents(&self) -> &[Agent] {
        match self {
            Engine::Mock(e) => &e.agents,
            Engine::Live(e) => e.agents(),
        }
    }

    pub fn messages(&self) -> &[Message] {
        match self {
            Engine::Mock(e) => &e.messages,
            Engine::Live(e) => e.messages(),
        }
    }

    /// Returns true if the engine wants to auto-lock to a session.
    pub fn tick(&mut self, session_locked: bool) -> bool {
        match self {
            Engine::Mock(e) => {
                e.tick();
                false
            }
            Engine::Live(e) => e.tick(session_locked),
        }
    }

    pub fn is_finished(&self) -> bool {
        match self {
            Engine::Mock(e) => e.is_finished(),
            Engine::Live(_) => false,
        }
    }

    pub fn reset(&mut self) {
        match self {
            Engine::Mock(e) => e.reset(),
            Engine::Live(e) => e.reset(),
        }
    }

    pub fn is_live(&self) -> bool {
        matches!(self, Engine::Live(_))
    }

    pub fn status_text(&self) -> &str {
        match self {
            Engine::Mock(e) => {
                if e.is_finished() {
                    "done"
                } else {
                    "running"
                }
            }
            Engine::Live(e) => {
                if e.file_found() {
                    "watching"
                } else {
                    "waiting for sessions"
                }
            }
        }
    }

    pub fn live_engine(&self) -> Option<&LiveEngine> {
        match self {
            Engine::Live(e) => Some(e),
            _ => None,
        }
    }

    pub fn live_engine_mut(&mut self) -> Option<&mut LiveEngine> {
        match self {
            Engine::Live(e) => Some(e),
            _ => None,
        }
    }
}
