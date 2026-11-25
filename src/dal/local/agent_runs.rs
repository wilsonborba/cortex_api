use std::{future::Future, pin::Pin};

use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use uuid::Uuid;

use crate::domain::services::agent_runs::{
    AgentRunStore, ChildEventKind, ChildRun, ChildRunError, ChildRunEvent, ChildRunStatus,
    OutputAction, PendingChildAction,
};

#[derive(Clone)]
pub struct PostgresAgentRunStore {
    pool: PgPool,
}
impl PostgresAgentRunStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    fn child_from_row(row: &sqlx::postgres::PgRow) -> Result<ChildRun, ChildRunError> {
        let status: String = row
            .try_get("status")
            .map_err(|_| ChildRunError::Unavailable)?;
        Ok(ChildRun {
            session_id: row
                .try_get("session_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            child_id: row
                .try_get("child_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            parent_run_id: row
                .try_get("parent_run_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            parent_chat_session_id: row
                .try_get("parent_chat_session_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            parent_output_id: row
                .try_get("parent_output_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            parent_call_id: row
                .try_get("parent_call_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            child_chat_session_id: row
                .try_get("child_chat_session_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            child_output_id: row
                .try_get("child_output_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            role: row
                .try_get("role")
                .map_err(|_| ChildRunError::Unavailable)?,
            task: row
                .try_get("task")
                .map_err(|_| ChildRunError::Unavailable)?,
            budget_tokens: u32::try_from(
                row.try_get::<i32, _>("budget_tokens")
                    .map_err(|_| ChildRunError::Unavailable)?,
            )
            .map_err(|_| ChildRunError::Unavailable)?,
            depth: u8::try_from(
                row.try_get::<i16, _>("depth")
                    .map_err(|_| ChildRunError::Unavailable)?,
            )
            .map_err(|_| ChildRunError::Unavailable)?,
            status: match status.as_str() {
                "queued" => ChildRunStatus::Queued,
                "running" => ChildRunStatus::Running,
                "completed" => ChildRunStatus::Completed,
                "failed" => ChildRunStatus::Failed,
                "cancelled" => ChildRunStatus::Cancelled,
                _ => return Err(ChildRunError::Unavailable),
            },
            cancel_requested: row
                .try_get::<Option<DateTime<Utc>>, _>("cancel_requested_at")
                .map_err(|_| ChildRunError::Unavailable)?
                .is_some(),
            budget_used_tokens: u64::try_from(
                row.try_get::<i64, _>("budget_used_tokens")
                    .map_err(|_| ChildRunError::Unavailable)?,
            )
            .map_err(|_| ChildRunError::Unavailable)?,
            budget_reserved_tokens: u64::try_from(
                row.try_get::<i64, _>("budget_reserved_tokens")
                    .map_err(|_| ChildRunError::Unavailable)?,
            )
            .map_err(|_| ChildRunError::Unavailable)?,
            budget_settled: row
                .try_get("budget_settled")
                .map_err(|_| ChildRunError::Unavailable)?,
            created_at: row
                .try_get("created_at")
                .map_err(|_| ChildRunError::Unavailable)?,
            updated_at: row
                .try_get("updated_at")
                .map_err(|_| ChildRunError::Unavailable)?,
        })
    }

    async fn owner_check(&self, session: Uuid, owner: &str) -> Result<(), ChildRunError> {
        let found: Option<String> =
            sqlx::query_scalar("SELECT owner_user FROM remote_cli_sessions WHERE session_id = $1")
                .bind(session)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
        match found.as_deref() {
            None => Err(ChildRunError::NotFound),
            Some(found) if found != owner => Err(ChildRunError::Forbidden),
            Some(_) => Ok(()),
        }
    }

    fn event(row: &sqlx::postgres::PgRow) -> Result<ChildRunEvent, ChildRunError> {
        let kind: String = row
            .try_get("kind")
            .map_err(|_| ChildRunError::Unavailable)?;
        let kind = match kind.as_str() {
            "queued" => ChildEventKind::Queued,
            "progress" => ChildEventKind::Progress,
            "completed" => ChildEventKind::Completed,
            "failed" => ChildEventKind::Failed,
            "cancel_requested" => ChildEventKind::CancelRequested,
            "cancelled" => ChildEventKind::Cancelled,
            "action_requested" => ChildEventKind::ActionRequested,
            "action_answered" => ChildEventKind::ActionAnswered,
            _ => return Err(ChildRunError::Unavailable),
        };
        Ok(ChildRunEvent {
            seq: row
                .try_get::<i64, _>("seq")
                .map_err(|_| ChildRunError::Unavailable)?
                .try_into()
                .map_err(|_| ChildRunError::Unavailable)?,
            event_id: row
                .try_get("event_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            kind,
            message: row
                .try_get("message")
                .map_err(|_| ChildRunError::Unavailable)?,
            result: row
                .try_get("result")
                .map_err(|_| ChildRunError::Unavailable)?,
            error_code: row
                .try_get("error_code")
                .map_err(|_| ChildRunError::Unavailable)?,
            action_id: row
                .try_get("action_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            action_kind: row
                .try_get("action_kind")
                .map_err(|_| ChildRunError::Unavailable)?,
            payload_digest: row
                .try_get("payload_digest")
                .map_err(|_| ChildRunError::Unavailable)?,
            display: row
                .try_get("display_text")
                .map_err(|_| ChildRunError::Unavailable)?,
            approved: row
                .try_get("approved")
                .map_err(|_| ChildRunError::Unavailable)?,
            created_at: row
                .try_get("created_at")
                .map_err(|_| ChildRunError::Unavailable)?,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn insert_event(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: Uuid,
        child: Uuid,
        event_id: Uuid,
        kind: ChildEventKind,
        message: Option<&str>,
        result: Option<&str>,
        error_code: Option<&str>,
        action_id: Option<Uuid>,
        action_kind: Option<&str>,
        digest: Option<&str>,
        display: Option<&str>,
        approved: Option<bool>,
        now: DateTime<Utc>,
    ) -> Result<ChildRunEvent, ChildRunError> {
        let seq: i64 = sqlx::query_scalar("SELECT next_event_seq FROM remote_cli_children WHERE session_id=$1 AND child_id=$2 FOR UPDATE")
            .bind(session).bind(child).fetch_optional(&mut **tx).await.map_err(|_| ChildRunError::Unavailable)?.ok_or(ChildRunError::NotFound)?;
        sqlx::query("INSERT INTO remote_cli_child_events(session_id,child_id,seq,event_id,kind,message,result,error_code,action_id,action_kind,payload_digest,display_text,approved,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14)")
            .bind(session).bind(child).bind(seq).bind(event_id).bind(kind.as_str()).bind(message).bind(result).bind(error_code).bind(action_id).bind(action_kind).bind(digest).bind(display).bind(approved).bind(now)
            .execute(&mut **tx).await.map_err(|e| if e.as_database_error().is_some_and(|db| db.is_unique_violation()) { ChildRunError::Conflict } else { ChildRunError::Unavailable })?;
        sqlx::query("UPDATE remote_cli_children SET next_event_seq=next_event_seq+1, updated_at=$3 WHERE session_id=$1 AND child_id=$2")
            .bind(session).bind(child).bind(now).execute(&mut **tx).await.map_err(|_| ChildRunError::Unavailable)?;
        Ok(ChildRunEvent {
            seq: seq as u64,
            event_id,
            kind,
            message: message.map(str::to_owned),
            result: result.map(str::to_owned),
            error_code: error_code.map(str::to_owned),
            action_id,
            action_kind: action_kind.map(str::to_owned),
            payload_digest: digest.map(str::to_owned),
            display: display.map(str::to_owned),
            approved,
            created_at: now,
        })
    }

    async fn run_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: Uuid,
        child: Uuid,
    ) -> Result<(ChildRunStatus, bool, String), ChildRunError> {
        let row = sqlx::query("SELECT status,cancel_requested_at,parent_run_id FROM remote_cli_children WHERE session_id=$1 AND child_id=$2 FOR UPDATE")
            .bind(session).bind(child).fetch_optional(&mut **tx).await.map_err(|_| ChildRunError::Unavailable)?.ok_or(ChildRunError::NotFound)?;
        let status: String = row
            .try_get("status")
            .map_err(|_| ChildRunError::Unavailable)?;
        let status = match status.as_str() {
            "queued" => ChildRunStatus::Queued,
            "running" => ChildRunStatus::Running,
            "completed" => ChildRunStatus::Completed,
            "failed" => ChildRunStatus::Failed,
            "cancelled" => ChildRunStatus::Cancelled,
            _ => return Err(ChildRunError::Unavailable),
        };
        let cancel: Option<DateTime<Utc>> = row
            .try_get("cancel_requested_at")
            .map_err(|_| ChildRunError::Unavailable)?;
        let parent: String = row
            .try_get("parent_run_id")
            .map_err(|_| ChildRunError::Unavailable)?;
        Ok((status, cancel.is_some(), parent))
    }

    async fn owner_in_tx(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session: Uuid,
        owner: &str,
    ) -> Result<(), ChildRunError> {
        let found: Option<String> = sqlx::query_scalar(
            "SELECT owner_user FROM remote_cli_sessions WHERE session_id=$1 FOR UPDATE",
        )
        .bind(session)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| ChildRunError::Unavailable)?;
        match found.as_deref() {
            None => Err(ChildRunError::NotFound),
            Some(found) if found != owner => Err(ChildRunError::Forbidden),
            Some(_) => Ok(()),
        }
    }

    fn output_action(row: &sqlx::postgres::PgRow) -> Result<OutputAction, ChildRunError> {
        Ok(OutputAction {
            action_id: row
                .try_get("action_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            chat_session_id: row
                .try_get("chat_session_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            output_id: row
                .try_get("output_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            call_id: row
                .try_get("call_id")
                .map_err(|_| ChildRunError::Unavailable)?,
            tool_name: row
                .try_get("tool_name")
                .map_err(|_| ChildRunError::Unavailable)?,
            action_kind: row
                .try_get("action_kind")
                .map_err(|_| ChildRunError::Unavailable)?,
            action_digest: row
                .try_get("action_digest")
                .map_err(|_| ChildRunError::Unavailable)?,
            display: row
                .try_get("display_text")
                .map_err(|_| ChildRunError::Unavailable)?,
            expires_at: row
                .try_get("expires_at")
                .map_err(|_| ChildRunError::Unavailable)?,
            approved: row
                .try_get("approved")
                .map_err(|_| ChildRunError::Unavailable)?,
            cancelled_at: row
                .try_get("cancelled_at")
                .map_err(|_| ChildRunError::Unavailable)?,
            claimed_at: row
                .try_get("claimed_at")
                .map_err(|_| ChildRunError::Unavailable)?,
            created_at: row
                .try_get("created_at")
                .map_err(|_| ChildRunError::Unavailable)?,
        })
    }
}

impl AgentRunStore for PostgresAgentRunStore {
    fn child_for_output<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        chat_session_id: Uuid,
        output_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ChildRun>, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            self.owner_check(session_id, owner).await?;
            let row = sqlx::query("SELECT session_id,child_id,parent_run_id,parent_chat_session_id,parent_output_id,parent_call_id,child_chat_session_id,child_output_id,role,task,budget_tokens,depth,status,cancel_requested_at,budget_used_tokens,budget_reserved_tokens,budget_settled,created_at,updated_at FROM remote_cli_children WHERE session_id=$1 AND child_chat_session_id=$2 AND child_output_id=$3")
                .bind(session_id).bind(chat_session_id).bind(output_id).fetch_optional(&self.pool).await.map_err(|_|ChildRunError::Unavailable)?;
            row.as_ref().map(Self::child_from_row).transpose()
        })
    }

    fn create_bound_child<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        request: crate::domain::services::agent_runs::BoundChildRequest,
        max_active: usize,
        max_depth: u8,
        max_budget_tokens: u32,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRun, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            if request.role.trim().is_empty()
                || request.role.len() > 80
                || request.task.trim().is_empty()
                || request.task.len() > 32_000
                || request.budget_tokens == 0
                || request.budget_tokens > max_budget_tokens.max(1)
            {
                return Err(ChildRunError::Invalid);
            }
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            if let Some(row) = sqlx::query("SELECT session_id,child_id,parent_run_id,parent_chat_session_id,parent_output_id,parent_call_id,child_chat_session_id,child_output_id,role,task,budget_tokens,depth,status,cancel_requested_at,budget_used_tokens,budget_reserved_tokens,budget_settled,created_at,updated_at FROM remote_cli_children WHERE parent_output_id=$1 AND parent_call_id=$2 FOR UPDATE")
                .bind(request.parent_output_id).bind(request.parent_call_id).fetch_optional(&mut *tx).await.map_err(|_| ChildRunError::Unavailable)?
            {
                let same = row.try_get::<Uuid,_>("session_id").map_err(|_|ChildRunError::Unavailable)? == session_id
                    && row.try_get::<Option<Uuid>,_>("parent_chat_session_id").map_err(|_|ChildRunError::Unavailable)? == Some(request.parent_chat_session_id)
                    && row.try_get::<String,_>("role").map_err(|_|ChildRunError::Unavailable)? == request.role
                    && row.try_get::<String,_>("task").map_err(|_|ChildRunError::Unavailable)? == request.task;
                if !same { return Err(ChildRunError::Conflict); }
                let child = Self::child_from_row(&row)?;
                tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
                return Ok(child);
            }
            let active: i64 = sqlx::query_scalar("SELECT count(*) FROM remote_cli_children WHERE session_id=$1 AND status IN ('queued','running')")
                .bind(session_id).fetch_one(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            if active >= i64::try_from(max_active.max(1)).unwrap_or(i64::MAX) {
                return Err(ChildRunError::Conflict);
            }
            let parent = sqlx::query("SELECT child_id,budget_tokens,depth,status,cancel_requested_at,budget_used_tokens,budget_reserved_tokens FROM remote_cli_children WHERE session_id=$1 AND child_chat_session_id=$2 AND child_output_id=$3 FOR UPDATE")
                .bind(session_id).bind(request.parent_chat_session_id).bind(request.parent_output_id)
                .fetch_optional(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            let (parent_run_id, parent_id, depth, budget_tokens, parent_usage) =
                if let Some(parent) = parent {
                    let status: String = parent
                        .try_get("status")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    if matches!(status.as_str(), "completed" | "failed" | "cancelled")
                        || parent
                            .try_get::<Option<DateTime<Utc>>, _>("cancel_requested_at")
                            .map_err(|_| ChildRunError::Unavailable)?
                            .is_some()
                    {
                        return Err(ChildRunError::Conflict);
                    }
                    let parent_budget = u64::try_from(
                        parent
                            .try_get::<i32, _>("budget_tokens")
                            .map_err(|_| ChildRunError::Unavailable)?,
                    )
                    .map_err(|_| ChildRunError::Unavailable)?;
                    let used: u64 = u64::try_from(
                        parent
                            .try_get::<i64, _>("budget_used_tokens")
                            .map_err(|_| ChildRunError::Unavailable)?,
                    )
                    .map_err(|_| ChildRunError::Unavailable)?;
                    let reserved: u64 = u64::try_from(
                        parent
                            .try_get::<i64, _>("budget_reserved_tokens")
                            .map_err(|_| ChildRunError::Unavailable)?,
                    )
                    .map_err(|_| ChildRunError::Unavailable)?;
                    let parent_usage = used.max(request.parent_budget_used_tokens);
                    let remaining = parent_budget
                        .saturating_sub(parent_usage)
                        .saturating_sub(reserved);
                    let budget = request
                        .budget_tokens
                        .min(u32::try_from(remaining).unwrap_or(u32::MAX));
                    let depth = u8::try_from(
                        parent
                            .try_get::<i16, _>("depth")
                            .map_err(|_| ChildRunError::Unavailable)?,
                    )
                    .map_err(|_| ChildRunError::Unavailable)?
                    .saturating_add(1);
                    if budget == 0 || depth > max_depth.max(1) {
                        return Err(ChildRunError::Conflict);
                    }
                    let parent_id = parent
                        .try_get::<Uuid, _>("child_id")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    (
                        parent_id.to_string(),
                        Some(parent_id),
                        depth,
                        budget,
                        parent_usage,
                    )
                } else {
                    (session_id.to_string(), None, 1, request.budget_tokens, 0)
                };
            let child_id = Uuid::now_v7();
            let depth_db = depth;
            let budget_db = i32::try_from(budget_tokens).map_err(|_| ChildRunError::Invalid)?;
            let result = sqlx::query("INSERT INTO remote_cli_children(session_id,child_id,parent_run_id,parent_chat_session_id,parent_output_id,parent_call_id,role,task,budget_tokens,depth,status,next_event_seq,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,'queued',1,$11,$11) ON CONFLICT DO NOTHING RETURNING child_id")
                .bind(session_id).bind(child_id).bind(&parent_run_id).bind(request.parent_chat_session_id).bind(request.parent_output_id).bind(request.parent_call_id).bind(&request.role).bind(&request.task).bind(budget_db).bind(i16::from(depth_db)).bind(now)
                .fetch_optional(&mut *tx).await.map_err(|e| if e.as_database_error().is_some_and(|db| db.is_unique_violation()) { ChildRunError::Conflict } else { ChildRunError::Unavailable })?;
            if result.is_none() {
                return Err(ChildRunError::Conflict);
            }
            if let Some(parent_id) = parent_id {
                let updated = sqlx::query("UPDATE remote_cli_children SET budget_used_tokens=GREATEST(budget_used_tokens,$3),budget_reserved_tokens=budget_reserved_tokens+$4,updated_at=$5 WHERE session_id=$1 AND child_id=$2 AND GREATEST(budget_used_tokens,$3)+budget_reserved_tokens+$4<=budget_tokens")
                    .bind(session_id).bind(parent_id).bind(i64::try_from(parent_usage).map_err(|_| ChildRunError::Invalid)?).bind(i64::from(budget_db)).bind(now).execute(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
                if updated.rows_affected() != 1 {
                    return Err(ChildRunError::NotFound);
                }
            }
            Self::insert_event(
                &mut tx,
                session_id,
                child_id,
                Uuid::now_v7(),
                ChildEventKind::Queued,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                now,
            )
            .await?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Ok(ChildRun {
                session_id,
                child_id,
                parent_run_id,
                parent_chat_session_id: Some(request.parent_chat_session_id),
                parent_output_id: Some(request.parent_output_id),
                parent_call_id: Some(request.parent_call_id),
                child_chat_session_id: None,
                child_output_id: None,
                role: request.role,
                task: request.task,
                budget_tokens,
                depth: depth_db,
                status: ChildRunStatus::Queued,
                cancel_requested: false,
                budget_used_tokens: 0,
                budget_reserved_tokens: 0,
                budget_settled: false,
                created_at: now,
                updated_at: now,
            })
        })
    }

    fn bind_child_output<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        child_chat_session_id: Uuid,
        child_output_id: Uuid,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRun, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let row = sqlx::query("UPDATE remote_cli_children SET child_chat_session_id=$3,child_output_id=$4,updated_at=$5 WHERE session_id=$1 AND child_id=$2 AND child_chat_session_id IS NULL AND child_output_id IS NULL AND EXISTS(SELECT 1 FROM chats WHERE session_id=$3 AND user_id=$6 AND app_id='cortex') AND EXISTS(SELECT 1 FROM chat_outputs WHERE output_id=$4 AND session_id=$3) RETURNING session_id,child_id,parent_run_id,parent_chat_session_id,parent_output_id,parent_call_id,child_chat_session_id,child_output_id,role,task,budget_tokens,depth,status,cancel_requested_at,budget_used_tokens,budget_reserved_tokens,budget_settled,created_at,updated_at")
                .bind(session_id).bind(child_id).bind(child_chat_session_id).bind(child_output_id).bind(now).bind(owner).fetch_optional(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            let row = match row {
                Some(row) => row,
                None => {
                    let existing = sqlx::query("SELECT session_id,child_id,parent_run_id,parent_chat_session_id,parent_output_id,parent_call_id,child_chat_session_id,child_output_id,role,task,budget_tokens,depth,status,cancel_requested_at,budget_used_tokens,budget_reserved_tokens,budget_settled,created_at,updated_at FROM remote_cli_children WHERE session_id=$1 AND child_id=$2")
                        .bind(session_id).bind(child_id).fetch_optional(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?.ok_or(ChildRunError::NotFound)?;
                    if existing
                        .try_get::<Option<Uuid>, _>("child_chat_session_id")
                        .map_err(|_| ChildRunError::Unavailable)?
                        != Some(child_chat_session_id)
                        || existing
                            .try_get::<Option<Uuid>, _>("child_output_id")
                            .map_err(|_| ChildRunError::Unavailable)?
                            != Some(child_output_id)
                    {
                        return Err(ChildRunError::Conflict);
                    }
                    existing
                }
            };
            let child = Self::child_from_row(&row)?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Ok(child)
        })
    }

    fn report_child_usage<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        used_tokens: u64,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let used = i64::try_from(used_tokens).map_err(|_| ChildRunError::Invalid)?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let result = sqlx::query("UPDATE remote_cli_children SET budget_used_tokens=$3,updated_at=$4 WHERE session_id=$1 AND child_id=$2 AND budget_settled=false AND budget_used_tokens<=$3 AND $3<=budget_tokens-budget_reserved_tokens")
                .bind(session_id).bind(child_id).bind(used).bind(now).execute(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            if result.rows_affected() != 1 {
                return Err(ChildRunError::Conflict);
            }
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)
        })
    }

    fn settle_child_budget<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        used_tokens: u64,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let used = i64::try_from(used_tokens).map_err(|_| ChildRunError::Invalid)?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let row = sqlx::query("SELECT parent_chat_session_id,parent_output_id,budget_tokens,budget_used_tokens,budget_reserved_tokens,budget_settled FROM remote_cli_children WHERE session_id=$1 AND child_id=$2 FOR UPDATE")
                .bind(session_id).bind(child_id).fetch_optional(&mut *tx).await.map_err(|_| ChildRunError::Unavailable)?.ok_or(ChildRunError::NotFound)?;
            let settled: bool = row
                .try_get("budget_settled")
                .map_err(|_| ChildRunError::Unavailable)?;
            let previous: i64 = row
                .try_get("budget_used_tokens")
                .map_err(|_| ChildRunError::Unavailable)?;
            let budget: i32 = row
                .try_get("budget_tokens")
                .map_err(|_| ChildRunError::Unavailable)?;
            let reserved: i64 = row
                .try_get("budget_reserved_tokens")
                .map_err(|_| ChildRunError::Unavailable)?;
            if settled {
                if previous != used {
                    return Err(ChildRunError::Conflict);
                }
                tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
                return Ok(());
            }
            if used < previous || used > i64::from(budget) {
                return Err(ChildRunError::Conflict);
            }
            let parent_chat: Option<Uuid> = row
                .try_get("parent_chat_session_id")
                .map_err(|_| ChildRunError::Unavailable)?;
            let parent_output: Option<Uuid> = row
                .try_get("parent_output_id")
                .map_err(|_| ChildRunError::Unavailable)?;
            sqlx::query("UPDATE remote_cli_children SET budget_used_tokens=$3,budget_reserved_tokens=0,budget_settled=true,updated_at=$4 WHERE session_id=$1 AND child_id=$2")
                .bind(session_id).bind(child_id).bind(used).bind(now).execute(&mut *tx).await.map_err(|_| ChildRunError::Unavailable)?;
            if let (Some(chat), Some(output)) = (parent_chat, parent_output) {
                let parent = sqlx::query("SELECT child_id,budget_tokens,budget_used_tokens,budget_reserved_tokens FROM remote_cli_children WHERE session_id=$1 AND child_chat_session_id=$2 AND child_output_id=$3 FOR UPDATE")
                    .bind(session_id).bind(chat).bind(output).fetch_optional(&mut *tx).await.map_err(|_| ChildRunError::Unavailable)?;
                if let Some(parent) = parent {
                    let parent_id: Uuid = parent
                        .try_get("child_id")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    let parent_used: i64 = parent
                        .try_get("budget_used_tokens")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    let parent_reserved: i64 = parent
                        .try_get("budget_reserved_tokens")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    if parent_reserved < i64::from(budget)
                        || parent_used
                            .saturating_add(used)
                            .saturating_add(parent_reserved - i64::from(budget))
                            > i64::from(
                                parent
                                    .try_get::<i32, _>("budget_tokens")
                                    .map_err(|_| ChildRunError::Unavailable)?,
                            )
                    {
                        return Err(ChildRunError::Conflict);
                    }
                    sqlx::query("UPDATE remote_cli_children SET budget_used_tokens=budget_used_tokens+$3,budget_reserved_tokens=budget_reserved_tokens-$4,updated_at=$5 WHERE session_id=$1 AND child_id=$2")
                        .bind(session_id).bind(parent_id).bind(used).bind(i64::from(budget)).bind(now).execute(&mut *tx).await.map_err(|_| ChildRunError::Unavailable)?;
                }
            }
            let _ = reserved;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)
        })
    }

    fn create_child<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        parent_run_id: &'a str,
        role: &'a str,
        task: &'a str,
        budget_tokens: u32,
        max_active: usize,
        max_depth: u8,
        max_budget_tokens: u32,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRun, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            if budget_tokens == 0 || budget_tokens > max_budget_tokens.max(1) {
                return Err(ChildRunError::Invalid);
            }
            let active:i64=sqlx::query_scalar("SELECT count(*) FROM remote_cli_children WHERE session_id=$1 AND status IN ('queued','running')").bind(session_id).fetch_one(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            if active >= i64::try_from(max_active.max(1)).unwrap_or(i64::MAX) {
                return Err(ChildRunError::Conflict);
            }
            let depth = if parent_run_id != session_id.to_string() {
                let parent_id =
                    Uuid::parse_str(parent_run_id).map_err(|_| ChildRunError::Invalid)?;
                let parent=sqlx::query("SELECT depth,budget_tokens,status,cancel_requested_at FROM remote_cli_children WHERE session_id=$1 AND child_id=$2 FOR UPDATE").bind(session_id).bind(parent_id).fetch_optional(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?.ok_or(ChildRunError::NotFound)?;
                let status: String = parent
                    .try_get("status")
                    .map_err(|_| ChildRunError::Unavailable)?;
                let cancel: Option<DateTime<Utc>> = parent
                    .try_get("cancel_requested_at")
                    .map_err(|_| ChildRunError::Unavailable)?;
                if matches!(status.as_str(), "completed" | "failed" | "cancelled")
                    || cancel.is_some()
                {
                    return Err(ChildRunError::Conflict);
                }
                let parent_budget = u32::try_from(
                    parent
                        .try_get::<i32, _>("budget_tokens")
                        .map_err(|_| ChildRunError::Unavailable)?,
                )
                .map_err(|_| ChildRunError::Unavailable)?;
                if budget_tokens > parent_budget {
                    return Err(ChildRunError::Invalid);
                }
                let allocated:i64=sqlx::query_scalar("SELECT COALESCE(SUM(budget_tokens),0) FROM remote_cli_children WHERE session_id=$1 AND parent_run_id=$2").bind(session_id).bind(parent_run_id).fetch_one(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
                if allocated.saturating_add(i64::from(budget_tokens)) > i64::from(parent_budget) {
                    return Err(ChildRunError::Conflict);
                }
                u8::try_from(
                    parent
                        .try_get::<i16, _>("depth")
                        .map_err(|_| ChildRunError::Unavailable)?,
                )
                .map_err(|_| ChildRunError::Unavailable)?
                .saturating_add(1)
            } else {
                1
            };
            if depth > max_depth.max(1) {
                return Err(ChildRunError::Conflict);
            }
            let child_id = Uuid::now_v7();
            sqlx::query("INSERT INTO remote_cli_children(session_id,child_id,parent_run_id,role,task,budget_tokens,depth,status,next_event_seq,created_at,updated_at) VALUES($1,$2,$3,$4,$5,$6,$7,'queued',1,$8,$8)")
                .bind(session_id).bind(child_id).bind(parent_run_id).bind(role).bind(task).bind(i32::try_from(budget_tokens).map_err(|_|ChildRunError::Invalid)?).bind(i16::from(depth)).bind(now).execute(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            Self::insert_event(
                &mut tx,
                session_id,
                child_id,
                Uuid::now_v7(),
                ChildEventKind::Queued,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                None,
                now,
            )
            .await?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Ok(ChildRun {
                session_id,
                child_id,
                parent_run_id: parent_run_id.into(),
                parent_chat_session_id: None,
                parent_output_id: None,
                parent_call_id: None,
                child_chat_session_id: None,
                child_output_id: None,
                role: role.into(),
                task: task.into(),
                budget_tokens,
                depth,
                status: ChildRunStatus::Queued,
                cancel_requested: false,
                budget_used_tokens: 0,
                budget_reserved_tokens: 0,
                budget_settled: false,
                created_at: now,
                updated_at: now,
            })
        })
    }
    fn append_event<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        event_id: Uuid,
        kind: ChildEventKind,
        message: Option<&'a str>,
        result: Option<&'a str>,
        error_code: Option<&'a str>,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            self.owner_check(session_id, owner).await?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            let (status, cancelled, _) = Self::run_tx(&mut tx, session_id, child_id).await?;
            if let Some(row)=sqlx::query("SELECT * FROM remote_cli_child_events WHERE session_id=$1 AND child_id=$2 AND event_id=$3").bind(session_id).bind(child_id).bind(event_id).fetch_optional(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)? {
                let old=Self::event(&row)?;
                if old.kind==kind&&old.message.as_deref()==message&&old.result.as_deref()==result&&old.error_code.as_deref()==error_code { tx.commit().await.map_err(|_|ChildRunError::Unavailable)?; return Ok(old); }
                return Err(ChildRunError::Conflict);
            }
            if status.terminal() {
                return Err(ChildRunError::Conflict);
            }
            let new_status = match kind {
                ChildEventKind::Progress if !cancelled => "running",
                ChildEventKind::Completed if !cancelled => "completed",
                ChildEventKind::Failed => "failed",
                ChildEventKind::Cancelled if cancelled => "cancelled",
                _ => return Err(ChildRunError::Conflict),
            };
            let ev = Self::insert_event(
                &mut tx, session_id, child_id, event_id, kind, message, result, error_code, None,
                None, None, None, None, now,
            )
            .await?;
            sqlx::query(
                "UPDATE remote_cli_children SET status=$3 WHERE session_id=$1 AND child_id=$2",
            )
            .bind(session_id)
            .bind(child_id)
            .bind(new_status)
            .execute(&mut *tx)
            .await
            .map_err(|_| ChildRunError::Unavailable)?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Ok(ev)
        })
    }
    fn request_cancel<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        event_id: Uuid,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let (root_status, cancelled, _) = Self::run_tx(&mut tx, session_id, child_id).await?;
            if root_status.terminal() {
                return Err(ChildRunError::Conflict);
            }
            let ids:Vec<Uuid>=sqlx::query_scalar("WITH RECURSIVE child_tree(child_id) AS (SELECT $2::uuid UNION SELECT child.child_id FROM remote_cli_children child JOIN child_tree parent ON child.parent_run_id=parent.child_id::text WHERE child.session_id=$1) SELECT child_id FROM child_tree ORDER BY child_id")
                .bind(session_id).bind(child_id).fetch_all(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            let mut root_event = if cancelled {
                let row=sqlx::query("SELECT * FROM remote_cli_child_events WHERE session_id=$1 AND child_id=$2 AND kind='cancel_requested' ORDER BY seq LIMIT 1").bind(session_id).bind(child_id).fetch_optional(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
                row.as_ref().map(Self::event).transpose()?
            } else {
                None
            };
            for id in ids {
                let (status, already_cancelled, _) = Self::run_tx(&mut tx, session_id, id).await?;
                if status.terminal() || already_cancelled {
                    continue;
                }
                let actions=sqlx::query("UPDATE remote_cli_child_actions SET approved=false,answered_at=$3 WHERE session_id=$1 AND child_id=$2 AND approved IS NULL RETURNING action_id,action_kind,payload_digest,display_text")
                    .bind(session_id).bind(id).bind(now).fetch_all(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
                for action in actions {
                    let action_id: Uuid = action
                        .try_get("action_id")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    let action_kind: String = action
                        .try_get("action_kind")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    let digest: String = action
                        .try_get("payload_digest")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    let display: String = action
                        .try_get("display_text")
                        .map_err(|_| ChildRunError::Unavailable)?;
                    Self::insert_event(
                        &mut tx,
                        session_id,
                        id,
                        Uuid::now_v7(),
                        ChildEventKind::ActionAnswered,
                        None,
                        None,
                        None,
                        Some(action_id),
                        Some(&action_kind),
                        Some(&digest),
                        Some(&display),
                        Some(false),
                        now,
                    )
                    .await?;
                }
                let ev = Self::insert_event(
                    &mut tx,
                    session_id,
                    id,
                    if id == child_id {
                        event_id
                    } else {
                        Uuid::now_v7()
                    },
                    ChildEventKind::CancelRequested,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    None,
                    now,
                )
                .await?;
                sqlx::query("UPDATE remote_cli_children SET cancel_requested_at=$3 WHERE session_id=$1 AND child_id=$2").bind(session_id).bind(id).bind(now).execute(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
                if id == child_id {
                    root_event = Some(ev);
                }
            }
            let ev = root_event.ok_or(ChildRunError::Conflict)?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Ok(ev)
        })
    }
    fn create_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        action: PendingChildAction,
        now: DateTime<Utc>,
        event_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let (status, cancelled, parent) = Self::run_tx(&mut tx, session_id, child_id).await?;
            if status.terminal()
                || cancelled
                || parent != action.parent_run_id
                || action.expires_at <= now
            {
                return Err(ChildRunError::Conflict);
            }
            sqlx::query("INSERT INTO remote_cli_child_actions(session_id,child_id,action_id,parent_run_id,action_kind,payload_digest,display_text,expires_at,created_at) VALUES($1,$2,$3,$4,$5,$6,$7,$8,$9)")
                .bind(session_id).bind(child_id).bind(action.action_id).bind(&action.parent_run_id).bind(&action.action_kind).bind(&action.payload_digest).bind(&action.display).bind(action.expires_at).bind(now).execute(&mut *tx).await.map_err(|e|if e.as_database_error().is_some_and(|d|d.is_unique_violation()){ChildRunError::Conflict}else{ChildRunError::Unavailable})?;
            let ev = Self::insert_event(
                &mut tx,
                session_id,
                child_id,
                event_id,
                ChildEventKind::ActionRequested,
                None,
                None,
                None,
                Some(action.action_id),
                Some(&action.action_kind),
                Some(&action.payload_digest),
                Some(&action.display),
                None,
                now,
            )
            .await?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Ok(ev)
        })
    }
    fn answer_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        action_id: Uuid,
        approved: bool,
        digest: &'a str,
        event_id: Uuid,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let (status, cancelled, _) = Self::run_tx(&mut tx, session_id, child_id).await?;
            if status.terminal() || cancelled {
                return Err(ChildRunError::Conflict);
            }
            let row=sqlx::query("SELECT action_kind,payload_digest,expires_at,approved FROM remote_cli_child_actions WHERE session_id=$1 AND child_id=$2 AND action_id=$3 FOR UPDATE").bind(session_id).bind(child_id).bind(action_id).fetch_optional(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?.ok_or(ChildRunError::NotFound)?;
            let action_kind: String = row
                .try_get("action_kind")
                .map_err(|_| ChildRunError::Unavailable)?;
            let stored_digest: String = row
                .try_get("payload_digest")
                .map_err(|_| ChildRunError::Unavailable)?;
            let expiry: DateTime<Utc> = row
                .try_get("expires_at")
                .map_err(|_| ChildRunError::Unavailable)?;
            let decision: Option<bool> = row
                .try_get("approved")
                .map_err(|_| ChildRunError::Unavailable)?;
            if decision.is_some() || expiry <= now || stored_digest != digest {
                return Err(ChildRunError::Conflict);
            }
            sqlx::query("UPDATE remote_cli_child_actions SET approved=$4,answered_at=$5 WHERE session_id=$1 AND child_id=$2 AND action_id=$3 AND approved IS NULL").bind(session_id).bind(child_id).bind(action_id).bind(approved).bind(now).execute(&mut *tx).await.map_err(|_|ChildRunError::Unavailable)?;
            let ev = Self::insert_event(
                &mut tx,
                session_id,
                child_id,
                event_id,
                ChildEventKind::ActionAnswered,
                None,
                None,
                None,
                Some(action_id),
                Some(&action_kind),
                Some(&stored_digest),
                None,
                Some(approved),
                now,
            )
            .await?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Ok(ev)
        })
    }
    fn create_output_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        action: OutputAction,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            self.owner_check(session_id, owner).await?;
            if action.expires_at <= now {
                return Err(ChildRunError::Conflict);
            }
            let row = sqlx::query("INSERT INTO remote_cli_output_actions AS existing (session_id, action_id, chat_session_id, output_id, call_id, tool_name, action_kind, action_digest, display_text, expires_at, created_at) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11) ON CONFLICT (output_id, call_id) DO UPDATE SET action_id=existing.action_id WHERE existing.session_id=EXCLUDED.session_id AND existing.chat_session_id=EXCLUDED.chat_session_id AND existing.tool_name=EXCLUDED.tool_name AND existing.action_kind=EXCLUDED.action_kind AND existing.action_digest=EXCLUDED.action_digest AND existing.display_text=EXCLUDED.display_text RETURNING *")
                .bind(session_id).bind(action.action_id).bind(action.chat_session_id)
                .bind(action.output_id).bind(action.call_id).bind(&action.tool_name)
                .bind(&action.action_kind).bind(&action.action_digest).bind(&action.display)
                .bind(action.expires_at).bind(now).fetch_optional(&self.pool).await
                .map_err(|e| if e.as_database_error().is_some_and(|db| db.is_unique_violation()) {
                    ChildRunError::Conflict
                } else { ChildRunError::Unavailable })?;
            let row = row.ok_or(ChildRunError::Conflict)?;
            Self::output_action(&row)
        })
    }
    fn output_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        chat_session_id: Uuid,
        output_id: Uuid,
        action_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            self.owner_check(session_id, owner).await?;
            let row = sqlx::query("SELECT * FROM remote_cli_output_actions WHERE session_id=$1 AND chat_session_id=$2 AND output_id=$3 AND action_id=$4")
                .bind(session_id).bind(chat_session_id).bind(output_id).bind(action_id)
                .fetch_optional(&self.pool).await.map_err(|_| ChildRunError::Unavailable)?
                .ok_or(ChildRunError::NotFound)?;
            Self::output_action(&row)
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn answer_output_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        chat_session_id: Uuid,
        output_id: Uuid,
        action_id: Uuid,
        approved: bool,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let row = sqlx::query("SELECT approved, cancelled_at, claimed_at, expires_at FROM remote_cli_output_actions WHERE session_id=$1 AND chat_session_id=$2 AND output_id=$3 AND action_id=$4 FOR UPDATE")
                .bind(session_id).bind(chat_session_id).bind(output_id).bind(action_id)
                .fetch_optional(&mut *tx).await.map_err(|_| ChildRunError::Unavailable)?
                .ok_or(ChildRunError::NotFound)?;
            let existing: Option<bool> = row
                .try_get("approved")
                .map_err(|_| ChildRunError::Unavailable)?;
            let cancelled: Option<DateTime<Utc>> = row
                .try_get("cancelled_at")
                .map_err(|_| ChildRunError::Unavailable)?;
            let claimed: Option<DateTime<Utc>> = row
                .try_get("claimed_at")
                .map_err(|_| ChildRunError::Unavailable)?;
            let expires: DateTime<Utc> = row
                .try_get("expires_at")
                .map_err(|_| ChildRunError::Unavailable)?;
            if existing.is_some() || cancelled.is_some() || claimed.is_some() || expires <= now {
                return Err(ChildRunError::Conflict);
            }
            let row = sqlx::query("UPDATE remote_cli_output_actions SET approved=$5,answered_at=$6 WHERE session_id=$1 AND chat_session_id=$2 AND output_id=$3 AND action_id=$4 AND approved IS NULL AND cancelled_at IS NULL AND claimed_at IS NULL AND expires_at>$6 RETURNING *")
                .bind(session_id).bind(chat_session_id).bind(output_id).bind(action_id).bind(approved).bind(now)
                .fetch_optional(&mut *tx).await.map_err(|_| ChildRunError::Unavailable)?
                .ok_or(ChildRunError::Conflict)?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Self::output_action(&row)
        })
    }
    #[allow(clippy::too_many_arguments)]
    fn claim_output_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        chat_session_id: Uuid,
        output_id: Uuid,
        action_id: Uuid,
        call_id: Uuid,
        digest: &'a str,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| ChildRunError::Unavailable)?;
            Self::owner_in_tx(&mut tx, session_id, owner).await?;
            let row = sqlx::query("UPDATE remote_cli_output_actions SET claimed_at=$7 WHERE session_id=$1 AND chat_session_id=$2 AND output_id=$3 AND action_id=$4 AND call_id=$5 AND action_digest=$6 AND approved IS TRUE AND cancelled_at IS NULL AND claimed_at IS NULL AND expires_at>$7 RETURNING *")
                .bind(session_id).bind(chat_session_id).bind(output_id).bind(action_id)
                .bind(call_id).bind(digest).bind(now).fetch_optional(&mut *tx).await
                .map_err(|_| ChildRunError::Unavailable)?.ok_or(ChildRunError::Conflict)?;
            tx.commit().await.map_err(|_| ChildRunError::Unavailable)?;
            Self::output_action(&row)
        })
    }
    fn cancel_output_actions<'a>(
        &'a self,
        owner: &'a str,
        chat_session_id: Uuid,
        output_id: Uuid,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            sqlx::query("UPDATE remote_cli_output_actions a SET cancelled_at=$4 FROM remote_cli_sessions s WHERE a.session_id=s.session_id AND s.owner_user=$1 AND a.chat_session_id=$2 AND a.output_id=$3 AND a.claimed_at IS NULL AND a.cancelled_at IS NULL")
                .bind(owner).bind(chat_session_id).bind(output_id).bind(now).execute(&self.pool).await
                .map_err(|_| ChildRunError::Unavailable)?;
            Ok(())
        })
    }
    fn events_after<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        after: u64,
        limit: usize,
    ) -> Pin<
        Box<dyn Future<Output = Result<(ChildRun, Vec<ChildRunEvent>), ChildRunError>> + Send + 'a>,
    > {
        Box::pin(async move {
            self.owner_check(session_id, owner).await?;
            let row=sqlx::query("SELECT parent_run_id,parent_chat_session_id,parent_output_id,parent_call_id,child_chat_session_id,child_output_id,budget_used_tokens,budget_reserved_tokens,budget_settled,role,task,budget_tokens,depth,status,cancel_requested_at,created_at,updated_at FROM remote_cli_children WHERE session_id=$1 AND child_id=$2").bind(session_id).bind(child_id).fetch_optional(&self.pool).await.map_err(|_|ChildRunError::Unavailable)?.ok_or(ChildRunError::NotFound)?;
            let raw: String = row
                .try_get("status")
                .map_err(|_| ChildRunError::Unavailable)?;
            let status = match raw.as_str() {
                "queued" => ChildRunStatus::Queued,
                "running" => ChildRunStatus::Running,
                "completed" => ChildRunStatus::Completed,
                "failed" => ChildRunStatus::Failed,
                "cancelled" => ChildRunStatus::Cancelled,
                _ => return Err(ChildRunError::Unavailable),
            };
            let run = ChildRun {
                session_id,
                child_id,
                parent_run_id: row
                    .try_get("parent_run_id")
                    .map_err(|_| ChildRunError::Unavailable)?,
                parent_chat_session_id: row
                    .try_get("parent_chat_session_id")
                    .map_err(|_| ChildRunError::Unavailable)?,
                parent_output_id: row
                    .try_get("parent_output_id")
                    .map_err(|_| ChildRunError::Unavailable)?,
                parent_call_id: row
                    .try_get("parent_call_id")
                    .map_err(|_| ChildRunError::Unavailable)?,
                child_chat_session_id: row
                    .try_get("child_chat_session_id")
                    .map_err(|_| ChildRunError::Unavailable)?,
                child_output_id: row
                    .try_get("child_output_id")
                    .map_err(|_| ChildRunError::Unavailable)?,
                role: row
                    .try_get("role")
                    .map_err(|_| ChildRunError::Unavailable)?,
                task: row
                    .try_get("task")
                    .map_err(|_| ChildRunError::Unavailable)?,
                budget_tokens: u32::try_from(
                    row.try_get::<i32, _>("budget_tokens")
                        .map_err(|_| ChildRunError::Unavailable)?,
                )
                .map_err(|_| ChildRunError::Unavailable)?,
                depth: u8::try_from(
                    row.try_get::<i16, _>("depth")
                        .map_err(|_| ChildRunError::Unavailable)?,
                )
                .map_err(|_| ChildRunError::Unavailable)?,
                status,
                cancel_requested: row
                    .try_get::<Option<DateTime<Utc>>, _>("cancel_requested_at")
                    .map_err(|_| ChildRunError::Unavailable)?
                    .is_some(),
                budget_used_tokens: u64::try_from(
                    row.try_get::<i64, _>("budget_used_tokens")
                        .map_err(|_| ChildRunError::Unavailable)?,
                )
                .map_err(|_| ChildRunError::Unavailable)?,
                budget_reserved_tokens: u64::try_from(
                    row.try_get::<i64, _>("budget_reserved_tokens")
                        .map_err(|_| ChildRunError::Unavailable)?,
                )
                .map_err(|_| ChildRunError::Unavailable)?,
                budget_settled: row
                    .try_get("budget_settled")
                    .map_err(|_| ChildRunError::Unavailable)?,
                created_at: row
                    .try_get("created_at")
                    .map_err(|_| ChildRunError::Unavailable)?,
                updated_at: row
                    .try_get("updated_at")
                    .map_err(|_| ChildRunError::Unavailable)?,
            };
            let rows=sqlx::query("SELECT * FROM remote_cli_child_events WHERE session_id=$1 AND child_id=$2 AND seq>$3 ORDER BY seq LIMIT $4").bind(session_id).bind(child_id).bind(i64::try_from(after).map_err(|_|ChildRunError::Invalid)?).bind(i64::try_from(limit.clamp(1,256)).unwrap_or(256)).fetch_all(&self.pool).await.map_err(|_|ChildRunError::Unavailable)?;
            let events = rows
                .iter()
                .map(Self::event)
                .collect::<Result<Vec<_>, _>>()?;
            Ok((run, events))
        })
    }
}
