use crate::domain::models::{
    context::ContextText,
    events::{Memory, Source},
    storage::{ChatInput, ChatOutput},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Temporary file content is extracted text only. Raw bytes are never saved.
#[derive(Clone, Serialize, Deserialize)]
pub struct TemporaryFile {
    pub file_id: Uuid,
    pub name: String,
    pub pages: Vec<ContextText>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct TemporaryOutput {
    pub output: ChatOutput,
    pub sources: Vec<Source>,
    pub memories: Vec<Memory>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct TemporaryChat {
    pub session_id: Uuid,
    pub title: String,
    pub persona_id: Option<String>,
    pub inputs: Vec<ChatInput>,
    pub outputs: Vec<TemporaryOutput>,
    pub files: Vec<TemporaryFile>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct TemporaryData {
    pub app_id: String,
    pub user_id: String,
    pub chat: Option<TemporaryChat>,
    pub file: Option<TemporaryFile>,
}
