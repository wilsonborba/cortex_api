use std::collections::{BTreeMap, HashMap};
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use serde::{Deserialize, Serialize};

use crate::core::config::Config;
use crate::dal::remote::cache::RedisCache;
use crate::domain::models::cache::{CacheKey, Lifetime};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RemoteEventKind {
    Prompt,
    Progress,
    AnswerPart,
    ApprovalRequest,
    ApprovalAnswer,
    Artifact,
    TurnEnd,
    Cancel,
    CommandExpired,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RemoteCliEvent {
    pub id: u64,
    pub kind: RemoteEventKind,
    pub payload: serde_json::Value,
    pub created_at_ms: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RemoteCliSessionView {
    pub session_id: String,
    pub device_name: String,
    pub platform: String,
    pub last_seen_ms: u64,
    pub online: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteCliSessionRecord {
    pub session_id: String,
    pub device_name: String,
    pub platform: String,
    pub last_seen_ms: u64,
    pub cli_connected: bool,
    pub last_disconnect_ms: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RemoteCliError {
    NotFound,
    Forbidden,
    AlreadyAnswered,
    BadRequest(String),
    Unavailable,
}

pub trait RemoteCliStore: Send + Sync {
    fn register<'a>(
        &'a self,
        owner_user: &'a str,
        device_name: &'a str,
        platform: &'a str,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliSessionRecord, RemoteCliError>> + Send + 'a>>;

    fn unregister<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), RemoteCliError>> + Send + 'a>>;

    fn list_sessions<'a>(
        &'a self,
        caller: &'a str,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<
        Box<dyn Future<Output = Result<Vec<RemoteCliSessionRecord>, RemoteCliError>> + Send + 'a>,
    >;

    fn set_cli_connected<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        connected: bool,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), RemoteCliError>> + Send + 'a>>;

    #[allow(clippy::too_many_arguments)]
    fn send_prompt<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        prompt: &'a str,
        file_ids: &'a [String],
        now_ms: u64,
        command_ttl_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>>;

    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        turn_id: Option<&'a str>,
        now_ms: u64,
        command_ttl_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>>;

    #[allow(clippy::too_many_arguments)]
    fn post_cli_event<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        kind: RemoteEventKind,
        payload: &'a serde_json::Value,
        ack_command_id: Option<u64>,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>>;

    #[allow(clippy::too_many_arguments)]
    fn answer_approval<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        approval_id: &'a str,
        approved: bool,
        now_ms: u64,
        command_ttl_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>>;

    #[allow(clippy::too_many_arguments)]
    fn answer_bound_output_action<'a>(
        &'a self,
        _session_id: &'a str,
        _caller: &'a str,
        _chat_session_id: Uuid,
        _output_id: Uuid,
        _action_id: Uuid,
        _approved: bool,
        _now_ms: u64,
        _command_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async { Err(RemoteCliError::NotFound) })
    }

    fn expire_pending_commands<'a>(
        &'a self,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, RemoteCliError>> + Send + 'a>>;

    fn purge_expired_sessions<'a>(
        &'a self,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<usize, RemoteCliError>> + Send + 'a>>;

    fn cli_commands_since<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        after_cursor: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<RemoteCliEvent>, RemoteCliError>> + Send + 'a>>;

    fn ctrl_events_since<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        after_cursor: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<RemoteCliEvent>, RemoteCliError>> + Send + 'a>>;
}

struct PendingCommand {
    event_id: u64,
    expires_at_ms: u64,
    acked: bool,
}

struct MemSession {
    session_id: String,
    owner_user: String,
    device_name: String,
    platform: String,
    last_seen_ms: u64,
    cli_connected: bool,
    last_disconnect_ms: Option<u64>,
    expires_at_ms: u64,
    next_cli_seq: u64,
    next_ctrl_seq: u64,
    cli_commands: Vec<RemoteCliEvent>,
    ctrl_events: Vec<RemoteCliEvent>,
    pending_commands: Vec<PendingCommand>,
    answered_approvals: BTreeMap<String, bool>,
}

/// In-memory test double for isolated unit tests.
#[derive(Default)]
pub struct InMemoryRemoteCliStore {
    sessions: RwLock<HashMap<String, MemSession>>,
}

