#[derive(Clone)]
pub struct ChatMessage {
    pub role: MessageRole,
    pub content: String,
}

#[derive(Clone, Copy, Default)]
pub enum MessageRole {
    User,
    #[default]
    Sage,
    Error,
}

impl ChatMessage {
    pub(super) fn user(content: String) -> Self {
        Self {
            role: MessageRole::User,
            content,
        }
    }

    pub(super) fn sage(content: String) -> Self {
        Self {
            role: MessageRole::Sage,
            content,
        }
    }

    pub(super) fn error(content: String) -> Self {
        Self {
            role: MessageRole::Error,
            content,
        }
    }
}
