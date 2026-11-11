use crate::{
    dal::local::chats::Chats,
    domain::models::storage::{AppUser, StorageError},
};
use sqlx::{Postgres, Transaction};
use uuid::Uuid;
#[derive(sqlx::FromRow)]
struct ExchangeRow {
    output_id: Uuid,
    title: String,
    input_text: String,
    memory_topic: Option<String>,
    output_text: String,
    memory_id: Option<String>,
    status: String,
}
pub struct MemoryAssignment {
    tx: Transaction<'static, Postgres>,
    row: ExchangeRow,
}
impl MemoryAssignment {
    pub fn title(&self) -> &str {
        &self.row.title
    }
    pub fn input(&self) -> &str {
        &self.row.input_text
    }
    pub fn topic(&self) -> Option<&str> {
        self.row.memory_topic.as_deref()
    }
    pub fn output(&self) -> &str {
        &self.row.output_text
    }
    pub fn memory_id(&self) -> Option<&str> {
        self.row.memory_id.as_deref()
    }
    pub async fn assign(mut self, id: &str) -> Result<(), StorageError> {
        sqlx::query("UPDATE chat_outputs SET memory_id=$2 WHERE output_id=$1")
            .bind(self.row.output_id)
            .bind(id)
            .execute(&mut *self.tx)
            .await?;
        self.tx.commit().await?;
        Ok(())
    }
    pub async fn release(self) -> Result<(), StorageError> {
        self.tx.commit().await?;
        Ok(())
    }
}
impl Chats {
    pub async fn lock_completed_exchange(
        &self,
        scope: &AppUser,
        session: Uuid,
        output: Uuid,
    ) -> Result<Option<MemoryAssignment>, StorageError> {
        let mut tx = self.pool.begin().await?;
        // Lock the parent too. Chat deletion must take this same parent lock
        // before it reads memory IDs, so it cannot miss an assignment.
        let row:ExchangeRow=sqlx::query_as("SELECT o.output_id,c.title,i.text AS input_text,i.memory_topic,o.text AS output_text,o.memory_id,o.status FROM chats c JOIN chat_outputs o USING(session_id) JOIN chat_inputs i ON i.input_id=o.input_id WHERE c.session_id=$1 AND c.app_id=$2 AND c.user_id=$3 AND o.output_id=$4 FOR UPDATE OF c,o").bind(session).bind(&scope.app_id).bind(&scope.user_id).bind(output).fetch_optional(&mut *tx).await?.ok_or(StorageError::NotFound)?;
        if row.status != "done" {
            return Ok(None);
        }
        Ok(Some(MemoryAssignment { tx, row }))
    }
}