impl RemoteCliStore for InMemoryRemoteCliStore {
    fn register<'a>(
        &'a self,
        owner_user: &'a str,
        device_name: &'a str,
        platform: &'a str,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliSessionRecord, RemoteCliError>> + Send + 'a>>
    {
        Box::pin(async move {
            let sid = Uuid::now_v7().to_string();
            let rec = MemSession {
                session_id: sid.clone(),
                owner_user: owner_user.to_string(),
                device_name: device_name.to_string(),
                platform: platform.to_string(),
                last_seen_ms: now_ms,
                cli_connected: true,
                last_disconnect_ms: None,
                expires_at_ms: now_ms.saturating_add(session_ttl_ms.max(1)),
                next_cli_seq: 0,
                next_ctrl_seq: 0,
                cli_commands: Vec::new(),
                ctrl_events: Vec::new(),
                pending_commands: Vec::new(),
                answered_approvals: BTreeMap::new(),
            };
            let out = RemoteCliSessionRecord {
                session_id: rec.session_id.clone(),
                device_name: rec.device_name.clone(),
                platform: rec.platform.clone(),
                last_seen_ms: rec.last_seen_ms,
                cli_connected: true,
                last_disconnect_ms: None,
            };
            self.sessions.write().await.insert(sid, rec);
            Ok(out)
        })
    }

