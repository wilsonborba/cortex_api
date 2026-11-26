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
pub struct ListQuery {
    pub status: Option<ListStatus>,
}
/// `GET /chats?status=running`: chats with a running output (10.4.4).
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum ListStatus {
    Running,
}
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
    /// Bounded scalar metadata recovered from a fully matched persisted run.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run: Option<OutputRunMetadata>,
    /// Source answer when this answer continues a saved answer.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub continued_from: Option<crate::domain::models::inspection::ContinuedFrom>,
}
#[derive(Serialize)]
pub struct OutputRunMetadata {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_number: Option<u16>,
    pub total_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tokens: Option<OutputTokenCounts>,
}
#[derive(Serialize)]
pub struct OutputTokenCounts {
    pub input: u64,
    pub output: u64,
    pub total: u64,
}
#[derive(Serialize)]
pub struct ChatWithMemory {
    #[serde(flatten)]
    pub chat: Chat,
    pub memory_node_id: Option<String>,
}
#[derive(Serialize)]
pub struct ChatPage {
    pub chat: ChatWithMemory,
    pub inputs: Vec<ChatInput>,
    pub outputs: Vec<OutputDetails>,
}
