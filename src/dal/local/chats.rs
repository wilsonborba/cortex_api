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
        sqlx::query("INSERT INTO chats(session_id,app_id,user_id,title,is_pinned,created_at,updated_at,persona_id) VALUES ($1,$2,$3,$4,$5,$6,$7,$8)").bind(c.session_id).bind(&c.app_id).bind(&c.user_id).bind(&c.title).bind(c.is_pinned).bind(c.created_at).bind(c.updated_at).bind(&c.persona_id).execute(&self.pool).await?;
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

impl Chats {
    /// Called after Accept. The complete write either commits or rolls back.
    pub async fn save_accepted(
        &self,
        prepared: &crate::domain::models::chat_session::PreparedInput,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        if let Some(chat) = &prepared.new_chat {
            sqlx::query("INSERT INTO chats(session_id,app_id,user_id,title,is_pinned,created_at,updated_at,persona_id) VALUES($1,$2,$3,$4,$5,$6,$7,$8)").bind(chat.session_id).bind(&chat.app_id).bind(&chat.user_id).bind(&chat.title).bind(chat.is_pinned).bind(chat.created_at).bind(chat.updated_at).bind(&chat.persona_id).execute(&mut *transaction).await?;
        }
        let input = &prepared.input;
        let scope = &prepared.scope;
        let output = &prepared.output;
        let updated=sqlx::query("UPDATE chats SET updated_at=GREATEST(updated_at,$4) WHERE session_id=$1 AND app_id=$2 AND user_id=$3").bind(input.session_id).bind(&scope.app_id).bind(&scope.user_id).bind(input.created_at).execute(&mut *transaction).await?.rows_affected();
        inserted(updated)?;
        sqlx::query("INSERT INTO chat_inputs(input_id,session_id,text,needs_web,web_mode,use_memory,memory_topic,thinking,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(input.input_id).bind(input.session_id).bind(&input.text).bind(input.needs_web).bind(&input.web_mode).bind(input.use_memory).bind(&input.memory_topic).bind(input.thinking).bind(input.created_at).execute(&mut *transaction).await?;
        sqlx::query("INSERT INTO chat_outputs(output_id,input_id,session_id,status,text,thinking_text,memory_id,created_at,finished_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)").bind(output.output_id).bind(output.input_id).bind(output.session_id).bind(&output.status).bind(&output.text).bind(&output.thinking_text).bind(&output.memory_id).bind(output.created_at).bind(output.finished_at).execute(&mut *transaction).await?;
        for file in &prepared.files {
            sqlx::query("INSERT INTO chat_input_files(input_id,file_id,memory_id,position) VALUES($1,$2,$3,$4)").bind(file.input_id).bind(&file.file_id).bind(&file.memory_id).bind(file.position).execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(())
    }
}

impl Chats {
    pub async fn history_head(
        &self,
        scope: &AppUser,
        session: Uuid,
    ) -> Result<Option<i64>, StorageError> {
        Ok(sqlx::query_scalar("SELECT (SELECT count(*) FROM chat_outputs o WHERE o.session_id=c.session_id AND o.status='done') FROM chats c WHERE c.session_id=$1 AND c.app_id=$2 AND c.user_id=$3").bind(session).bind(&scope.app_id).bind(&scope.user_id).fetch_optional(&self.pool).await?)
    }
    pub async fn recent_history(
        &self,
        scope: &AppUser,
        session: Uuid,
        limit: usize,
    ) -> Result<Vec<crate::domain::models::history::HistoryExchange>, StorageError> {
        let limit = i64::try_from(limit).map_err(|_| StorageError::Database)?;
        Ok(sqlx::query_as("SELECT i.input_id,i.text AS input,o.text AS output FROM chat_inputs i JOIN chat_outputs o USING(input_id,session_id) JOIN chats c USING(session_id) WHERE c.session_id=$1 AND c.app_id=$2 AND c.user_id=$3 AND o.status='done' ORDER BY i.created_at DESC,i.input_id DESC LIMIT $4").bind(session).bind(&scope.app_id).bind(&scope.user_id).bind(limit).fetch_all(&self.pool).await?)
    }
    pub async fn all_chat_files(
        &self,
        scope: &AppUser,
        session: Uuid,
    ) -> Result<Vec<crate::domain::models::history::ChatFile>, StorageError> {
        Ok(sqlx::query_as("SELECT f.file_id,f.memory_id FROM chat_input_files f JOIN chat_inputs i USING(input_id) JOIN chats c USING(session_id) WHERE c.session_id=$1 AND c.app_id=$2 AND c.user_id=$3 GROUP BY f.file_id,f.memory_id ORDER BY min(i.created_at),f.file_id,f.memory_id").bind(session).bind(&scope.app_id).bind(&scope.user_id).fetch_all(&self.pool).await?)
    }
    pub async fn finish_output(
        &self,
        scope: &AppUser,
        output: &crate::domain::models::history::FinishedOutput,
    ) -> Result<(), StorageError> {
        let mut tx = self.pool.begin().await?;
        let rows=sqlx::query("UPDATE chat_outputs o SET status=$4,text=$5,thinking_text=$6,memory_id=$7,finished_at=$8 FROM chats c WHERE o.session_id=c.session_id AND o.session_id=$1 AND o.output_id=$2 AND c.app_id=$3 AND c.user_id=$9 AND o.status='running'").bind(output.session_id).bind(output.output_id).bind(&scope.app_id).bind(output.status.name()).bind(&output.text).bind(&output.thinking_text).bind(&output.memory_id).bind(output.finished_at).bind(&scope.user_id).execute(&mut *tx).await?.rows_affected();
        inserted(rows)?;
        for (position, source) in output.sources.iter().enumerate() {
            sqlx::query(
                "INSERT INTO chat_output_sources(output_id,position,title,url) VALUES($1,$2,$3,$4)",
            )
            .bind(output.output_id)
            .bind(i32::try_from(position).map_err(|_| StorageError::Database)?)
            .bind(&source.title)
            .bind(&source.url)
            .execute(&mut *tx)
            .await?;
        }
        for (position, memory) in output.memories.iter().enumerate() {
            sqlx::query("INSERT INTO chat_output_memories(output_id,position,memory_id,title) VALUES($1,$2,$3,$4)").bind(output.output_id).bind(i32::try_from(position).map_err(|_|StorageError::Database)?).bind(&memory.id).bind(&memory.title).execute(&mut *tx).await?;
        }
        tx.commit().await?;
        Ok(())
    }
}

impl Chats {
    /// The app comes from the owned chat, not from a caller override.
    pub async fn owned_chat(
        &self,
        user: &str,
        session: Uuid,
    ) -> Result<Option<Chat>, StorageError> {
        Ok(
            sqlx::query_as("SELECT * FROM chats WHERE session_id=$1 AND user_id=$2")
                .bind(session)
                .bind(user)
                .fetch_optional(&self.pool)
                .await?,
        )
    }
    pub async fn list_chats(&self, user: &str) -> Result<Vec<Chat>, StorageError> {
        Ok(sqlx::query_as("SELECT * FROM chats WHERE user_id=$1 ORDER BY is_pinned DESC,updated_at DESC,session_id DESC").bind(user).fetch_all(&self.pool).await?)
    }
    pub async fn running_chats(&self, user: &str) -> Result<Vec<Chat>, StorageError> {
        Ok(sqlx::query_as("SELECT c.* FROM chats c WHERE c.user_id=$1 AND EXISTS(SELECT 1 FROM chat_outputs o WHERE o.session_id=c.session_id AND o.status='running') ORDER BY c.is_pinned DESC,c.updated_at DESC,c.session_id DESC").bind(user).fetch_all(&self.pool).await?)
    }
    pub async fn page_inputs(
        &self,
        scope: &AppUser,
        session: Uuid,
        before: Option<Uuid>,
        limit: u32,
    ) -> Result<Vec<ChatInput>, StorageError> {
        if let Some(cursor) = before {
            let exists:bool=sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM chat_inputs i JOIN chats c USING(session_id) WHERE i.input_id=$1 AND i.session_id=$2 AND c.app_id=$3 AND c.user_id=$4)").bind(cursor).bind(session).bind(&scope.app_id).bind(&scope.user_id).fetch_one(&self.pool).await?;
            if !exists {
                return Err(StorageError::NotFound);
            }
        }
        Ok(sqlx::query_as("SELECT i.* FROM chat_inputs i JOIN chats c USING(session_id) WHERE i.session_id=$1 AND c.app_id=$2 AND c.user_id=$3 AND ($4::uuid IS NULL OR (i.created_at,i.input_id)<(SELECT b.created_at,b.input_id FROM chat_inputs b WHERE b.input_id=$4 AND b.session_id=$1)) ORDER BY i.created_at DESC,i.input_id DESC LIMIT $5").bind(session).bind(&scope.app_id).bind(&scope.user_id).bind(before).bind(i64::from(limit)).fetch_all(&self.pool).await?)
    }
    pub async fn output_for_input(
        &self,
        scope: &AppUser,
        input: Uuid,
    ) -> Result<Option<ChatOutput>, StorageError> {
        Ok(sqlx::query_as("SELECT o.* FROM chat_outputs o JOIN chats c USING(session_id) WHERE o.input_id=$1 AND c.app_id=$2 AND c.user_id=$3").bind(input).bind(&scope.app_id).bind(&scope.user_id).fetch_optional(&self.pool).await?)
    }
    pub async fn patch_chat(
        &self,
        scope: &AppUser,
        session: Uuid,
        patch: &crate::domain::models::chat_history::ChatPatch,
    ) -> Result<Chat, StorageError> {
        use crate::domain::models::chat_history::ChatPatch;
        let (title, pin) = match patch {
            ChatPatch::Title { title } => (Some(title), None),
            ChatPatch::Pin { is_pinned } => (None, Some(*is_pinned)),
        };
        sqlx::query_as("UPDATE chats SET title=COALESCE($4,title),is_pinned=COALESCE($5,is_pinned) WHERE session_id=$1 AND app_id=$2 AND user_id=$3 RETURNING *").bind(session).bind(&scope.app_id).bind(&scope.user_id).bind(title).bind(pin).fetch_optional(&self.pool).await?.ok_or(StorageError::NotFound)
    }
}