    fn unregister<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<(), RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let s = guard.get(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            guard.remove(session_id);
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
            let guard = self.sessions.read().await;
            let cutoff = now_ms.saturating_sub(session_ttl_ms.max(1));
            Ok(guard
                .values()
                .filter(|s| s.owner_user == caller && s.last_seen_ms > cutoff)
                .map(|s| RemoteCliSessionRecord {
                    session_id: s.session_id.clone(),
                    device_name: s.device_name.clone(),
                    platform: s.platform.clone(),
                    last_seen_ms: s.last_seen_ms,
                    cli_connected: s.cli_connected,
                    last_disconnect_ms: s.last_disconnect_ms,
                })
                .collect())
        })
    }

    fn set_cli_connected<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        connected: bool,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<(), RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            s.cli_connected = connected;
            s.last_seen_ms = now_ms;
            s.expires_at_ms = s
                .expires_at_ms
                .max(now_ms.saturating_add(session_ttl_ms.max(1)));
            if !connected {
                s.last_disconnect_ms = Some(now_ms);
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
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            s.expires_at_ms = s
                .expires_at_ms
                .max(now_ms.saturating_add(session_ttl_ms.max(1)));
            s.next_cli_seq += 1;
            let ev = RemoteCliEvent {
                id: s.next_cli_seq,
                kind: RemoteEventKind::Prompt,
                payload: serde_json::json!({ "prompt": prompt, "file_ids": file_ids }),
                created_at_ms: now_ms,
            };
            s.cli_commands.push(ev.clone());
            s.pending_commands.push(PendingCommand {
                event_id: ev.id,
                expires_at_ms: now_ms.saturating_add(command_ttl_ms.max(1)),
                acked: false,
            });
            Ok(ev)
        })
    }

    fn cancel_turn<'a>(
        &'a self,
        session_id: &'a str,
        caller: &'a str,
        turn_id: Option<&'a str>,
        now_ms: u64,
        command_ttl_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            s.expires_at_ms = s
                .expires_at_ms
                .max(now_ms.saturating_add(session_ttl_ms.max(1)));
            let payload = serde_json::json!({ "turn_id": turn_id });
            s.next_cli_seq += 1;
            let cli_ev = RemoteCliEvent {
                id: s.next_cli_seq,
                kind: RemoteEventKind::Cancel,
                payload: payload.clone(),
                created_at_ms: now_ms,
            };
            s.cli_commands.push(cli_ev.clone());
            s.pending_commands.push(PendingCommand {
                event_id: cli_ev.id,
                expires_at_ms: now_ms.saturating_add(command_ttl_ms.max(1)),
                acked: false,
            });
            s.next_ctrl_seq += 1;
            s.ctrl_events.push(RemoteCliEvent {
                id: s.next_ctrl_seq,
                kind: RemoteEventKind::Cancel,
                payload,
                created_at_ms: now_ms,
            });
            Ok(cli_ev)
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
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            if kind == RemoteEventKind::ApprovalAnswer {
                let aid = payload
                    .get("approval_id")
                    .and_then(|v| v.as_str())
                    .map(str::trim)
                    .filter(|v| !v.is_empty())
                    .ok_or_else(|| RemoteCliError::BadRequest("approval_id is required".into()))?;
                let approved = payload
                    .get("approved")
                    .and_then(|v| v.as_bool())
                    .ok_or_else(|| {
                        RemoteCliError::BadRequest("approved boolean is required".into())
                    })?;
                if s.answered_approvals.contains_key(aid) {
                    return Err(RemoteCliError::AlreadyAnswered);
                }
                s.answered_approvals.insert(aid.to_string(), approved);
            }
            s.last_seen_ms = now_ms;
            s.cli_connected = true;
            s.expires_at_ms = s
                .expires_at_ms
                .max(now_ms.saturating_add(session_ttl_ms.max(1)));
            if let Some(cmd_id) = ack_command_id {
                for p in &mut s.pending_commands {
                    if p.event_id == cmd_id {
                        p.acked = true;
                    }
                }
            }
            s.next_ctrl_seq += 1;
            let ev = RemoteCliEvent {
                id: s.next_ctrl_seq,
                kind,
                payload: payload.clone(),
                created_at_ms: now_ms,
            };
            s.ctrl_events.push(ev.clone());
            Ok(ev)
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
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<RemoteCliEvent, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            if s.answered_approvals.contains_key(approval_id) {
                return Err(RemoteCliError::AlreadyAnswered);
            }
            s.answered_approvals
                .insert(approval_id.to_string(), approved);
            s.expires_at_ms = s
                .expires_at_ms
                .max(now_ms.saturating_add(session_ttl_ms.max(1)));
            let payload = serde_json::json!({ "approval_id": approval_id, "approved": approved });
            s.next_cli_seq += 1;
            let cli_ev = RemoteCliEvent {
                id: s.next_cli_seq,
                kind: RemoteEventKind::ApprovalAnswer,
                payload: payload.clone(),
                created_at_ms: now_ms,
            };
            s.cli_commands.push(cli_ev.clone());
            s.pending_commands.push(PendingCommand {
                event_id: cli_ev.id,
                expires_at_ms: now_ms.saturating_add(command_ttl_ms.max(1)),
                acked: false,
            });
            s.next_ctrl_seq += 1;
            s.ctrl_events.push(RemoteCliEvent {
                id: s.next_ctrl_seq,
                kind: RemoteEventKind::ApprovalAnswer,
                payload,
                created_at_ms: now_ms,
            });
            Ok(cli_ev)
        })
    }

    fn expire_pending_commands<'a>(
        &'a self,
        now_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<String>, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let mut touched = Vec::new();
            for s in guard.values_mut() {
                let mut expired = Vec::new();
                for p in &mut s.pending_commands {
                    if !p.acked && now_ms > p.expires_at_ms {
                        p.acked = true;
                        expired.push(p.event_id);
                    }
                }
                for cmd_id in expired {
                    s.next_ctrl_seq += 1;
                    s.ctrl_events.push(RemoteCliEvent {
                        id: s.next_ctrl_seq,
                        kind: RemoteEventKind::CommandExpired,
                        payload: serde_json::json!({ "command_id": cmd_id, "status": "not_delivered" }),
                        created_at_ms: now_ms,
                    });
                    touched.push(s.session_id.clone());
                }
            }
            Ok(touched)
        })
    }

    fn purge_expired_sessions<'a>(
        &'a self,
        now_ms: u64,
        session_ttl_ms: u64,
    ) -> Pin<Box<dyn Future<Output = Result<usize, RemoteCliError>> + Send + 'a>> {
        Box::pin(async move {
            let mut guard = self.sessions.write().await;
            let cutoff = now_ms.saturating_sub(session_ttl_ms.max(1));
            let before = guard.len();
            guard.retain(|_, s| s.last_seen_ms > cutoff);
            Ok(before - guard.len())
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
            let guard = self.sessions.read().await;
            let s = guard.get(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            Ok(s.cli_commands
                .iter()
                .filter(|e| e.id > after_cursor)
                .cloned()
                .collect())
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
            let guard = self.sessions.read().await;
            let s = guard.get(session_id).ok_or(RemoteCliError::NotFound)?;
            if s.owner_user != caller {
                return Err(RemoteCliError::Forbidden);
            }
            Ok(s.ctrl_events
                .iter()
                .filter(|e| e.id > after_cursor)
                .cloned()
                .collect())
        })
    }
}

