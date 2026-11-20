use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

use serde::{Deserialize, Serialize};

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
pub enum RemoteCliError {
    NotFound,
    Forbidden,
    AlreadyAnswered,
    BadRequest(String),
}

struct PendingCommand {
    event_id: u64,
    expires_at_ms: u64,
    acked: bool,
}

struct SessionState {
    session_id: String,
    owner_user: String,
    device_name: String,
    platform: String,
    last_seen_ms: u64,
    cli_connected: bool,
    last_disconnect_ms: Option<u64>,
    next_cli_seq: u64,
    next_ctrl_seq: u64,
    cli_commands: Vec<RemoteCliEvent>,
    ctrl_events: Vec<RemoteCliEvent>,
    pending_commands: Vec<PendingCommand>,
    answered_approvals: BTreeMap<String, bool>,
    cli_tx: broadcast::Sender<RemoteCliEvent>,
    ctrl_tx: broadcast::Sender<RemoteCliEvent>,
}

impl SessionState {
    fn is_online(&self, now_ms: u64, reconnect_window_ms: u64) -> bool {
        if self.cli_connected {
            return true;
        }
        self.last_disconnect_ms
            .map(|d| now_ms.saturating_sub(d) <= reconnect_window_ms)
            .unwrap_or(false)
    }

    fn push_cli(&mut self, kind: RemoteEventKind, payload: serde_json::Value, ttl_ms: u64, now_ms: u64) -> RemoteCliEvent {
        self.next_cli_seq += 1;
        let ev = RemoteCliEvent {
            id: self.next_cli_seq,
            kind,
            payload,
            created_at_ms: now_ms,
        };
        self.cli_commands.push(ev.clone());
        self.pending_commands.push(PendingCommand {
            event_id: ev.id,
            expires_at_ms: now_ms.saturating_add(ttl_ms),
            acked: false,
        });
        let _ = self.cli_tx.send(ev.clone());
        ev
    }

    fn push_ctrl(&mut self, kind: RemoteEventKind, payload: serde_json::Value, now_ms: u64) -> RemoteCliEvent {
        self.next_ctrl_seq += 1;
        let ev = RemoteCliEvent {
            id: self.next_ctrl_seq,
            kind,
            payload,
            created_at_ms: now_ms,
        };
        self.ctrl_events.push(ev.clone());
        let _ = self.ctrl_tx.send(ev.clone());
        ev
    }
}

#[derive(Clone)]
pub struct RemoteCliService {
    sessions: Arc<RwLock<HashMap<String, SessionState>>>,
    reconnect_window_ms: u64,
    command_ttl_ms: u64,
}

impl Default for RemoteCliService {
    fn default() -> Self {
        Self::new(15_000, 10_000)
    }
}

