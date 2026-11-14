//! Cortex chat rows that tell where a memory came from and when an answer
//! used it (cortex_api #121). Every row is scoped to one app and user.
use crate::domain::models::storage::{AppUser, StorageError};
use chrono::{DateTime, Utc};
use sqlx::PgPool;

#[derive(Clone)]
pub struct MemoryRecords {
    pub pool: PgPool,
}

/// One row: `kind` is `chat` (exchange memory of an output), `file` (memory
/// of a chat input file) or `used` (memory used in an answer).
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct MemoryRecord {
    pub kind: String,
    pub memory_id: String,
    /// Session id for `chat` and `used`, file id for `file`.
    pub ref_id: String,
    /// The chat title.
    pub ref_title: String,
    pub session_id: String,
    pub at: DateTime<Utc>,
    pub output_id: Option<String>,
}

impl MemoryRecords {
    /// One query for sources and usage of the caller's memories.
    pub async fn load(&self, scope: &AppUser) -> Result<Vec<MemoryRecord>, StorageError> {
        Ok(sqlx::query_as(
            "SELECT 'chat' AS kind, o.memory_id AS memory_id, c.session_id::text AS ref_id, \
             c.title AS ref_title, c.session_id::text AS session_id, o.created_at AS at, \
             o.output_id::text AS output_id \
             FROM chat_outputs o JOIN chats c USING(session_id) \
             WHERE c.app_id=$1 AND c.user_id=$2 AND o.memory_id IS NOT NULL \
             UNION ALL \
             SELECT 'file', f.memory_id, f.file_id, c.title, c.session_id::text, i.created_at, NULL \
             FROM chat_input_files f JOIN chat_inputs i USING(input_id) \
             JOIN chats c ON c.session_id=i.session_id \
             WHERE c.app_id=$1 AND c.user_id=$2 \
             UNION ALL \
             SELECT 'used', m.memory_id, c.session_id::text, c.title, c.session_id::text, \
             o.created_at, o.output_id::text \
             FROM chat_output_memories m JOIN chat_outputs o USING(output_id) \
             JOIN chats c ON c.session_id=o.session_id \
             WHERE c.app_id=$1 AND c.user_id=$2",
        )
        .bind(&scope.app_id)
        .bind(&scope.user_id)
        .fetch_all(&self.pool)
        .await?)
    }
}
