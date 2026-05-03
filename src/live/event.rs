use serde::Deserialize;

#[derive(Deserialize, Debug)]
#[serde(tag = "type")]
pub enum LiveEvent {
    #[serde(rename = "session_start")]
    SessionStart {
        #[serde(rename = "sessionId")]
        session_id: String,
        #[serde(default)]
        name: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[allow(dead_code)]
        ts: u64,
    },
    #[serde(rename = "turn_start")]
    TurnStart {
        #[serde(rename = "turnIndex")]
        turn_index: usize,
        #[serde(default)]
        prompt: Option<String>,
        #[allow(dead_code)]
        ts: u64,
    },
    #[serde(rename = "session_clear")]
    SessionClear {
        #[allow(dead_code)]
        ts: u64,
    },
    #[serde(rename = "agent_spawn")]
    AgentSpawn {
        id: String,
        name: String,
        role: String,
        #[allow(dead_code)]
        ts: u64,
    },
    #[serde(rename = "message")]
    Message {
        from: String,
        to: String,
        content: String,
        #[allow(dead_code)]
        ts: u64,
    },
    #[serde(rename = "agent_done")]
    AgentDone {
        id: String,
        #[allow(dead_code)]
        ts: u64,
    },
}