impl RemoteCliService {
    pub fn new(reconnect_window_ms: u64, command_ttl_ms: u64) -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
            reconnect_window_ms,
            command_ttl_ms,
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
            return Err(RemoteCliError::BadRequest("device_name and platform are required".into()));
        }
        let session_id = Uuid::new_v4().to_string();
        let (cli_tx, _) = broadcast::channel(64);
        let (ctrl_tx, _) = broadcast::channel(64);
        let state = SessionState {
            session_id: session_id.clone(),
            owner_user: owner_user.to_string(),
            device_name: device_name.trim().to_string(),
            platform: platform.trim().to_string(),
            last_seen_ms: now_ms,
            cli_connected: true,
            last_disconnect_ms: None,
            next_cli_seq: 0,
            next_ctrl_seq: 0,
            cli_commands: Vec::new(),
            ctrl_events: Vec::new(),
            pending_commands: Vec::new(),
            answered_approvals: BTreeMap::new(),
            cli_tx,
            ctrl_tx,
        };
        let view = RemoteCliSessionView {
            session_id: state.session_id.clone(),
            device_name: state.device_name.clone(),
            platform: state.platform.clone(),
            last_seen_ms: state.last_seen_ms,
            online: true,
        };
        self.sessions.write().await.insert(session_id, state);
        Ok(view)
    }

    pub async fn unregister(&self, session_id: &str, caller: &str) -> Result<(), RemoteCliError> {
        let mut guard = self.sessions.write().await;
        let s = guard.get(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        guard.remove(session_id);
        Ok(())
    }

    pub async fn list_sessions(&self, caller: &str, now_ms: u64) -> Vec<RemoteCliSessionView> {
        let guard = self.sessions.read().await;
        guard
            .values()
            .filter(|s| s.owner_user == caller)
            .map(|s| RemoteCliSessionView {
                session_id: s.session_id.clone(),
                device_name: s.device_name.clone(),
                platform: s.platform.clone(),
                last_seen_ms: s.last_seen_ms,
                online: s.is_online(now_ms, self.reconnect_window_ms),
            })
            .collect()
    }

    pub async fn set_cli_connected(
        &self,
        session_id: &str,
        caller: &str,
        connected: bool,
        now_ms: u64,
    ) -> Result<(), RemoteCliError> {
        let mut guard = self.sessions.write().await;
        let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        s.cli_connected = connected;
        s.last_seen_ms = now_ms;
        if !connected {
            s.last_disconnect_ms = Some(now_ms);
        }
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
        let mut guard = self.sessions.write().await;
        let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        let ev = s.push_cli(
            RemoteEventKind::Prompt,
            serde_json::json!({ "prompt": prompt, "file_ids": file_ids }),
            self.command_ttl_ms,
            now_ms,
        );
        Ok(ev)
    }

    pub async fn cancel_turn(
        &self,
        session_id: &str,
        caller: &str,
        turn_id: Option<String>,
        now_ms: u64,
    ) -> Result<RemoteCliEvent, RemoteCliError> {
        let mut guard = self.sessions.write().await;
        let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        let payload = serde_json::json!({ "turn_id": turn_id });
        let ev = s.push_cli(RemoteEventKind::Cancel, payload.clone(), self.command_ttl_ms, now_ms);
        s.push_ctrl(RemoteEventKind::Cancel, payload, now_ms);
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
        let mut guard = self.sessions.write().await;
        let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        s.last_seen_ms = now_ms;
        if let Some(cmd_id) = ack_command_id {
            for p in &mut s.pending_commands {
                if p.event_id == cmd_id {
                    p.acked = true;
                }
            }
        }
        Ok(s.push_ctrl(kind, payload, now_ms))
    }

    pub async fn answer_approval(
        &self,
        session_id: &str,
        caller: &str,
        approval_id: &str,
        approved: bool,
        now_ms: u64,
    ) -> Result<RemoteCliEvent, RemoteCliError> {
        let mut guard = self.sessions.write().await;
        let s = guard.get_mut(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        if s.answered_approvals.contains_key(approval_id) {
            return Err(RemoteCliError::AlreadyAnswered);
        }
        s.answered_approvals.insert(approval_id.to_string(), approved);
        let payload = serde_json::json!({ "approval_id": approval_id, "approved": approved });
        let ev = s.push_cli(
            RemoteEventKind::ApprovalAnswer,
            payload.clone(),
            self.command_ttl_ms,
            now_ms,
        );
        s.push_ctrl(RemoteEventKind::ApprovalAnswer, payload, now_ms);
        Ok(ev)
    }

    pub async fn expire_pending_commands(&self, now_ms: u64) -> usize {
        let mut guard = self.sessions.write().await;
        let mut expired_total = 0;
        for s in guard.values_mut() {
            let mut newly_expired = Vec::new();
            for p in &mut s.pending_commands {
                if !p.acked && now_ms > p.expires_at_ms {
                    p.acked = true;
                    newly_expired.push(p.event_id);
                }
            }
            for cmd_id in newly_expired {
                expired_total += 1;
                s.push_ctrl(
                    RemoteEventKind::CommandExpired,
                    serde_json::json!({ "command_id": cmd_id, "status": "not_delivered" }),
                    now_ms,
                );
            }
        }
        expired_total
    }

    pub async fn cli_commands_since(
        &self,
        session_id: &str,
        caller: &str,
        after_cursor: u64,
    ) -> Result<(Vec<RemoteCliEvent>, broadcast::Receiver<RemoteCliEvent>), RemoteCliError> {
        let guard = self.sessions.read().await;
        let s = guard.get(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        let replay: Vec<RemoteCliEvent> = s
            .cli_commands
            .iter()
            .filter(|e| e.id > after_cursor)
            .cloned()
            .collect();
        Ok((replay, s.cli_tx.subscribe()))
    }

    pub async fn ctrl_events_since(
        &self,
        session_id: &str,
        caller: &str,
        after_cursor: u64,
    ) -> Result<(Vec<RemoteCliEvent>, broadcast::Receiver<RemoteCliEvent>), RemoteCliError> {
        let guard = self.sessions.read().await;
        let s = guard.get(session_id).ok_or(RemoteCliError::NotFound)?;
        if s.owner_user != caller {
            return Err(RemoteCliError::Forbidden);
        }
        let replay: Vec<RemoteCliEvent> = s
            .ctrl_events
            .iter()
            .filter(|e| e.id > after_cursor)
            .cloned()
            .collect();
        Ok((replay, s.ctrl_tx.subscribe()))
    }
}
