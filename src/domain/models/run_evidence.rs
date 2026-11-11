use crate::domain::models::run_record::RunMode;
use chrono::{DateTime, Utc};
use uuid::Uuid;
pub struct RunIdentity {
    pub session_id: Uuid,
    pub output_id: Uuid,
    pub app_id: String,
    pub user_id: String,
    pub created_at: DateTime<Utc>,
    pub mode: RunMode,
}
