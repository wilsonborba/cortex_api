use chrono::{DateTime, Utc};
use sqlx::{PgPool, Row};
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use uuid::Uuid;

use crate::domain::services::remote_cli::{
    RemoteCliError, RemoteCliEvent, RemoteCliSessionRecord, RemoteCliStore, RemoteEventKind,
};

const MAX_PG_TS_MS: u64 = 253_402_300_799_000;

fn ts_from_ms(ms: u64) -> DateTime<Utc> {
    let clamped = ms.min(MAX_PG_TS_MS) as i64;
    DateTime::from_timestamp_millis(clamped).unwrap_or(DateTime::UNIX_EPOCH)
}

fn ms_from_ts(ts: DateTime<Utc>) -> u64 {
    ts.timestamp_millis().max(0) as u64
}

fn kind_to_str(kind: RemoteEventKind) -> &'static str {
    match kind {
        RemoteEventKind::Prompt => "prompt",
        RemoteEventKind::Progress => "progress",
        RemoteEventKind::AnswerPart => "answer_part",
        RemoteEventKind::ApprovalRequest => "approval_request",
        RemoteEventKind::ApprovalAnswer => "approval_answer",
        RemoteEventKind::Artifact => "artifact",
        RemoteEventKind::TurnEnd => "turn_end",
        RemoteEventKind::Cancel => "cancel",
        RemoteEventKind::CommandExpired => "command_expired",
    }
}

fn str_to_kind(raw: &str) -> Result<RemoteEventKind, RemoteCliError> {
    match raw {
        "prompt" => Ok(RemoteEventKind::Prompt),
        "progress" => Ok(RemoteEventKind::Progress),
        "answer_part" => Ok(RemoteEventKind::AnswerPart),
        "approval_request" => Ok(RemoteEventKind::ApprovalRequest),
        "approval_answer" => Ok(RemoteEventKind::ApprovalAnswer),
        "artifact" => Ok(RemoteEventKind::Artifact),
        "turn_end" => Ok(RemoteEventKind::TurnEnd),
        "cancel" => Ok(RemoteEventKind::Cancel),
        "command_expired" => Ok(RemoteEventKind::CommandExpired),
        _ => Err(RemoteCliError::Unavailable),
    }
}

fn json_opt_str(payload: &serde_json::Value, key: &str) -> Option<String> {
    payload
        .get(key)
        .and_then(|v| v.as_str())
        .map(ToOwned::to_owned)
}

fn json_opt_bool(payload: &serde_json::Value, key: &str) -> Option<bool> {
    payload.get(key).and_then(|v| v.as_bool())
}

fn json_opt_i64(payload: &serde_json::Value, key: &str) -> Option<i64> {
    payload.get(key).and_then(|v| v.as_i64())
}

struct EventPayloadFields {
    turn_id: Option<String>,
    message_text: Option<String>,
    stage: Option<String>,
    approval_id: Option<String>,
    action_text: Option<String>,
    approved: Option<bool>,
    file_id: Option<String>,
    artifact_name: Option<String>,
    content_type: Option<String>,
    command_id: Option<i64>,
    status: Option<String>,
}

