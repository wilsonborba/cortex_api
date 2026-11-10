use crate::domain::models::{
    events::{Memory, Source},
    storage::{Chat, ChatInput, ChatOutput},
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageQuery {
    pub before: Option<Uuid>,
    pub limit: Option<u32>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ListQuery {}
#[derive(Deserialize)]
#[serde(untagged, deny_unknown_fields)]
pub enum ChatPatch {
    Title { title: String },
    Pin { is_pinned: bool },
}
#[derive(Serialize)]
pub struct OutputDetails {
    #[serde(flatten)]
    pub output: ChatOutput,
    pub sources: Vec<Source>,
    pub memories: Vec<Memory>,
}
#[derive(Serialize)]
pub struct ChatPage {
    pub chat: Chat,
    pub inputs: Vec<ChatInput>,
    pub outputs: Vec<OutputDetails>,
}
