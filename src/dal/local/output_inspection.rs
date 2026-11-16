use super::chats::Chats;
use crate::domain::models::{
    inspection::{Alternative, AnswerInspection, InspectionToken},
    storage::{AppUser, StorageError},
};
use uuid::Uuid;

#[derive(sqlx::FromRow)]
struct TokenRow {
    position: i32,
    start_byte: i32,
    end_byte: i32,
    bytes: Vec<u8>,
    logprob: f64,
}
#[derive(sqlx::FromRow)]
struct AlternativeRow {
    token_position: i32,
    bytes: Vec<u8>,
    logprob: f64,
}
impl Chats {
    /// The output must already exist. All child rows commit together.
    pub async fn save_inspection(
        &self,
        scope: &AppUser,
        output: Uuid,
        data: &AnswerInspection,
    ) -> Result<(), StorageError> {
        let mut transaction = self.pool.begin().await?;
        let text:Option<String>=sqlx::query_scalar("SELECT o.text FROM chat_outputs o JOIN chats c USING(session_id) WHERE o.output_id=$1 AND c.app_id=$2 AND c.user_id=$3")
            .bind(output).bind(&scope.app_id).bind(&scope.user_id).fetch_optional(&mut *transaction).await?;
        let text = text.ok_or(StorageError::NotFound)?;
        if !data.matches(&text) {
            return Err(StorageError::Database);
        }
        let changed=sqlx::query("INSERT INTO output_inspections(output_id,model,reason) VALUES($1,$2,$3) ON CONFLICT(output_id) DO NOTHING")
            .bind(output).bind(&data.model).bind(&data.reason).execute(&mut *transaction).await?.rows_affected();
        if changed == 0 {
            return Ok(());
        }
        if !data.tokens.is_empty() {
            let mut query = sqlx::QueryBuilder::new(
                "INSERT INTO output_inspection_tokens(output_id,position,start_byte,end_byte,bytes,logprob) ",
            );
            query.push_values(
                data.tokens.iter().enumerate(),
                |mut row, (position, token)| {
                    row.push_bind(output)
                        .push_bind(position as i32)
                        .push_bind(token.start_byte as i32)
                        .push_bind(token.end_byte as i32)
                        .push_bind(&token.bytes)
                        .push_bind(token.logprob);
                },
            );
            query.build().execute(&mut *transaction).await?;
        }
        let alternatives: Vec<_> = data
            .tokens
            .iter()
            .enumerate()
            .flat_map(|(position, token)| {
                token
                    .alternatives
                    .iter()
                    .enumerate()
                    .map(move |(rank, alt)| (position, rank, alt))
            })
            .collect();
        if !alternatives.is_empty() {
            let mut query = sqlx::QueryBuilder::new(
                "INSERT INTO output_inspection_alternatives(output_id,token_position,rank,bytes,logprob) ",
            );
            query.push_values(alternatives, |mut row, (position, rank, alt)| {
                row.push_bind(output)
                    .push_bind(position as i32)
                    .push_bind((rank + 1) as i16)
                    .push_bind(&alt.bytes)
                    .push_bind(alt.logprob);
            });
            query.build().execute(&mut *transaction).await?;
        }
        transaction.commit().await?;
        Ok(())
    }
    pub async fn inspection(
        &self,
        scope: &AppUser,
        output: Uuid,
    ) -> Result<AnswerInspection, StorageError> {
        if self.output(scope, output).await?.is_none() {
            return Err(StorageError::NotFound);
        }
        let metadata: Option<(String, Option<String>)> =
            sqlx::query_as("SELECT model,reason FROM output_inspections WHERE output_id=$1")
                .bind(output)
                .fetch_optional(&self.pool)
                .await?;
        let Some((model, reason)) = metadata else {
            return Ok(AnswerInspection::unavailable("", "not_recorded"));
        };
        let rows:Vec<TokenRow>=sqlx::query_as("SELECT position,start_byte,end_byte,bytes,logprob FROM output_inspection_tokens WHERE output_id=$1 ORDER BY position")
            .bind(output).fetch_all(&self.pool).await?;
        let alternatives:Vec<AlternativeRow>=sqlx::query_as("SELECT token_position,bytes,logprob FROM output_inspection_alternatives WHERE output_id=$1 ORDER BY token_position,rank")
            .bind(output).fetch_all(&self.pool).await?;
        let mut tokens: Vec<InspectionToken> = rows
            .iter()
            .map(|row| InspectionToken {
                start_byte: row.start_byte as usize,
                end_byte: row.end_byte as usize,
                bytes: row.bytes.clone(),
                logprob: row.logprob,
                alternatives: vec![],
            })
            .collect();
        for alt in alternatives {
            if let Some(token) = tokens.get_mut(alt.token_position as usize) {
                token.alternatives.push(Alternative {
                    bytes: alt.bytes,
                    logprob: alt.logprob,
                });
            }
        }
        debug_assert!(
            rows.iter()
                .enumerate()
                .all(|(i, t)| i == t.position as usize)
        );
        Ok(AnswerInspection {
            model,
            reason,
            tokens,
        })
    }
}
