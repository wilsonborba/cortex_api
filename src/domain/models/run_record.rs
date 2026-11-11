use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize, Serializer};
use uuid::Uuid;
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunRecord {
    pub session_id: Uuid,
    pub output_id: Uuid,
    pub app_id: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub mode: RunMode,
    pub total_ms: u64,
    pub stages: Vec<StageRecord>,
    pub calls: Vec<ModelCall>,
    pub counts: RunCounts,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunMode {
    pub web_mode: WebMode,
    pub thinking: bool,
}
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum WebMode {
    Fast,
    Medium,
    Deep,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct StageRecord {
    pub name: String,
    pub status: String,
    pub error_number: Option<u16>,
    pub notice_number: Option<u16>,
    pub time_ms: u64,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ModelCall {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub images: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<u32>,
    pub role: String,
    pub model: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub time_to_first_token_ms: Option<u64>,
    pub total_ms: u64,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct RunCounts {
    pub pages_read: u32,
    pub chunks_used: u32,
    pub files: u32,
    pub file_pages: u32,
    #[serde(serialize_with = "audio_seconds")]
    pub audio_seconds: f64,
}
fn audio_seconds<S: Serializer>(value: &f64, serializer: S) -> Result<S::Ok, S::Error> {
    if !value.is_finite() || *value < 0.0 {
        return Err(serde::ser::Error::custom(
            "Audio duration must be finite and nonnegative",
        ));
    }
    serializer.serialize_f64(*value)
}
