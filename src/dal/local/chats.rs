use crate::domain::models::storage::*;
use sqlx::PgPool;
use uuid::Uuid;
#[derive(Clone)]
pub struct Chats {
    pub pool: PgPool,
}
fn inserted(rows: u64) -> Result<(), StorageError> {
    if rows == 1 {
        Ok(())
    } else {
        Err(StorageError::NotFound)
    }
}
impl Chats {
    pub async fn insert_chat(&self, c: &Chat) -> Result<(), StorageError> {
        sqlx::query("INSERT INTO chats(session_id,app_id,user_id,title,is_pinned,created_at,updated_at) VALUES ($1,$2,$3,$4,$5,$6,$7)").bind(c.session_id).bind(&c.app_id).bind(&c.user_id).bind(&c.title).bind(c.is_pinned).bind(c.created_at).bind(c.updated_at).execute(&self.pool).await?;
        Ok(())
    }
    pub async fn chat(&self, s: &AppUser, id: Uuid) -> Result<Option<Chat>, StorageError> {
        Ok(
            sqlx::query_as("SELECT * FROM chats WHERE session_id=$1 AND app_id=$2 AND user_id=$3")
                .bind(id)
                .bind(&s.app_id)
                .bind(&s.user_id)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    pub async fn insert_input(&self, s: &AppUser, i: &ChatInput) -> Result<(), StorageError> {
        inserted(sqlx::query("INSERT INTO chat_inputs(input_id,session_id,text,needs_web,web_mode,use_memory,memory_topic,thinking,created_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9 FROM chats WHERE session_id=$2 AND app_id=$10 AND user_id=$11").bind(i.input_id).bind(i.session_id).bind(&i.text).bind(i.needs_web).bind(&i.web_mode).bind(i.use_memory).bind(&i.memory_topic).bind(i.thinking).bind(i.created_at).bind(&s.app_id).bind(&s.user_id).execute(&self.pool).await?.rows_affected())
    }
    pub async fn input(&self, s: &AppUser, id: Uuid) -> Result<Option<ChatInput>, StorageError> {
        Ok(sqlx::query_as("SELECT i.* FROM chat_inputs i JOIN chats c USING(session_id) WHERE i.input_id=$1 AND c.app_id=$2 AND c.user_id=$3").bind(id).bind(&s.app_id).bind(&s.user_id).fetch_optional(&self.pool).await?)
    }
    pub async fn insert_file(&self, s: &AppUser, f: &InputFile) -> Result<(), StorageError> {
        inserted(sqlx::query("INSERT INTO chat_input_files(input_id,file_id,memory_id,position) SELECT $1,$2,$3,$4 FROM chat_inputs i JOIN chats c USING(session_id) WHERE i.input_id=$1 AND c.app_id=$5 AND c.user_id=$6").bind(f.input_id).bind(&f.file_id).bind(&f.memory_id).bind(f.position).bind(&s.app_id).bind(&s.user_id).execute(&self.pool).await?.rows_affected())
    }
    pub async fn files(&self, s: &AppUser, id: Uuid) -> Result<Vec<InputFile>, StorageError> {
        Ok(sqlx::query_as("SELECT f.* FROM chat_input_files f JOIN chat_inputs i USING(input_id) JOIN chats c USING(session_id) WHERE f.input_id=$1 AND c.app_id=$2 AND c.user_id=$3 ORDER BY f.position").bind(id).bind(&s.app_id).bind(&s.user_id).fetch_all(&self.pool).await?)
    }
    pub async fn insert_output(&self, s: &AppUser, o: &ChatOutput) -> Result<(), StorageError> {
        inserted(sqlx::query("INSERT INTO chat_outputs(output_id,input_id,session_id,status,text,thinking_text,memory_id,created_at,finished_at) SELECT $1,$2,$3,$4,$5,$6,$7,$8,$9 FROM chat_inputs i JOIN chats c USING(session_id) WHERE i.input_id=$2 AND i.session_id=$3 AND c.app_id=$10 AND c.user_id=$11").bind(o.output_id).bind(o.input_id).bind(o.session_id).bind(&o.status).bind(&o.text).bind(&o.thinking_text).bind(&o.memory_id).bind(o.created_at).bind(o.finished_at).bind(&s.app_id).bind(&s.user_id).execute(&self.pool).await?.rows_affected())
    }
    pub async fn output(&self, s: &AppUser, id: Uuid) -> Result<Option<ChatOutput>, StorageError> {
        Ok(sqlx::query_as("SELECT o.* FROM chat_outputs o JOIN chats c USING(session_id) WHERE o.output_id=$1 AND c.app_id=$2 AND c.user_id=$3").bind(id).bind(&s.app_id).bind(&s.user_id).fetch_optional(&self.pool).await?)
    }
    pub async fn insert_source(&self, s: &AppUser, v: &OutputSource) -> Result<(), StorageError> {
        inserted(sqlx::query("INSERT INTO chat_output_sources(output_id,position,title,url) SELECT $1,$2,$3,$4 FROM chat_outputs o JOIN chats c USING(session_id) WHERE o.output_id=$1 AND c.app_id=$5 AND c.user_id=$6").bind(v.output_id).bind(v.position).bind(&v.title).bind(&v.url).bind(&s.app_id).bind(&s.user_id).execute(&self.pool).await?.rows_affected())
    }
    pub async fn sources(&self, s: &AppUser, id: Uuid) -> Result<Vec<OutputSource>, StorageError> {
        Ok(sqlx::query_as("SELECT v.* FROM chat_output_sources v JOIN chat_outputs o USING(output_id) JOIN chats c USING(session_id) WHERE v.output_id=$1 AND c.app_id=$2 AND c.user_id=$3 ORDER BY position").bind(id).bind(&s.app_id).bind(&s.user_id).fetch_all(&self.pool).await?)
    }
    pub async fn insert_memory(&self, s: &AppUser, v: &OutputMemory) -> Result<(), StorageError> {
        inserted(sqlx::query("INSERT INTO chat_output_memories(output_id,position,memory_id,title) SELECT $1,$2,$3,$4 FROM chat_outputs o JOIN chats c USING(session_id) WHERE o.output_id=$1 AND c.app_id=$5 AND c.user_id=$6").bind(v.output_id).bind(v.position).bind(&v.memory_id).bind(&v.title).bind(&s.app_id).bind(&s.user_id).execute(&self.pool).await?.rows_affected())
    }
    pub async fn memories(&self, s: &AppUser, id: Uuid) -> Result<Vec<OutputMemory>, StorageError> {
        Ok(sqlx::query_as("SELECT v.* FROM chat_output_memories v JOIN chat_outputs o USING(output_id) JOIN chats c USING(session_id) WHERE v.output_id=$1 AND c.app_id=$2 AND c.user_id=$3 ORDER BY position").bind(id).bind(&s.app_id).bind(&s.user_id).fetch_all(&self.pool).await?)
    }
}
