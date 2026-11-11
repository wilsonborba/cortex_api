use crate::domain::models::{
    context::ContextText,
    events::{Memory, Source},
    storage::{ChatInput, ChatOutput},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Temporary text and optional vision payload stay only in the scoped Redis record.
#[derive(Clone, Serialize, Deserialize)]
pub struct TemporaryFile {
    pub file_id: Uuid,
    pub name: String,
    pub pages: Vec<ContextText>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
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
