use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Debug, Clone)]
pub struct AppUser {
    pub app_id: String,
    pub user_id: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct Chat {
    pub session_id: Uuid,
    pub app_id: String,
    pub user_id: String,
    pub title: String,
    pub is_pinned: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct ChatInput {
    pub input_id: Uuid,
    pub session_id: Uuid,
    pub text: String,
    pub needs_web: bool,
    pub web_mode: String,
    pub use_memory: bool,
    pub memory_topic: Option<String>,
    pub thinking: bool,
    pub created_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct InputFile {
    pub input_id: Uuid,
    pub file_id: String,
    pub memory_id: String,
    pub position: i32,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct ChatOutput {
    pub output_id: Uuid,
    pub input_id: Uuid,
    pub session_id: Uuid,
    pub status: String,
    pub text: String,
    pub thinking_text: Option<String>,
    pub memory_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub finished_at: Option<DateTime<Utc>>,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct OutputSource {
    pub output_id: Uuid,
    pub position: i32,
    pub title: String,
    pub url: String,
}
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow, PartialEq, Eq)]
pub struct OutputMemory {
    pub output_id: Uuid,
    pub position: i32,
    pub memory_id: String,
    pub title: String,
}
#[derive(Debug, PartialEq, Eq)]
pub enum StorageError {
    Database,
    NotFound,
}
impl From<sqlx::Error> for StorageError {
    fn from(error: sqlx::Error) -> Self {
        if let Some(db) = error.as_database_error() {
            tracing::error!(sqlstate=?db.code(),constraint=?db.constraint(),"Database operation failed");
        }
        Self::Database
    }
}
