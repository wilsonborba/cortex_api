use crate::domain::models::{
    run_record::WebMode,
    storage::{AppUser, Chat, ChatInput, ChatOutput, InputFile},
};
use serde::Deserialize;
use uuid::Uuid;
fn fast() -> WebMode {
    WebMode::Fast
}
/// Public input body. Deliberately no Debug implementation: it contains text.
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewChat {
    pub app_id: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub file_ids: Vec<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub needs_web: bool,
    #[serde(default = "fast")]
    pub web_mode: WebMode,
    #[serde(default)]
    pub use_memory: bool,
    #[serde(default)]
    pub memory_topic: Option<String>,
    #[serde(default)]
    pub thinking: bool,
    #[serde(default)]
    pub persona_id: Option<String>,
    #[serde(default)]
    pub temporary: bool,
}
#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NextInput {
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub file_ids: Vec<String>,
    #[serde(default)]
    pub stream: bool,
    #[serde(default)]
    pub needs_web: bool,
    #[serde(default = "fast")]
    pub web_mode: WebMode,
    #[serde(default)]
    pub use_memory: bool,
    #[serde(default)]
    pub memory_topic: Option<String>,
    #[serde(default)]
    pub thinking: bool,
}
impl NewChat {
    pub fn input(&self) -> NextInput {
        NextInput {
            text: self.text.clone(),
            file_ids: self.file_ids.clone(),
            stream: self.stream,
            needs_web: self.needs_web,
            web_mode: self.web_mode,
            use_memory: self.use_memory,
            memory_topic: self.memory_topic.clone(),
            thinking: self.thinking,
        }
    }
}
/// File metadata is resolved in the caller's scope before Accept succeeds.
#[derive(Clone)]
pub struct SessionFile {
    pub file_id: String,
    pub memory_id: String,
    pub name: String,
}
/// Storage plan for one accepted permanent input. It never starts inference.
/// The Accept service must finish security and file checks before it persists it.
#[derive(Clone)]
pub struct PreparedInput {
    pub new_chat: Option<Chat>,
    pub scope: AppUser,
    pub input: ChatInput,
    pub output: ChatOutput,
    pub files: Vec<InputFile>,
}
#[derive(Clone, serde::Serialize)]
pub struct InputIds {
    pub session_id: Uuid,
    pub input_id: Uuid,
    pub output_id: Uuid,
}
impl PreparedInput {
    pub fn ids(&self) -> InputIds {
        InputIds {
            session_id: self.input.session_id,
            input_id: self.input.input_id,
            output_id: self.output.output_id,
        }
    }
}