fn build_event_payload(kind: RemoteEventKind, f: EventPayloadFields) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    match kind {
        RemoteEventKind::Progress => {
            if let Some(s) = f.stage {
                map.insert("stage".into(), serde_json::Value::String(s));
            }
            if let Some(m) = f.message_text {
                map.insert("message".into(), serde_json::Value::String(m));
            }
            if let Some(t) = f.turn_id {
                map.insert("turn_id".into(), serde_json::Value::String(t));
            }
        }
        RemoteEventKind::AnswerPart => {
            if let Some(m) = f.message_text {
                map.insert("text".into(), serde_json::Value::String(m));
            }
            if let Some(t) = f.turn_id {
                map.insert("turn_id".into(), serde_json::Value::String(t));
            }
        }
        RemoteEventKind::ApprovalRequest => {
            if let Some(a) = f.approval_id {
                map.insert("approval_id".into(), serde_json::Value::String(a));
            }
            if let Some(act) = f.action_text {
                map.insert("action".into(), serde_json::Value::String(act));
            }
            if let Some(t) = f.turn_id {
                map.insert("turn_id".into(), serde_json::Value::String(t));
            }
        }
        RemoteEventKind::ApprovalAnswer => {
            if let Some(a) = f.approval_id {
                map.insert("approval_id".into(), serde_json::Value::String(a));
            }
            if let Some(ok) = f.approved {
                map.insert("approved".into(), serde_json::Value::Bool(ok));
            }
            if let Some(t) = f.turn_id {
                map.insert("turn_id".into(), serde_json::Value::String(t));
            }
        }
        RemoteEventKind::Artifact => {
            if let Some(file_id) = f.file_id {
                map.insert("file_id".into(), serde_json::Value::String(file_id));
            }
            if let Some(n) = f.artifact_name {
                map.insert("name".into(), serde_json::Value::String(n));
            }
            if let Some(ct) = f.content_type {
                map.insert("content_type".into(), serde_json::Value::String(ct));
            }
            if let Some(t) = f.turn_id {
                map.insert("turn_id".into(), serde_json::Value::String(t));
            }
        }
        RemoteEventKind::TurnEnd => {
            if let Some(t) = f.turn_id {
                map.insert("turn_id".into(), serde_json::Value::String(t));
            }
            if let Some(s) = f.status {
                map.insert("status".into(), serde_json::Value::String(s));
            }
            if let Some(m) = f.message_text {
                map.insert("message".into(), serde_json::Value::String(m));
            }
        }
        RemoteEventKind::Cancel => {
            map.insert(
                "turn_id".into(),
                f.turn_id
                    .map(serde_json::Value::String)
                    .unwrap_or(serde_json::Value::Null),
            );
        }
        RemoteEventKind::CommandExpired => {
            if let Some(cid) = f.command_id {
                map.insert("command_id".into(), serde_json::json!(cid));
            }
            if let Some(s) = f.status {
                map.insert("status".into(), serde_json::Value::String(s));
            }
        }
        RemoteEventKind::Prompt => {}
    }
    serde_json::Value::Object(map)
}

#[derive(Clone)]
pub struct PostgresRemoteCliStore {
    pool: PgPool,
}

impl PostgresRemoteCliStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    async fn lock_session(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
        session_id: Uuid,
        caller: &str,
    ) -> Result<(String, i64, i64), RemoteCliError> {
        let row = sqlx::query(
            "SELECT owner_user, next_cli_seq, next_ctrl_seq
             FROM remote_cli_sessions
             WHERE session_id = $1
             FOR UPDATE",
        )
        .bind(session_id)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|_| RemoteCliError::Unavailable)?
        .ok_or(RemoteCliError::NotFound)?;

        let owner: String = row
            .try_get("owner_user")
            .map_err(|_| RemoteCliError::Unavailable)?;
        if owner != caller {
            return Err(RemoteCliError::Forbidden);
        }
        let cli_seq: i64 = row
            .try_get("next_cli_seq")
            .map_err(|_| RemoteCliError::Unavailable)?;
        let ctrl_seq: i64 = row
            .try_get("next_ctrl_seq")
            .map_err(|_| RemoteCliError::Unavailable)?;
        Ok((owner, cli_seq, ctrl_seq))
    }

    async fn check_owner(&self, session_id: Uuid, caller: &str) -> Result<(), RemoteCliError> {
        let owner: Option<String> =
            sqlx::query_scalar("SELECT owner_user FROM remote_cli_sessions WHERE session_id = $1")
                .bind(session_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;

        let Some(owner) = owner else {
            return Err(RemoteCliError::NotFound);
        };
        if owner != caller {
            return Err(RemoteCliError::Forbidden);
        }
        Ok(())
    }
}

