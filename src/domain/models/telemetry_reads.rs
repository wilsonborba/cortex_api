use crate::domain::models::run_record::RunMode;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;
#[derive(Serialize)]
pub struct TimingAverage {
    pub kind: RunMode,
    pub expected_ms: Option<u64>,
}
#[derive(Serialize)]
pub struct GroupStats {
    pub kind: RunMode,
    pub total_runs: u64,
    pub avg_latency_seconds: f64,
    pub avg_latency_ms: f64,
    pub avg_input_tokens: f64,
    pub avg_output_tokens: f64,
    pub avg_total_tokens: f64,
    pub success_rate: f64,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct CallEvent {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user_id: Option<String>,
    pub request_id: String,
    pub output_id: Uuid,
    pub model: String,
    pub role: String,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
    pub started_at: DateTime<Utc>,
    pub finished_at: DateTime<Utc>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EventPage {
    pub before: Option<String>,
    pub limit: Option<usize>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum TelemetryReadError {
    Unavailable,
    InvalidCursor,
}
