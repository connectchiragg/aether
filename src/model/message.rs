use chrono::{DateTime, Utc};

#[derive(Clone, Debug)]
pub struct Message {
    pub id: usize,
    pub from: String,
    pub to: String,
    pub content: String,
    pub revealed_chars: usize,
    pub timestamp: DateTime<Utc>,
    pub message_type: MessageType,
}

#[derive(Clone, Debug, PartialEq)]
pub enum MessageType {
    Request,
    Response,
    Delegation,
    StatusUpdate,
}

impl Message {
    pub fn new(id: usize, from: &str, to: &str, content: &str, message_type: MessageType) -> Self {
        Self {
            id,
            from: from.to_string(),
            to: to.to_string(),
            content: content.to_string(),
            revealed_chars: 0,
            timestamp: Utc::now(),
            message_type,
        }
    }

    pub fn visible_content(&self) -> &str {
        let mut end = self.revealed_chars.min(self.content.len());
        // Don't split in the middle of a multi-byte char
        while end < self.content.len() && !self.content.is_char_boundary(end) {
            end += 1;
        }
        &self.content[..end]
    }

    pub fn is_fully_revealed(&self) -> bool {
        self.revealed_chars >= self.content.len()
    }
}
