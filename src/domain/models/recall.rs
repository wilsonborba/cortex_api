use crate::domain::models::{context::ContextText, events::Memory, pipeline::SourceNotice};
use serde::Deserialize;

/// Search responses carry scope so an invalid response cannot cross a boundary.
#[derive(Deserialize)]
pub struct RecalledMemory {
    pub id: String,
    pub workspace_id: String,
    pub app_id: String,
    pub user_id: Option<String>,
    pub memory_type: String,
    pub title: String,
    pub content: Option<String>,
}
#[derive(Default)]
pub struct Recalled {
    pub memories: Vec<ContextText>,
    pub files: Vec<ContextText>,
    pub references: Vec<Memory>,
    pub notices: Vec<SourceNotice>,
}
