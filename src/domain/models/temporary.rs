use crate::domain::models::{
    context::ContextText,
    events::{Memory, Source},
    model_profile::ModelProfile,
    storage::{ChatInput, ChatOutput},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Temporary text and optional vision payload stay only in the scoped Redis record.
#[derive(Clone, Serialize, Deserialize)]
pub struct TemporaryFile {
    pub file_id: Uuid,
    pub name: String,
    #[serde(default = "unknown_content_type")]
    pub content_type: String,
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
    #[serde(default = "legacy_model_id")]
    pub model_id: String,
    pub title: String,
    pub persona_id: Option<String>,
    pub inputs: Vec<ChatInput>,
    pub outputs: Vec<TemporaryOutput>,
    pub files: Vec<TemporaryFile>,
}

fn unknown_content_type() -> String {
    "application/octet-stream".into()
}

fn legacy_model_id() -> String {
    ModelProfile::legacy_default().id().into()
}
#[derive(Clone, Serialize, Deserialize)]
pub struct TemporaryData {
    pub app_id: String,
    pub user_id: String,
    pub chat: Option<TemporaryChat>,
    pub file: Option<TemporaryFile>,
}
