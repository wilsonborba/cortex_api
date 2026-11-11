use crate::domain::models::{
    context::Exchange,
    events::{Memory, Source},
};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Clone, Serialize, Deserialize, sqlx::FromRow)]
#[serde(deny_unknown_fields)]
pub struct HistoryExchange {
    pub input_id: Uuid,
    pub input: String,
    pub output: String,
}
impl HistoryExchange {
    pub fn context(&self) -> Exchange {
        Exchange {
            input: self.input.clone(),
            output: self.output.clone(),
        }
    }
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CachedHistory {
    pub session_id: Uuid,
    pub app_id: String,
    pub user_id: String,
    pub completed_count: i64,
    pub exchanges: Vec<HistoryExchange>,
}
#[derive(Clone, Copy)]
pub enum FinishedStatus {
    Done,
    Cancelled,
    Failed,
}
impl FinishedStatus {
    pub fn name(self) -> &'static str {
        match self {
            Self::Done => "done",
            Self::Cancelled => "cancelled",
            Self::Failed => "failed",
        }
    }
}
pub struct FinishedOutput {
    pub session_id: Uuid,
    pub output_id: Uuid,
    pub status: FinishedStatus,
    pub text: String,
    pub thinking_text: Option<String>,
    pub memory_id: Option<String>,
    pub sources: Vec<Source>,
    pub memories: Vec<Memory>,
    pub finished_at: DateTime<Utc>,
}
#[derive(Clone, sqlx::FromRow)]
pub struct ChatFile {
    pub file_id: String,
    pub memory_id: String,
}