#[derive(Clone)]
pub struct RemoteCliService {
    store: Arc<dyn RemoteCliStore>,
    cache: RedisCache,
    reconnect_window_ms: u64,
    command_ttl_ms: u64,
    session_ttl_ms: u64,
    local_cli_tx: Arc<RwLock<HashMap<String, broadcast::Sender<u64>>>>,
    local_ctrl_tx: Arc<RwLock<HashMap<String, broadcast::Sender<u64>>>>,
}

impl Default for RemoteCliService {
    fn default() -> Self {
        Self::in_memory(15_000, 10_000, 3_600_000)
    }
}

impl RemoteCliService {
    pub fn new(store: Arc<dyn RemoteCliStore>, cache: RedisCache, config: &Config) -> Self {
        Self {
            store,
            cache,
            reconnect_window_ms: config.remote_cli_reconnect_window.as_millis() as u64,
            command_ttl_ms: config.remote_cli_command_ttl.as_millis() as u64,
            session_ttl_ms: config.temporary_ttl.as_millis().max(1) as u64,
            local_cli_tx: Arc::new(RwLock::new(HashMap::new())),
            local_ctrl_tx: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn with_timeouts(
        store: Arc<dyn RemoteCliStore>,
        cache: RedisCache,
        reconnect_window_ms: u64,
        command_ttl_ms: u64,
        session_ttl_ms: u64,
    ) -> Self {
        Self {
            store,
            cache,
            reconnect_window_ms,
            command_ttl_ms,
            session_ttl_ms,
            local_cli_tx: Arc::new(RwLock::new(HashMap::new())),
            local_ctrl_tx: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn in_memory(reconnect_window_ms: u64, command_ttl_ms: u64, session_ttl_ms: u64) -> Self {
        Self::with_timeouts(
            Arc::new(InMemoryRemoteCliStore::default()),
            RedisCache::new(None, Duration::from_millis(50)),
            reconnect_window_ms,
            command_ttl_ms,
            session_ttl_ms,
        )
    }

    pub fn cache(&self) -> &RedisCache {
        &self.cache
    }

    async fn local_subscribe(
        map: &RwLock<HashMap<String, broadcast::Sender<u64>>>,
        session_id: &str,
    ) -> broadcast::Receiver<u64> {
        {
            let guard = map.read().await;
            if let Some(tx) = guard.get(session_id) {
                return tx.subscribe();
            }
        }
        let mut guard = map.write().await;
        let tx = guard
            .entry(session_id.to_string())
            .or_insert_with(|| broadcast::channel(64).0);
        tx.subscribe()
    }

    async fn local_notify(
        map: &RwLock<HashMap<String, broadcast::Sender<u64>>>,
        session_id: &str,
        seq: u64,
    ) {
        let guard = map.read().await;
        if let Some(tx) = guard.get(session_id) {
            let _ = tx.send(seq);
        }
    }

    async fn notify_cli(&self, session_id: &str, seq: u64) {
        Self::local_notify(&self.local_cli_tx, session_id, seq).await;
        if self.cache.is_configured() {
            let _ = self
                .cache
                .publish(
                    &CacheKey::RemoteCliCliChannel(session_id.to_string()),
                    &seq.to_string(),
                )
                .await;
        }
    }

    async fn notify_ctrl(&self, session_id: &str, seq: u64) {
        Self::local_notify(&self.local_ctrl_tx, session_id, seq).await;
        if self.cache.is_configured() {
            let _ = self
                .cache
                .publish(
                    &CacheKey::RemoteCliCtrlChannel(session_id.to_string()),
                    &seq.to_string(),
                )
                .await;
        }
    }

    async fn set_presence(&self, session_id: &str, connected: bool) {
        if !self.cache.is_configured() {
            return;
        }
        let key = CacheKey::RemoteCliPresence(session_id.to_string());
        if connected {
            let ms = self.reconnect_window_ms.max(1000);
            if let Ok(ttl) = Lifetime::new(Duration::from_millis(ms)) {
                let _ = self
                    .cache
                    .put(&key, &serde_json::json!({"online": true}), ttl)
                    .await;
            }
        } else {
            let _ = self.cache.remove(&key).await;
        }
    }

    pub async fn register(
        &self,
        owner_user: &str,
        device_name: &str,
        platform: &str,
        now_ms: u64,
    ) -> Result<RemoteCliSessionView, RemoteCliError> {
        if device_name.trim().is_empty() || platform.trim().is_empty() {
            return Err(RemoteCliError::BadRequest(
                "device_name and platform are required".into(),
            ));
        }
        let rec = self
            .store
            .register(
                owner_user,
                device_name.trim(),
                platform.trim(),
                now_ms,
                self.session_ttl_ms,
            )
            .await?;
        self.set_presence(&rec.session_id, true).await;
        Ok(RemoteCliSessionView {
            session_id: rec.session_id,
            device_name: rec.device_name,
            platform: rec.platform,
            last_seen_ms: rec.last_seen_ms,
            online: true,
        })
    }

    pub async fn unregister(&self, session_id: &str, caller: &str) -> Result<(), RemoteCliError> {
        self.store.unregister(session_id, caller).await?;
        self.set_presence(session_id, false).await;
        Ok(())
    }

    pub async fn list_sessions(
        &self,
        caller: &str,
        now_ms: u64,
    ) -> Result<Vec<RemoteCliSessionView>, RemoteCliError> {
        let records = self
            .store
            .list_sessions(caller, now_ms, self.session_ttl_ms)
            .await?;
        let presence_flags = if self.cache.is_configured() && !records.is_empty() {
            let keys: Vec<_> = records
                .iter()
                .map(|r| CacheKey::RemoteCliPresence(r.session_id.clone()))
                .collect();
            self.cache
                .get_many(&keys)
                .await
                .ok()
                .map(|vals| vals.into_iter().map(|v| v.is_some()).collect::<Vec<_>>())
        } else {
            None
        };

        Ok(records
            .into_iter()
            .enumerate()
            .map(|(idx, rec)| {
                let valkey_online = presence_flags
                    .as_ref()
                    .and_then(|flags| flags.get(idx).copied())
                    .unwrap_or(false);
                let within_window = rec
                    .last_disconnect_ms
                    .map(|d| now_ms.saturating_sub(d) <= self.reconnect_window_ms)
                    .unwrap_or(false);
                let online = rec.cli_connected || valkey_online || within_window;
                RemoteCliSessionView {
                    session_id: rec.session_id,
                    device_name: rec.device_name,
                    platform: rec.platform,
                    last_seen_ms: rec.last_seen_ms,
                    online,
                }
            })
            .collect())
    }

    pub async fn set_cli_connected(
        &self,
        session_id: &str,
        caller: &str,
        connected: bool,
        now_ms: u64,
    ) -> Result<(), RemoteCliError> {
        self.store
            .set_cli_connected(session_id, caller, connected, now_ms, self.session_ttl_ms)
            .await?;
        self.set_presence(session_id, connected).await;
        Ok(())
    }

    pub async fn send_prompt(
        &self,
        session_id: &str,
        caller: &str,
        prompt: &str,
        file_ids: Vec<String>,
        now_ms: u64,
    ) -> Result<RemoteCliEvent, RemoteCliError> {
        if prompt.trim().is_empty() {
            return Err(RemoteCliError::BadRequest("prompt is required".into()));
        }
        let ev = self
            .store
            .send_prompt(
                session_id,
                caller,
                prompt,
                &file_ids,
                now_ms,
                self.command_ttl_ms,
                self.session_ttl_ms,
            )
            .await?;
        self.notify_cli(session_id, ev.id).await;
        Ok(ev)
    }

    pub async fn cancel_turn(
        &self,
        session_id: &str,
        caller: &str,
        turn_id: Option<String>,
        now_ms: u64,
    ) -> Result<RemoteCliEvent, RemoteCliError> {
        let ev = self
            .store
            .cancel_turn(
                session_id,
                caller,
                turn_id.as_deref(),
                now_ms,
                self.command_ttl_ms,
                self.session_ttl_ms,
            )
            .await?;
        self.notify_cli(session_id, ev.id).await;
        self.notify_ctrl(session_id, 0).await;
        Ok(ev)
    }

    pub async fn post_cli_event(
        &self,
        session_id: &str,
        caller: &str,
        kind: RemoteEventKind,
        payload: serde_json::Value,
        ack_command_id: Option<u64>,
        now_ms: u64,
    ) -> Result<RemoteCliEvent, RemoteCliError> {
        let ev = self
            .store
            .post_cli_event(
                session_id,
                caller,
                kind,
                &payload,
                ack_command_id,
                now_ms,
                self.session_ttl_ms,
            )
            .await?;
        self.set_presence(session_id, true).await;
        self.notify_ctrl(session_id, ev.id).await;
        Ok(ev)
    }

    pub async fn answer_approval(
        &self,
        session_id: &str,
        caller: &str,
        approval_id: &str,
        approved: bool,
        now_ms: u64,
    ) -> Result<RemoteCliEvent, RemoteCliError> {
        if approval_id.trim().is_empty() {
            return Err(RemoteCliError::BadRequest("approval_id is required".into()));
        }
        let ev = self
            .store
            .answer_approval(
                session_id,
                caller,
                approval_id,
                approved,
                now_ms,
                self.command_ttl_ms,
                self.session_ttl_ms,
            )
            .await?;
        self.notify_cli(session_id, ev.id).await;
        self.notify_ctrl(session_id, 0).await;
        Ok(ev)
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn answer_bound_output_action(
        &self,
        session_id: &str,
        caller: &str,
        chat_session_id: Uuid,
        output_id: Uuid,
        action_id: Uuid,
        approved: bool,
        now_ms: u64,
    ) -> Result<RemoteCliEvent, RemoteCliError> {
        let ev = self
            .store
            .answer_bound_output_action(
                session_id,
                caller,
                chat_session_id,
                output_id,
                action_id,
                approved,
                now_ms,
                self.command_ttl_ms,
            )
            .await?;
        self.notify_cli(session_id, ev.id).await;
        self.notify_ctrl(session_id, 0).await;
        Ok(ev)
    }

    pub async fn expire_pending_commands(&self, now_ms: u64) -> Result<usize, RemoteCliError> {
        let touched = self.store.expire_pending_commands(now_ms).await?;
        let count = touched.len();
        for sid in touched {
            self.notify_ctrl(&sid, 0).await;
        }
        Ok(count)
    }

    pub async fn purge_expired_sessions(&self, now_ms: u64) -> Result<usize, RemoteCliError> {
        self.store
            .purge_expired_sessions(now_ms, self.session_ttl_ms)
            .await
    }

    pub async fn cli_commands_since(
        &self,
        session_id: &str,
        caller: &str,
        after_cursor: u64,
    ) -> Result<(Vec<RemoteCliEvent>, broadcast::Receiver<u64>), RemoteCliError> {
        let rx = Self::local_subscribe(&self.local_cli_tx, session_id).await;
        let replay = self
            .store
            .cli_commands_since(session_id, caller, after_cursor)
            .await?;
        Ok((replay, rx))
    }

    pub async fn ctrl_events_since(
        &self,
        session_id: &str,
        caller: &str,
        after_cursor: u64,
    ) -> Result<(Vec<RemoteCliEvent>, broadcast::Receiver<u64>), RemoteCliError> {
        let rx = Self::local_subscribe(&self.local_ctrl_tx, session_id).await;
        let replay = self
            .store
            .ctrl_events_since(session_id, caller, after_cursor)
            .await?;
        Ok((replay, rx))
    }

    pub async fn subscribe_cli_pubsub(&self, session_id: &str) -> Option<redis::aio::PubSub> {
        if !self.cache.is_configured() {
            return None;
        }
        self.cache
            .subscribe(&CacheKey::RemoteCliCliChannel(session_id.to_string()))
            .await
            .ok()
    }

    pub async fn subscribe_ctrl_pubsub(&self, session_id: &str) -> Option<redis::aio::PubSub> {
        if !self.cache.is_configured() {
            return None;
        }
        self.cache
            .subscribe(&CacheKey::RemoteCliCtrlChannel(session_id.to_string()))
            .await
            .ok()
    }
}