impl RemoteCliStore for PostgresRemoteCliStore {
    fn register<'a>(
        &'a self,
        owner_user: &'a str,
        device_name: &'a str,
        platform: &'a str,
        now_ms: u64,
        _session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliSessionRecord, RemoteCliError>> + Send + 'a>>
    {
        Box::pin(async move {
            let session_id = Uuid::now_v7();
            let now = ts_from_ms(now_ms);
            sqlx::query(
                "INSERT INTO remote_cli_sessions (
                    session_id, owner_user, device_name, platform, cli_connected,
                    last_seen_at, last_disconnect_at, next_cli_seq, next_ctrl_seq, created_at
                ) VALUES ($1, $2, $3, $4, true, $5, NULL, 0, 0, $5)",
            )
            .bind(session_id)
            .bind(owner_user)
            .bind(device_name)
            .bind(platform)
            .bind(now)
            .execute(&self.pool)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            Ok(RemoteCliSessionRecord {
                session_id: session_id.to_string(),
                device_name: device_name.to_string(),
                platform: platform.to_string(),
                last_seen_ms: now_ms,
                cli_connected: true,
                last_disconnect_ms: None,
            })
        })
    }

    fn unregister<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            self.check_owner(sid, caller).await?;
            let affected = sqlx::query(
                "DELETE FROM remote_cli_sessions WHERE session_id = $1 AND owner_user = $2",
            )
            .bind(sid)
            .bind(caller)
            .execute(&self.pool)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?
            .rows_affected();
            if affected == 0 {
                return Err(RemoteCliError::NotFound);
            }
            Ok(())
        })
    }

    fn list_sessions<'a>(
        &'a self,
        caller: &'a str,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<RemoteCliSessionRecord>, RemoteCliError>> + Send + 'a>,
    > {
        Box::pin(async move {
            let cutoff = ts_from_ms(now_ms.saturating_sub(session_ttl_ms.max(1)));
            let rows = sqlx::query(
                "SELECT session_id, device_name, platform, cli_connected, last_seen_at, last_disconnect_at
                 FROM remote_cli_sessions
                 WHERE owner_user = $1 AND last_seen_at > $2
                 ORDER BY last_seen_at DESC, session_id DESC",
            )
            .bind(caller)
            .bind(cutoff)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let sid: Uuid = row
                    .try_get("session_id")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let last_seen: DateTime<Utc> = row
                    .try_get("last_seen_at")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let last_disc: Option<DateTime<Utc>> = row
                    .try_get("last_disconnect_at")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                out.push(RemoteCliSessionRecord {
                    session_id: sid.to_string(),
                    device_name: row
                        .try_get("device_name")
                        .map_err(|_| RemoteCliError::Unavailable)?,
                    platform: row
                        .try_get("platform")
                        .map_err(|_| RemoteCliError::Unavailable)?,
                    last_seen_ms: ms_from_ts(last_seen),
                    cli_connected: row
                        .try_get("cli_connected")
                        .map_err(|_| RemoteCliError::Unavailable)?,
                    last_disconnect_ms: last_disc.map(ms_from_ts),
                });
            }
            Ok(out)
        })
    }

    fn set_cli_connected<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        connected: bool,
        now_ms: u64,
        _session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            self.check_owner(sid, caller).await?;
            let now = ts_from_ms(now_ms);
            let disc_ts = if connected { None } else { Some(now) };
            let affected = sqlx::query(
                "UPDATE remote_cli_sessions
                 SET cli_connected = $3,
                     last_seen_at = $4,
                     last_disconnect_at = COALESCE($5, last_disconnect_at)
                 WHERE session_id = $1 AND owner_user = $2",
            )
            .bind(sid)
            .bind(caller)
            .bind(connected)
            .bind(now)
            .bind(disc_ts)
            .execute(&self.pool)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?
            .rows_affected();
            if affected == 0 {
                return Err(RemoteCliError::NotFound);
            }
            Ok(())
        })
    }

    fn send_prompt<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        prompt: &'a str,
        file_ids: &'a [String],
        now_ms: u64,
        command_ttl_ms: u64,
        _session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;
            let (_, cli_seq, _) = Self::lock_session(&mut tx, sid, caller).await?;
            let seq = cli_seq + 1;
            let now = ts_from_ms(now_ms);
            let ttl_ms = i64::try_from(command_ttl_ms.max(1)).unwrap_or(i64::MAX);

            sqlx::query(
                "UPDATE remote_cli_sessions
                 SET next_cli_seq = $2, last_seen_at = GREATEST(last_seen_at, $3)
                 WHERE session_id = $1",
            )
            .bind(sid)
            .bind(seq)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            sqlx::query(
                "INSERT INTO remote_cli_commands (
                    session_id, seq, kind, prompt_text, ttl_ms, created_at
                 ) VALUES ($1, $2, 'prompt', $3, $4, $5)",
            )
            .bind(sid)
            .bind(seq)
            .bind(prompt)
            .bind(ttl_ms)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            let mut clean_files = Vec::new();
            for (pos, fid) in file_ids.iter().enumerate() {
                let trimmed = fid.trim();
                if trimmed.is_empty() {
                    return Err(RemoteCliError::BadRequest(
                        "file_ids entries must not be empty".into(),
                    ));
                }
                clean_files.push(trimmed.to_string());
                sqlx::query(
                    "INSERT INTO remote_cli_command_files (session_id, seq, position, file_id)
                     VALUES ($1, $2, $3, $4)",
                )
                .bind(sid)
                .bind(seq)
                .bind(pos as i32)
                .bind(trimmed)
                .execute(&mut *tx)
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;
            }

            tx.commit().await.map_err(|_| RemoteCliError::Unavailable)?;
            Ok(RemoteCliEvent {
                id: seq as u64,
                kind: RemoteEventKind::Prompt,
                payload: serde_json::json!({ "prompt": prompt, "file_ids": clean_files }),
                created_at_ms: now_ms,
            })
        })
    }

    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        turn_id: Option<&'a str>,
        now_ms: u64,
        command_ttl_ms: u64,
        _session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;
            let (_, cli_seq, ctrl_seq) = Self::lock_session(&mut tx, sid, caller).await?;
            let new_cli_seq = cli_seq + 1;
            let new_ctrl_seq = ctrl_seq + 1;
            let now = ts_from_ms(now_ms);
            let ttl_ms = i64::try_from(command_ttl_ms.max(1)).unwrap_or(i64::MAX);

            sqlx::query(
                "UPDATE remote_cli_sessions
                 SET next_cli_seq = $2, next_ctrl_seq = $3, last_seen_at = GREATEST(last_seen_at, $4)
                 WHERE session_id = $1",
            )
            .bind(sid)
            .bind(new_cli_seq)
            .bind(new_ctrl_seq)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            sqlx::query(
                "INSERT INTO remote_cli_commands (
                    session_id, seq, kind, turn_id, ttl_ms, created_at
                 ) VALUES ($1, $2, 'cancel', $3, $4, $5)",
            )
            .bind(sid)
            .bind(new_cli_seq)
            .bind(turn_id)
            .bind(ttl_ms)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            sqlx::query(
                "INSERT INTO remote_cli_events (
                    session_id, seq, kind, turn_id, created_at
                 ) VALUES ($1, $2, 'cancel', $3, $4)",
            )
            .bind(sid)
            .bind(new_ctrl_seq)
            .bind(turn_id)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            tx.commit().await.map_err(|_| RemoteCliError::Unavailable)?;
            Ok(RemoteCliEvent {
                id: new_cli_seq as u64,
                kind: RemoteEventKind::Cancel,
                payload: serde_json::json!({ "turn_id": turn_id }),
                created_at_ms: now_ms,
            })
        })
    }

    fn post_cli_event<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        kind: RemoteEventKind,
        payload: &'a serde_json::Value,
        ack_command_id: Option<u64>,
        now_ms: u64,
        _session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            if matches!(
                kind,
                RemoteEventKind::Prompt | RemoteEventKind::CommandExpired
            ) {
                return Err(RemoteCliError::BadRequest(
                    "invalid event kind for CLI post".into(),
                ));
            }
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;
            let (_, _, ctrl_seq) = Self::lock_session(&mut tx, sid, caller).await?;
            let now = ts_from_ms(now_ms);

            if let Some(cmd_id) = ack_command_id {
                sqlx::query(
                    "UPDATE remote_cli_commands
                     SET acked_at = COALESCE(acked_at, $3)
                     WHERE session_id = $1 AND seq = $2",
                )
                .bind(sid)
                .bind(cmd_id as i64)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;
            }

            let turn_id = json_opt_str(payload, "turn_id");
            let message_text = match kind {
                RemoteEventKind::AnswerPart => json_opt_str(payload, "text"),
                _ => json_opt_str(payload, "message"),
            };
            let stage = json_opt_str(payload, "stage");
            let approval_id = json_opt_str(payload, "approval_id");
            let action_text = json_opt_str(payload, "action");
            let approved = json_opt_bool(payload, "approved");
            let file_id = json_opt_str(payload, "file_id");
            let artifact_name = json_opt_str(payload, "name");
            let content_type = json_opt_str(payload, "content_type");
            let command_id = json_opt_i64(payload, "command_id");
            let status = json_opt_str(payload, "status");

            if kind == RemoteEventKind::ApprovalAnswer {
                let aid = approval_id
                    .as_deref()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| RemoteCliError::BadRequest("approval_id is required".into()))?;
                let ok = approved.ok_or_else(|| {
                    RemoteCliError::BadRequest("approved boolean is required".into())
                })?;
                let inserted = sqlx::query(
                    "INSERT INTO remote_cli_approvals (session_id, approval_id, approved, answered_at)
                     VALUES ($1, $2, $3, $4)
                     ON CONFLICT (session_id, approval_id) DO NOTHING",
                )
                .bind(sid)
                .bind(aid)
                .bind(ok)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| RemoteCliError::Unavailable)?
                .rows_affected();
                if inserted == 0 {
                    return Err(RemoteCliError::AlreadyAnswered);
                }
            }

            let new_ctrl_seq = ctrl_seq + 1;
            sqlx::query(
                "UPDATE remote_cli_sessions
                 SET next_ctrl_seq = $2,
                     cli_connected = true,
                     last_seen_at = $3
                 WHERE session_id = $1",
            )
            .bind(sid)
            .bind(new_ctrl_seq)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            sqlx::query(
                "INSERT INTO remote_cli_events (
                    session_id, seq, kind, turn_id, message_text, stage,
                    approval_id, action_text, approved, file_id, artifact_name,
                    content_type, command_id, status, created_at
                ) VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15)",
            )
            .bind(sid)
            .bind(new_ctrl_seq)
            .bind(kind_to_str(kind))
            .bind(&turn_id)
            .bind(&message_text)
            .bind(&stage)
            .bind(&approval_id)
            .bind(&action_text)
            .bind(approved)
            .bind(&file_id)
            .bind(&artifact_name)
            .bind(&content_type)
            .bind(command_id)
            .bind(&status)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            tx.commit().await.map_err(|_| RemoteCliError::Unavailable)?;
            Ok(RemoteCliEvent {
                id: new_ctrl_seq as u64,
                kind,
                payload: build_event_payload(
                    kind,
                    EventPayloadFields {
                        turn_id,
                        message_text,
                        stage,
                        approval_id,
                        action_text,
                        approved,
                        file_id,
                        artifact_name,
                        content_type,
                        command_id,
                        status,
                    },
                ),
                created_at_ms: now_ms,
            })
        })
    }

    fn answer_approval<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        approval_id: &'a str,
        approved: bool,
        now_ms: u64,
        command_ttl_ms: u64,
        _session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;
            let (_, cli_seq, ctrl_seq) = Self::lock_session(&mut tx, sid, caller).await?;
            let now = ts_from_ms(now_ms);
            let ttl_ms = i64::try_from(command_ttl_ms.max(1)).unwrap_or(i64::MAX);

            let inserted = sqlx::query(
                "INSERT INTO remote_cli_approvals (session_id, approval_id, approved, answered_at)
                 VALUES ($1, $2, $3, $4)
                 ON CONFLICT (session_id, approval_id) DO NOTHING",
            )
            .bind(sid)
            .bind(approval_id)
            .bind(approved)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?
            .rows_affected();
            if inserted == 0 {
                return Err(RemoteCliError::AlreadyAnswered);
            }

            let new_cli_seq = cli_seq + 1;
            let new_ctrl_seq = ctrl_seq + 1;
            sqlx::query(
                "UPDATE remote_cli_sessions
                 SET next_cli_seq = $2, next_ctrl_seq = $3, last_seen_at = GREATEST(last_seen_at, $4)
                 WHERE session_id = $1",
            )
            .bind(sid)
            .bind(new_cli_seq)
            .bind(new_ctrl_seq)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            sqlx::query(
                "INSERT INTO remote_cli_commands (
                    session_id, seq, kind, approval_id, approved, ttl_ms, created_at
                 ) VALUES ($1, $2, 'approval_answer', $3, $4, $5, $6)",
            )
            .bind(sid)
            .bind(new_cli_seq)
            .bind(approval_id)
            .bind(approved)
            .bind(ttl_ms)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            sqlx::query(
                "INSERT INTO remote_cli_events (
                    session_id, seq, kind, approval_id, approved, created_at
                 ) VALUES ($1, $2, 'approval_answer', $3, $4, $5)",
            )
            .bind(sid)
            .bind(new_ctrl_seq)
            .bind(approval_id)
            .bind(approved)
            .bind(now)
            .execute(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            tx.commit().await.map_err(|_| RemoteCliError::Unavailable)?;
            Ok(RemoteCliEvent {
                id: new_cli_seq as u64,
                kind: RemoteEventKind::ApprovalAnswer,
                payload: serde_json::json!({ "approval_id": approval_id, "approved": approved }),
                created_at_ms: now_ms,
            })
        })
    }

    fn expire_pending_commands<'a>(
        &'a self,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let now = ts_from_ms(now_ms);
            let mut tx = self
                .pool
                .begin()
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;
            let expired_rows = sqlx::query(
                "UPDATE remote_cli_commands
                 SET expired_reported_at = $1, acked_at = $1
                 WHERE acked_at IS NULL
                   AND expired_reported_at IS NULL
                   AND created_at + make_interval(secs => ttl_ms::double precision / 1000.0) < $1
                 RETURNING session_id, seq",
            )
            .bind(now)
            .fetch_all(&mut *tx)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            let mut touched_sessions = Vec::with_capacity(expired_rows.len());
            for row in expired_rows {
                let sid: Uuid = row
                    .try_get("session_id")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let seq: i64 = row
                    .try_get("seq")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let next_seq: i64 = sqlx::query_scalar(
                    "UPDATE remote_cli_sessions
                     SET next_ctrl_seq = next_ctrl_seq + 1
                     WHERE session_id = $1
                     RETURNING next_ctrl_seq",
                )
                .bind(sid)
                .fetch_one(&mut *tx)
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;

                sqlx::query(
                    "INSERT INTO remote_cli_events (
                        session_id, seq, kind, command_id, status, created_at
                     ) VALUES ($1, $2, 'command_expired', $3, 'not_delivered', $4)",
                )
                .bind(sid)
                .bind(next_seq)
                .bind(seq)
                .bind(now)
                .execute(&mut *tx)
                .await
                .map_err(|_| RemoteCliError::Unavailable)?;

                touched_sessions.push(sid.to_string());
            }
            tx.commit().await.map_err(|_| RemoteCliError::Unavailable)?;
            Ok(touched_sessions)
        })
    }

    fn purge_expired_sessions<'a>(
        &'a self,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<usize, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let cutoff = ts_from_ms(now_ms.saturating_sub(session_ttl_ms.max(1)));
            let affected = sqlx::query("DELETE FROM remote_cli_sessions WHERE last_seen_at <= $1")
                .bind(cutoff)
                .execute(&self.pool)
                .await
                .map_err(|_| RemoteCliError::Unavailable)?
                .rows_affected();
            Ok(affected as usize)
        })
    }

    fn cli_commands_since<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        after_cursor: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<RemoteCliEvent>, RemoteCliError>> + Send + 'a>>
    {
        Box::pin(async move {
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            self.check_owner(sid, caller).await?;
            let after = i64::try_from(after_cursor).unwrap_or(i64::MAX);
            let rows = sqlx::query(
                "SELECT seq, kind, prompt_text, turn_id, approval_id, approved, created_at
                 FROM remote_cli_commands
                 WHERE session_id = $1 AND seq > $2
                 ORDER BY seq ASC",
            )
            .bind(sid)
            .bind(after)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            let file_rows = sqlx::query(
                "SELECT seq, file_id
                 FROM remote_cli_command_files
                 WHERE session_id = $1 AND seq > $2
                 ORDER BY seq ASC, position ASC",
            )
            .bind(sid)
            .bind(after)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            let mut files_by_seq: BTreeMap<i64, Vec<String>> = BTreeMap::new();
            for fr in file_rows {
                let seq: i64 = fr.try_get("seq").map_err(|_| RemoteCliError::Unavailable)?;
                let fid: String = fr
                    .try_get("file_id")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                files_by_seq.entry(seq).or_default().push(fid);
            }

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let seq: i64 = row
                    .try_get("seq")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let kind_str: String = row
                    .try_get("kind")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let kind = str_to_kind(&kind_str)?;
                let created_at: DateTime<Utc> = row
                    .try_get("created_at")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let payload = match kind {
                    RemoteEventKind::Prompt => {
                        let prompt: String = row
                            .try_get::<Option<String>, _>("prompt_text")
                            .map_err(|_| RemoteCliError::Unavailable)?
                            .unwrap_or_default();
                        let files = files_by_seq.remove(&seq).unwrap_or_default();
                        serde_json::json!({ "prompt": prompt, "file_ids": files })
                    }
                    RemoteEventKind::ApprovalAnswer => {
                        let approval_id: String = row
                            .try_get::<Option<String>, _>("approval_id")
                            .map_err(|_| RemoteCliError::Unavailable)?
                            .unwrap_or_default();
                        let approved: bool = row
                            .try_get::<Option<bool>, _>("approved")
                            .map_err(|_| RemoteCliError::Unavailable)?
                            .unwrap_or(false);
                        serde_json::json!({ "approval_id": approval_id, "approved": approved })
                    }
                    RemoteEventKind::Cancel => {
                        let turn_id: Option<String> = row
                            .try_get("turn_id")
                            .map_err(|_| RemoteCliError::Unavailable)?;
                        serde_json::json!({ "turn_id": turn_id })
                    }
                    _ => serde_json::json!({}),
                };
                out.push(RemoteCliEvent {
                    id: seq as u64,
                    kind,
                    payload,
                    created_at_ms: ms_from_ts(created_at),
                });
            }
            Ok(out)
        })
    }

    fn ctrl_events_since<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        after_cursor: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<RemoteCliEvent>, RemoteCliError>> + Send + 'a>>
    {
        Box::pin(async move {
            let sid = Uuid::parse_str(session_id).map_err(|_| RemoteCliError::NotFound)?;
            self.check_owner(sid, caller).await?;
            let after = i64::try_from(after_cursor).unwrap_or(i64::MAX);
            let rows = sqlx::query(
                "SELECT seq, kind, turn_id, message_text, stage, approval_id, action_text,
                        approved, file_id, artifact_name, content_type, command_id, status, created_at
                 FROM remote_cli_events
                 WHERE session_id = $1 AND seq > $2
                 ORDER BY seq ASC",
            )
            .bind(sid)
            .bind(after)
            .fetch_all(&self.pool)
            .await
            .map_err(|_| RemoteCliError::Unavailable)?;

            let mut out = Vec::with_capacity(rows.len());
            for row in rows {
                let seq: i64 = row
                    .try_get("seq")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let kind_str: String = row
                    .try_get("kind")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let kind = str_to_kind(&kind_str)?;
                let created_at: DateTime<Utc> = row
                    .try_get("created_at")
                    .map_err(|_| RemoteCliError::Unavailable)?;
                let payload = build_event_payload(
                    kind,
                    EventPayloadFields {
                        turn_id: row
                            .try_get("turn_id")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        message_text: row
                            .try_get("message_text")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        stage: row
                            .try_get("stage")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        approval_id: row
                            .try_get("approval_id")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        action_text: row
                            .try_get("action_text")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        approved: row
                            .try_get("approved")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        file_id: row
                            .try_get("file_id")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        artifact_name: row
                            .try_get("artifact_name")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        content_type: row
                            .try_get("content_type")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        command_id: row
                            .try_get("command_id")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                        status: row
                            .try_get("status")
                            .map_err(|_| RemoteCliError::Unavailable)?,
                    },
                );
                out.push(RemoteCliEvent {
                    id: seq as u64,
                    kind,
                    payload,
                    created_at_ms: ms_from_ts(created_at),
                });
            }
            Ok(out)
        })
    }
}
