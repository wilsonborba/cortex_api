use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

use chrono::{DateTime, Utc};
use tokio::sync::{RwLock, watch};
use uuid::Uuid;

type ActiveChildTasks = Arc<std::sync::Mutex<HashMap<(Uuid, Uuid), watch::Sender<bool>>>>;
type ActiveChildBudgets =
    Arc<std::sync::Mutex<HashMap<(Uuid, Uuid), Arc<crate::domain::models::thinking::TokenBudget>>>>;

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildRunStatus {
    Queued,
    Running,
    Completed,
    Failed,
    Cancelled,
}
impl ChildRunStatus {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Running => "running",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::Cancelled => "cancelled",
        }
    }
    pub fn terminal(self) -> bool {
        matches!(self, Self::Completed | Self::Failed | Self::Cancelled)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ChildEventKind {
    Queued,
    Progress,
    Completed,
    Failed,
    CancelRequested,
    Cancelled,
    ActionRequested,
    ActionAnswered,
}
impl ChildEventKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Queued => "queued",
            Self::Progress => "progress",
            Self::Completed => "completed",
            Self::Failed => "failed",
            Self::CancelRequested => "cancel_requested",
            Self::Cancelled => "cancelled",
            Self::ActionRequested => "action_requested",
            Self::ActionAnswered => "action_answered",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildRunError {
    NotFound,
    Forbidden,
    Conflict,
    Invalid,
    Unavailable,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ChildRun {
    pub session_id: Uuid,
    pub child_id: Uuid,
    pub parent_run_id: String,
    pub parent_chat_session_id: Option<Uuid>,
    pub parent_output_id: Option<Uuid>,
    pub parent_call_id: Option<Uuid>,
    pub child_chat_session_id: Option<Uuid>,
    pub child_output_id: Option<Uuid>,
    pub role: String,
    pub task: String,
    pub budget_tokens: u32,
    pub depth: u8,
    pub status: ChildRunStatus,
    pub cancel_requested: bool,
    pub budget_used_tokens: u64,
    pub budget_reserved_tokens: u64,
    pub budget_settled: bool,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct BoundChildRequest {
    pub parent_chat_session_id: Uuid,
    pub parent_output_id: Uuid,
    pub parent_call_id: Uuid,
    pub parent_budget_used_tokens: u64,
    pub role: String,
    pub task: String,
    pub budget_tokens: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct ChildRunEvent {
    pub seq: u64,
    pub event_id: Uuid,
    pub kind: ChildEventKind,
    pub message: Option<String>,
    pub result: Option<String>,
    pub error_code: Option<String>,
    pub action_id: Option<Uuid>,
    pub action_kind: Option<String>,
    pub payload_digest: Option<String>,
    pub display: Option<String>,
    pub approved: Option<bool>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone)]
pub struct PendingChildAction {
    pub action_id: Uuid,
    pub parent_run_id: String,
    pub action_kind: String,
    pub payload_digest: String,
    pub display: String,
    pub expires_at: DateTime<Utc>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct OutputAction {
    pub action_id: Uuid,
    pub chat_session_id: Uuid,
    pub output_id: Uuid,
    pub call_id: Uuid,
    pub tool_name: String,
    pub action_kind: String,
    pub action_digest: String,
    pub display: String,
    pub expires_at: DateTime<Utc>,
    pub approved: Option<bool>,
    pub cancelled_at: Option<DateTime<Utc>>,
    pub claimed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
}

impl OutputAction {
    pub fn status(&self, now: DateTime<Utc>) -> &'static str {
        if self.claimed_at.is_some() {
            "claimed"
        } else if self.cancelled_at.is_some() {
            "cancelled"
        } else if self.expires_at <= now {
            "expired"
        } else {
            match self.approved {
                Some(true) => "approved",
                Some(false) => "denied",
                None => "pending",
            }
        }
    }
}

type EventPageFuture<'a> = Pin<
    Box<dyn Future<Output = Result<(ChildRun, Vec<ChildRunEvent>), ChildRunError>> + Send + 'a>,
>;

pub trait AgentRunStore: Send + Sync {
    fn child_for_output<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        chat_session_id: Uuid,
        output_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ChildRun>, ChildRunError>> + Send + 'a>>;
    #[allow(clippy::too_many_arguments)]
    fn create_bound_child<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        request: BoundChildRequest,
        max_active: usize,
        max_depth: u8,
        max_budget_tokens: u32,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRun, ChildRunError>> + Send + 'a>>;
    fn bind_child_output<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        child_chat_session_id: Uuid,
        child_output_id: Uuid,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRun, ChildRunError>> + Send + 'a>>;
    fn report_child_usage<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        used_tokens: u64,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ChildRunError>> + Send + 'a>>;
    fn settle_child_budget<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        used_tokens: u64,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ChildRunError>> + Send + 'a>>;
    #[allow(clippy::too_many_arguments)]
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
    ) -> Pin<Box<dyn Future<Output = Result<ChildRun, ChildRunError>> + Send + 'a>>;
    #[allow(clippy::too_many_arguments)]
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
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>>;
    fn request_cancel<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        event_id: Uuid,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>>;
    fn create_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        action: PendingChildAction,
        now: DateTime<Utc>,
        event_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>>;
    #[allow(clippy::too_many_arguments)]
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
    ) -> Pin<Box<dyn Future<Output = Result<ChildRunEvent, ChildRunError>> + Send + 'a>>;
    fn create_output_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        action: OutputAction,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>>;
    fn output_action<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        chat_session_id: Uuid,
        output_id: Uuid,
        action_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>>;
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
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>>;
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
    ) -> Pin<Box<dyn Future<Output = Result<OutputAction, ChildRunError>> + Send + 'a>>;
    fn cancel_output_actions<'a>(
        &'a self,
        owner: &'a str,
        chat_session_id: Uuid,
        output_id: Uuid,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<(), ChildRunError>> + Send + 'a>>;
    fn events_after<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        after: u64,
        limit: usize,
    ) -> EventPageFuture<'a>;
}

#[derive(Default)]
struct MemoryChild {
    run: Option<ChildRun>,
    next_seq: u64,
    events: Vec<ChildRunEvent>,
    actions: HashMap<Uuid, (PendingChildAction, Option<bool>)>,
}

#[derive(Default)]
pub struct InMemoryAgentRunStore {
    owners: RwLock<HashMap<Uuid, String>>,
    children: RwLock<HashMap<(Uuid, Uuid), MemoryChild>>,
    output_actions: RwLock<HashMap<(Uuid, Uuid), OutputAction>>,
}
impl InMemoryAgentRunStore {
    pub async fn register_session(&self, session_id: Uuid, owner: &str) {
        self.owners.write().await.insert(session_id, owner.into());
    }
}

fn check_owner(
    owners: &HashMap<Uuid, String>,
    owner: &str,
    session: Uuid,
) -> Result<(), ChildRunError> {
    match owners.get(&session) {
        None => Err(ChildRunError::NotFound),
        Some(found) if found != owner => Err(ChildRunError::Forbidden),
        Some(_) => Ok(()),
    }
}

impl InMemoryAgentRunStore {
    async fn append_locked(
        child: &mut MemoryChild,
        event_id: Uuid,
        kind: ChildEventKind,
        message: Option<&str>,
        result: Option<&str>,
        error_code: Option<&str>,
        now: DateTime<Utc>,
    ) -> Result<ChildRunEvent, ChildRunError> {
        if let Some(old) = child.events.iter().find(|e| e.event_id == event_id) {
            if old.kind == kind
                && old.message.as_deref() == message
                && old.result.as_deref() == result
                && old.error_code.as_deref() == error_code
            {
                return Ok(old.clone());
            }
            return Err(ChildRunError::Conflict);
        }
        let run = child.run.as_mut().ok_or(ChildRunError::NotFound)?;
        if run.status.terminal() {
            return Err(ChildRunError::Conflict);
        }
        match kind {
            ChildEventKind::Queued => return Err(ChildRunError::Invalid),
            ChildEventKind::Progress => {
                if run.cancel_requested {
                    return Err(ChildRunError::Conflict);
                }
                run.status = ChildRunStatus::Running;
            }
            ChildEventKind::Completed => {
                if run.cancel_requested {
                    return Err(ChildRunError::Conflict);
                }
                run.status = ChildRunStatus::Completed;
            }
            ChildEventKind::Failed => {
                run.status = ChildRunStatus::Failed;
            }
            ChildEventKind::Cancelled => {
                if !run.cancel_requested {
                    return Err(ChildRunError::Conflict);
                }
                run.status = ChildRunStatus::Cancelled;
            }
            _ => return Err(ChildRunError::Invalid),
        }
        run.updated_at = now;
        let event = ChildRunEvent {
            seq: child.next_seq,
            event_id,
            kind,
            message: message.map(str::to_owned),
            result: result.map(str::to_owned),
            error_code: error_code.map(str::to_owned),
            action_id: None,
            action_kind: None,
            payload_digest: None,
            display: None,
            approved: None,
            created_at: now,
        };
        child.next_seq += 1;
        child.events.push(event.clone());
        Ok(event)
    }
}

impl AgentRunStore for InMemoryAgentRunStore {
    fn child_for_output<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        chat_session_id: Uuid,
        output_id: Uuid,
    ) -> Pin<Box<dyn Future<Output = Result<Option<ChildRun>, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            check_owner(&*self.owners.read().await, owner, session_id)?;
            Ok(self
                .children
                .read()
                .await
                .values()
                .filter_map(|child| child.run.as_ref())
                .find(|run| {
                    run.session_id == session_id
                        && run.child_chat_session_id == Some(chat_session_id)
                        && run.child_output_id == Some(output_id)
                })
                .cloned())
        })
    }

    fn create_bound_child<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        request: BoundChildRequest,
        max_active: usize,
        max_depth: u8,
        max_budget_tokens: u32,
        now: DateTime<Utc>,
    ) -> Pin<Box<dyn Future<Output = Result<ChildRun, ChildRunError>> + Send + 'a>> {
        Box::pin(async move {
            check_owner(&*self.owners.read().await, owner, session_id)?;
            if request.role.trim().is_empty()
                || request.role.len() > 80
                || request.task.trim().is_empty()
                || request.task.len() > 32_000
                || request.budget_tokens == 0
                || request.budget_tokens > max_budget_tokens.max(1)
            {
                return Err(ChildRunError::Invalid);
            }
            let mut children = self.children.write().await;
            if let Some(existing) = children
                .values()
                .filter_map(|child| child.run.as_ref())
                .find(|run| {
                    run.parent_output_id == Some(request.parent_output_id)
                        && run.parent_call_id == Some(request.parent_call_id)
                })
            {
                if existing.session_id == session_id
                    && existing.parent_chat_session_id == Some(request.parent_chat_session_id)
                    && existing.role == request.role
                    && existing.task == request.task
                {
                    return Ok(existing.clone());
                }
                return Err(ChildRunError::Conflict);
            }
            let active = children
                .values()
                .filter_map(|child| child.run.as_ref())
                .filter(|run| run.session_id == session_id && !run.status.terminal())
                .count();
            if active >= max_active.max(1) {
                return Err(ChildRunError::Conflict);
            }
            let parent_id = children
                .values()
                .filter_map(|child| child.run.as_ref())
                .find(|run| {
                    run.session_id == session_id
                        && run.child_chat_session_id == Some(request.parent_chat_session_id)
                        && run.child_output_id == Some(request.parent_output_id)
                })
                .map(|run| run.child_id);
            let (parent_run_id, depth, budget_tokens, parent_usage) =
                if let Some(parent_id) = parent_id {
                    let parent = children
                        .get(&(session_id, parent_id))
                        .and_then(|child| child.run.as_ref())
                        .ok_or(ChildRunError::NotFound)?;
                    if parent.status.terminal() || parent.cancel_requested {
                        return Err(ChildRunError::Conflict);
                    }
                    let parent_usage = parent
                        .budget_used_tokens
                        .max(request.parent_budget_used_tokens);
                    let allocated = u64::from(parent.budget_tokens)
                        .saturating_sub(parent_usage)
                        .saturating_sub(parent.budget_reserved_tokens);
                    let budget = request
                        .budget_tokens
                        .min(u32::try_from(allocated).unwrap_or(u32::MAX));
                    let depth = parent.depth.saturating_add(1);
                    if budget == 0 || depth > max_depth.max(1) {
                        return Err(ChildRunError::Conflict);
                    }
                    (parent_id.to_string(), depth, budget, parent_usage)
                } else {
                    (session_id.to_string(), 1, request.budget_tokens, 0)
                };
            let child_id = Uuid::now_v7();
            let run = ChildRun {
                session_id,
                child_id,
                parent_run_id: parent_run_id.clone(),
                parent_chat_session_id: Some(request.parent_chat_session_id),
                parent_output_id: Some(request.parent_output_id),
                parent_call_id: Some(request.parent_call_id),
                child_chat_session_id: None,
                child_output_id: None,
                role: request.role,
                task: request.task,
                budget_tokens,
                depth,
                status: ChildRunStatus::Queued,
                cancel_requested: false,
                budget_used_tokens: 0,
                budget_reserved_tokens: 0,
                budget_settled: false,
                created_at: now,
                updated_at: now,
            };
            if let Some(parent_id) = parent_id {
                let parent = children
                    .get_mut(&(session_id, parent_id))
                    .and_then(|child| child.run.as_mut())
                    .ok_or(ChildRunError::NotFound)?;
                parent.budget_reserved_tokens = parent
                    .budget_reserved_tokens
                    .saturating_add(u64::from(budget_tokens));
                parent.budget_used_tokens = parent.budget_used_tokens.max(parent_usage);
                parent.updated_at = now;
            }
            let mut child = MemoryChild {
                run: Some(run.clone()),
                next_seq: 2,
                ..Default::default()
            };
            child.events.push(ChildRunEvent {
                seq: 1,
                event_id: Uuid::now_v7(),
                kind: ChildEventKind::Queued,
                message: None,
                result: None,
                error_code: None,
                action_id: None,
                action_kind: None,
                payload_digest: None,
                display: None,
                approved: None,
                created_at: now,
            });
            children.insert((session_id, child_id), child);
            Ok(run)
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut children = self.children.write().await;
            let child = children
                .get_mut(&(session_id, child_id))
                .ok_or(ChildRunError::NotFound)?;
            let run = child.run.as_mut().ok_or(ChildRunError::NotFound)?;
            match (run.child_chat_session_id, run.child_output_id) {
                (None, None) => {
                    run.child_chat_session_id = Some(child_chat_session_id);
                    run.child_output_id = Some(child_output_id);
                    run.updated_at = now;
                    Ok(run.clone())
                }
                (Some(chat), Some(output))
                    if chat == child_chat_session_id && output == child_output_id =>
                {
                    Ok(run.clone())
                }
                _ => Err(ChildRunError::Conflict),
            }
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut children = self.children.write().await;
            let child = children
                .get_mut(&(session_id, child_id))
                .ok_or(ChildRunError::NotFound)?;
            let run = child.run.as_mut().ok_or(ChildRunError::NotFound)?;
            if run.budget_settled
                || used_tokens < run.budget_used_tokens
                || used_tokens.saturating_add(run.budget_reserved_tokens)
                    > u64::from(run.budget_tokens)
            {
                return Err(ChildRunError::Conflict);
            }
            run.budget_used_tokens = used_tokens;
            run.updated_at = now;
            Ok(())
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut children = self.children.write().await;
            let child = children
                .get(&(session_id, child_id))
                .ok_or(ChildRunError::NotFound)?;
            let run = child.run.as_ref().ok_or(ChildRunError::NotFound)?;
            if run.budget_settled {
                return if run.budget_used_tokens == used_tokens {
                    Ok(())
                } else {
                    Err(ChildRunError::Conflict)
                };
            }
            if used_tokens < run.budget_used_tokens || used_tokens > u64::from(run.budget_tokens) {
                return Err(ChildRunError::Conflict);
            }
            let (parent_chat, parent_output, allocation) = (
                run.parent_chat_session_id,
                run.parent_output_id,
                u64::from(run.budget_tokens),
            );
            let parent_id = parent_chat.zip(parent_output).and_then(|(chat, output)| {
                children
                    .values()
                    .filter_map(|candidate| candidate.run.as_ref())
                    .find(|candidate| {
                        candidate.session_id == session_id
                            && candidate.child_chat_session_id == Some(chat)
                            && candidate.child_output_id == Some(output)
                    })
                    .map(|candidate| candidate.child_id)
            });
            if let Some(parent_id) = parent_id {
                let parent = children
                    .get(&(session_id, parent_id))
                    .and_then(|candidate| candidate.run.as_ref())
                    .ok_or(ChildRunError::NotFound)?;
                if parent.budget_reserved_tokens < allocation
                    || parent
                        .budget_used_tokens
                        .saturating_add(used_tokens)
                        .saturating_add(parent.budget_reserved_tokens - allocation)
                        > u64::from(parent.budget_tokens)
                {
                    return Err(ChildRunError::Conflict);
                }
            }
            {
                let run = children
                    .get_mut(&(session_id, child_id))
                    .and_then(|c| c.run.as_mut())
                    .ok_or(ChildRunError::NotFound)?;
                run.budget_used_tokens = used_tokens;
                run.budget_reserved_tokens = 0;
                run.budget_settled = true;
                run.updated_at = now;
            }
            if let Some(parent_id) = parent_id {
                let parent = children
                    .get_mut(&(session_id, parent_id))
                    .and_then(|candidate| candidate.run.as_mut())
                    .ok_or(ChildRunError::NotFound)?;
                parent.budget_reserved_tokens -= allocation;
                parent.budget_used_tokens = parent.budget_used_tokens.saturating_add(used_tokens);
                parent.updated_at = now;
            }
            Ok(())
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            if budget_tokens == 0 || budget_tokens > max_budget_tokens.max(1) {
                return Err(ChildRunError::Invalid);
            }
            let mut children = self.children.write().await;
            let active = children
                .values()
                .filter_map(|child| child.run.as_ref())
                .filter(|run| run.session_id == session_id && !run.status.terminal())
                .count();
            if active >= max_active.max(1) {
                return Err(ChildRunError::Conflict);
            }
            let depth = if parent_run_id != session_id.to_string() {
                let parent_id =
                    Uuid::parse_str(parent_run_id).map_err(|_| ChildRunError::Invalid)?;
                let parent = children
                    .get(&(session_id, parent_id))
                    .and_then(|child| child.run.as_ref())
                    .ok_or(ChildRunError::NotFound)?;
                if parent.status.terminal() || parent.cancel_requested {
                    return Err(ChildRunError::Conflict);
                }
                if budget_tokens > parent.budget_tokens {
                    return Err(ChildRunError::Invalid);
                }
                let allocated: u64 = children
                    .values()
                    .filter_map(|child| child.run.as_ref())
                    .filter(|run| {
                        run.session_id == session_id && run.parent_run_id == parent_run_id
                    })
                    .map(|run| u64::from(run.budget_tokens))
                    .sum();
                if allocated.saturating_add(u64::from(budget_tokens))
                    > u64::from(parent.budget_tokens)
                {
                    return Err(ChildRunError::Conflict);
                }
                parent.depth.saturating_add(1)
            } else {
                1
            };
            if depth > max_depth.max(1) {
                return Err(ChildRunError::Conflict);
            }
            let child_id = Uuid::now_v7();
            let run = ChildRun {
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
            };
            let mut child = MemoryChild {
                run: Some(run.clone()),
                next_seq: 1,
                ..Default::default()
            };
            child.events.push(ChildRunEvent {
                seq: 1,
                event_id: Uuid::now_v7(),
                kind: ChildEventKind::Queued,
                message: None,
                result: None,
                error_code: None,
                action_id: None,
                action_kind: None,
                payload_digest: None,
                display: None,
                approved: None,
                created_at: now,
            });
            child.next_seq = 2;
            children.insert((session_id, child_id), child);
            Ok(run)
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut children = self.children.write().await;
            let child = children
                .get_mut(&(session_id, child_id))
                .ok_or(ChildRunError::NotFound)?;
            Self::append_locked(child, event_id, kind, message, result, error_code, now).await
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut children = self.children.write().await;
            if !children.contains_key(&(session_id, child_id)) {
                return Err(ChildRunError::NotFound);
            }
            let mut subtree = vec![child_id];
            let mut cursor = 0;
            while cursor < subtree.len() {
                let parent = subtree[cursor].to_string();
                cursor += 1;
                for ((sid, id), child) in children.iter() {
                    if *sid == session_id
                        && child
                            .run
                            .as_ref()
                            .is_some_and(|run| run.parent_run_id == parent)
                        && !subtree.contains(id)
                    {
                        subtree.push(*id);
                    }
                }
            }
            let mut root_event = None;
            for id in subtree {
                let child = children
                    .get_mut(&(session_id, id))
                    .ok_or(ChildRunError::NotFound)?;
                let run = child.run.as_mut().ok_or(ChildRunError::NotFound)?;
                if run.status.terminal() {
                    continue;
                }
                if run.cancel_requested {
                    if id == child_id {
                        root_event = child
                            .events
                            .iter()
                            .find(|e| e.kind == ChildEventKind::CancelRequested)
                            .cloned();
                    }
                    continue;
                }
                run.cancel_requested = true;
                run.updated_at = now;
                let mut invalidated = vec![];
                for (action_id, (action, decision)) in &mut child.actions {
                    if decision.is_none() {
                        *decision = Some(false);
                        invalidated.push((*action_id, action.clone()));
                    }
                }
                for (action_id, action) in invalidated {
                    let event = ChildRunEvent {
                        seq: child.next_seq,
                        event_id: Uuid::now_v7(),
                        kind: ChildEventKind::ActionAnswered,
                        message: None,
                        result: None,
                        error_code: None,
                        action_id: Some(action_id),
                        action_kind: Some(action.action_kind),
                        payload_digest: Some(action.payload_digest),
                        display: None,
                        approved: Some(false),
                        created_at: now,
                    };
                    child.next_seq += 1;
                    child.events.push(event);
                }
                let event = ChildRunEvent {
                    seq: child.next_seq,
                    event_id: if id == child_id {
                        event_id
                    } else {
                        Uuid::now_v7()
                    },
                    kind: ChildEventKind::CancelRequested,
                    message: None,
                    result: None,
                    error_code: None,
                    action_id: None,
                    action_kind: None,
                    payload_digest: None,
                    display: None,
                    approved: None,
                    created_at: now,
                };
                child.next_seq += 1;
                child.events.push(event.clone());
                if id == child_id {
                    root_event = Some(event);
                }
            }
            root_event.ok_or(ChildRunError::Conflict)
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut children = self.children.write().await;
            let child = children
                .get_mut(&(session_id, child_id))
                .ok_or(ChildRunError::NotFound)?;
            let run = child.run.as_mut().ok_or(ChildRunError::NotFound)?;
            if run.status.terminal()
                || run.cancel_requested
                || run.parent_run_id != action.parent_run_id
                || action.expires_at <= now
            {
                return Err(ChildRunError::Conflict);
            }
            if child.actions.contains_key(&action.action_id) {
                return Err(ChildRunError::Conflict);
            }
            let event = ChildRunEvent {
                seq: child.next_seq,
                event_id,
                kind: ChildEventKind::ActionRequested,
                message: None,
                result: None,
                error_code: None,
                action_id: Some(action.action_id),
                action_kind: Some(action.action_kind.clone()),
                payload_digest: Some(action.payload_digest.clone()),
                display: Some(action.display.clone()),
                approved: None,
                created_at: now,
            };
            child.next_seq += 1;
            child.actions.insert(action.action_id, (action, None));
            child.events.push(event.clone());
            Ok(event)
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut children = self.children.write().await;
            let child = children
                .get_mut(&(session_id, child_id))
                .ok_or(ChildRunError::NotFound)?;
            let (action, decision) = child
                .actions
                .get_mut(&action_id)
                .ok_or(ChildRunError::NotFound)?;
            if decision.is_some() || action.expires_at <= now || action.payload_digest != digest {
                return Err(ChildRunError::Conflict);
            }
            let run = child.run.as_ref().ok_or(ChildRunError::NotFound)?;
            if run.status.terminal() || run.cancel_requested {
                return Err(ChildRunError::Conflict);
            }
            *decision = Some(approved);
            let event = ChildRunEvent {
                seq: child.next_seq,
                event_id,
                kind: ChildEventKind::ActionAnswered,
                message: None,
                result: None,
                error_code: None,
                action_id: Some(action_id),
                action_kind: Some(action.action_kind.clone()),
                payload_digest: Some(action.payload_digest.clone()),
                display: None,
                approved: Some(approved),
                created_at: now,
            };
            child.next_seq += 1;
            child.events.push(event.clone());
            Ok(event)
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            if action.expires_at <= now {
                return Err(ChildRunError::Conflict);
            }
            let mut actions = self.output_actions.write().await;
            if let Some(((stored_session, _), existing)) = actions.iter().find(|(_, existing)| {
                existing.output_id == action.output_id && existing.call_id == action.call_id
            }) {
                if *stored_session == session_id
                    && existing.chat_session_id == action.chat_session_id
                    && existing.tool_name == action.tool_name
                    && existing.action_kind == action.action_kind
                    && existing.action_digest == action.action_digest
                    && existing.display == action.display
                {
                    return Ok(existing.clone());
                }
                return Err(ChildRunError::Conflict);
            }
            if actions.contains_key(&(session_id, action.action_id)) {
                return Err(ChildRunError::Conflict);
            }
            actions.insert((session_id, action.action_id), action.clone());
            Ok(action)
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            self.output_actions
                .read()
                .await
                .get(&(session_id, action_id))
                .filter(|action| {
                    action.chat_session_id == chat_session_id && action.output_id == output_id
                })
                .cloned()
                .ok_or(ChildRunError::NotFound)
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut actions = self.output_actions.write().await;
            let action = actions
                .get_mut(&(session_id, action_id))
                .filter(|action| {
                    action.chat_session_id == chat_session_id && action.output_id == output_id
                })
                .ok_or(ChildRunError::NotFound)?;
            if action.approved.is_some()
                || action.cancelled_at.is_some()
                || action.claimed_at.is_some()
                || action.expires_at <= now
            {
                return Err(ChildRunError::Conflict);
            }
            action.approved = Some(approved);
            Ok(action.clone())
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let mut actions = self.output_actions.write().await;
            let action = actions
                .get_mut(&(session_id, action_id))
                .filter(|action| {
                    action.chat_session_id == chat_session_id && action.output_id == output_id
                })
                .ok_or(ChildRunError::NotFound)?;
            if action.approved != Some(true)
                || action.cancelled_at.is_some()
                || action.claimed_at.is_some()
                || action.expires_at <= now
                || action.call_id != call_id
                || action.action_digest != digest
            {
                return Err(ChildRunError::Conflict);
            }
            action.claimed_at = Some(now);
            Ok(action.clone())
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
            let owners = self.owners.read().await;
            let mut actions = self.output_actions.write().await;
            for ((session_id, _), action) in actions.iter_mut() {
                if action.chat_session_id == chat_session_id
                    && action.output_id == output_id
                    && action.claimed_at.is_none()
                    && owners.get(session_id).is_some_and(|stored| stored == owner)
                {
                    action.cancelled_at = Some(now);
                }
            }
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
            check_owner(&*self.owners.read().await, owner, session_id)?;
            let children = self.children.read().await;
            let child = children
                .get(&(session_id, child_id))
                .ok_or(ChildRunError::NotFound)?;
            let run = child.run.clone().ok_or(ChildRunError::NotFound)?;
            Ok((
                run,
                child
                    .events
                    .iter()
                    .filter(|e| e.seq > after)
                    .take(limit.max(1))
                    .cloned()
                    .collect(),
            ))
        })
    }
}

#[derive(Clone)]
pub struct AgentRunService {
    pub store: Arc<dyn AgentRunStore>,
    pub action_ttl_ms: u64,
    pub max_active_children: usize,
    pub max_depth: u8,
    pub max_budget_tokens: u32,
    active_child_tasks: ActiveChildTasks,
    active_child_budgets: ActiveChildBudgets,
}
impl AgentRunService {
    pub fn new(
        store: Arc<dyn AgentRunStore>,
        action_ttl_ms: u64,
        max_active_children: usize,
        max_depth: u8,
        max_budget_tokens: u32,
    ) -> Self {
        Self {
            store,
            action_ttl_ms: action_ttl_ms.clamp(1_000, 600_000),
            max_active_children: max_active_children.clamp(1, 16),
            max_depth: max_depth.clamp(1, 4),
            max_budget_tokens: max_budget_tokens.clamp(256, 65_536),
            active_child_tasks: Arc::new(std::sync::Mutex::new(HashMap::new())),
            active_child_budgets: Arc::new(std::sync::Mutex::new(HashMap::new())),
        }
    }

    pub fn register_child_task(
        &self,
        session_id: Uuid,
        child_id: Uuid,
        cancel: tokio::sync::watch::Sender<bool>,
    ) -> bool {
        let mut tasks = self
            .active_child_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        if tasks.contains_key(&(session_id, child_id)) {
            return false;
        }
        tasks.insert((session_id, child_id), cancel);
        true
    }

    pub fn unregister_child_task(&self, session_id: Uuid, child_id: Uuid) {
        self.active_child_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(session_id, child_id));
        self.active_child_budgets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&(session_id, child_id));
    }

    pub fn register_child_budget(
        &self,
        session_id: Uuid,
        child_id: Uuid,
        budget: Arc<crate::domain::models::thinking::TokenBudget>,
    ) {
        self.active_child_budgets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert((session_id, child_id), budget);
    }

    pub fn child_budget_usage(&self, session_id: Uuid, child_id: Uuid) -> Option<u64> {
        self.active_child_budgets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(session_id, child_id))
            .map(|budget| budget.used())
    }

    pub fn active_child_budget(
        &self,
        session_id: Uuid,
        child_id: Uuid,
    ) -> Option<Arc<crate::domain::models::thinking::TokenBudget>> {
        self.active_child_budgets
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(session_id, child_id))
            .cloned()
    }

    pub fn cancel_child_task(&self, session_id: Uuid, child_id: Uuid) {
        if let Some(cancel) = self
            .active_child_tasks
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&(session_id, child_id))
        {
            let _ = cancel.send(true);
        }
    }
    pub fn in_memory(action_ttl_ms: u64) -> Self {
        Self::new(
            Arc::new(InMemoryAgentRunStore::default()),
            action_ttl_ms,
            4,
            2,
            8192,
        )
    }
    pub fn in_memory_with_store(action_ttl_ms: u64) -> (Self, Arc<InMemoryAgentRunStore>) {
        let store = Arc::new(InMemoryAgentRunStore::default());
        (Self::new(store.clone(), action_ttl_ms, 4, 2, 8192), store)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    fn action(
        action_id: Uuid,
        parent: &str,
        now: DateTime<Utc>,
        digest: &str,
    ) -> PendingChildAction {
        PendingChildAction {
            action_id,
            parent_run_id: parent.into(),
            action_kind: "command".into(),
            payload_digest: digest.into(),
            display: "cwd: workspace; command: cargo test".into(),
            expires_at: now + Duration::seconds(30),
        }
    }

    #[tokio::test]
    async fn bound_child_creation_is_idempotent_and_reserves_nested_budget_once() {
        let (service, store) = AgentRunService::in_memory_with_store(30_000);
        let _ = service;
        let session = Uuid::now_v7();
        store.register_session(session, "owner").await;
        let now = Utc::now();
        let parent_chat = Uuid::now_v7();
        let parent_output = Uuid::now_v7();
        let call_id = Uuid::now_v7();
        let request = BoundChildRequest {
            parent_chat_session_id: parent_chat,
            parent_output_id: parent_output,
            parent_call_id: call_id,
            parent_budget_used_tokens: 0,
            role: "researcher".into(),
            task: "Find a concise answer".into(),
            budget_tokens: 2048,
        };
        let root = store
            .create_bound_child("owner", session, request.clone(), 4, 3, 8192, now)
            .await
            .unwrap();
        let retry = store
            .create_bound_child("owner", session, request, 4, 3, 8192, now)
            .await
            .unwrap();
        assert_eq!(root.child_id, retry.child_id);
        let child_chat = Uuid::now_v7();
        let child_output = Uuid::now_v7();
        store
            .bind_child_output(
                "owner",
                session,
                root.child_id,
                child_chat,
                child_output,
                now,
            )
            .await
            .unwrap();
        store
            .report_child_usage("owner", session, root.child_id, 512, now)
            .await
            .unwrap();
        let nested = store
            .create_bound_child(
                "owner",
                session,
                BoundChildRequest {
                    parent_chat_session_id: child_chat,
                    parent_output_id: child_output,
                    parent_call_id: Uuid::now_v7(),
                    parent_budget_used_tokens: 512,
                    role: "checker".into(),
                    task: "Check the answer".into(),
                    budget_tokens: 2048,
                },
                4,
                3,
                8192,
                now,
            )
            .await
            .unwrap();
        assert_eq!(nested.parent_run_id, root.child_id.to_string());
        assert_eq!(nested.budget_tokens, 1536);
        assert_eq!(nested.depth, 2);
        let (root_state, _) = store
            .events_after("owner", session, root.child_id, 0, 10)
            .await
            .unwrap();
        assert_eq!(root_state.budget_used_tokens, 512);
        assert_eq!(root_state.budget_reserved_tokens, 1536);
        assert!(
            store
                .report_child_usage("owner", session, root.child_id, 1000, now)
                .await
                .is_err()
        );
        store
            .settle_child_budget("owner", session, nested.child_id, 128, now)
            .await
            .unwrap();
        store
            .settle_child_budget("owner", session, nested.child_id, 128, now)
            .await
            .unwrap();
        let (settled_parent, _) = store
            .events_after("owner", session, root.child_id, 0, 10)
            .await
            .unwrap();
        assert_eq!(settled_parent.budget_used_tokens, 640);
        assert_eq!(settled_parent.budget_reserved_tokens, 0);
        assert!(
            store
                .settle_child_budget("owner", session, nested.child_id, 129, now)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn account_binding_first_decision_digest_and_replay_are_enforced() {
        let store = InMemoryAgentRunStore::default();
        let session = Uuid::now_v7();
        store.register_session(session, "owner-a").await;
        let now = Utc::now();
        let run = store
            .create_child(
                "owner-a",
                session,
                &session.to_string(),
                "reviewer",
                "Inspect the change",
                4096,
                4,
                2,
                8192,
                now,
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .events_after("owner-b", session, run.child_id, 0, 20)
                .await
                .unwrap_err(),
            ChildRunError::Forbidden
        );
        let action_id = Uuid::now_v7();
        let digest = "a".repeat(64);
        let request_event_id = Uuid::now_v7();
        store
            .create_action(
                "owner-a",
                session,
                run.child_id,
                action(action_id, &run.parent_run_id, now, &digest),
                now,
                request_event_id,
            )
            .await
            .unwrap();

        assert_eq!(
            store
                .answer_action(
                    "owner-a",
                    session,
                    run.child_id,
                    action_id,
                    true,
                    &"b".repeat(64),
                    Uuid::now_v7(),
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
        let answer_event_id = Uuid::now_v7();
        let denied = store
            .answer_action(
                "owner-a",
                session,
                run.child_id,
                action_id,
                false,
                &digest,
                answer_event_id,
                now,
            )
            .await
            .unwrap();
        assert_eq!(denied.approved, Some(false));
        assert_eq!(
            store
                .answer_action(
                    "owner-a",
                    session,
                    run.child_id,
                    action_id,
                    true,
                    &digest,
                    Uuid::now_v7(),
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
        let progress_id = Uuid::now_v7();
        let first_progress = store
            .append_event(
                "owner-a",
                session,
                run.child_id,
                progress_id,
                ChildEventKind::Progress,
                Some("executing"),
                None,
                None,
                now,
            )
            .await
            .unwrap();
        assert_eq!(first_progress.seq, 4);
        let replay = store
            .append_event(
                "owner-a",
                session,
                run.child_id,
                progress_id,
                ChildEventKind::Progress,
                Some("executing"),
                None,
                None,
                now,
            )
            .await
            .unwrap();
        assert_eq!(replay.seq, first_progress.seq);
        assert_eq!(
            store
                .append_event(
                    "owner-a",
                    session,
                    run.child_id,
                    progress_id,
                    ChildEventKind::Progress,
                    Some("different"),
                    None,
                    None,
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
        let (_, events) = store
            .events_after("owner-a", session, run.child_id, 0, 20)
            .await
            .unwrap();
        assert_eq!(
            events.iter().map(|e| e.seq).collect::<Vec<_>>(),
            vec![1, 2, 3, 4]
        );
        assert_eq!(events[2].approved, Some(false));
    }

    #[tokio::test]
    async fn cancellation_is_only_terminal_after_cli_confirms_stop() {
        let store = InMemoryAgentRunStore::default();
        let session = Uuid::now_v7();
        store.register_session(session, "owner").await;
        let now = Utc::now();
        let run = store
            .create_child(
                "owner",
                session,
                &session.to_string(),
                "worker",
                "Run a bounded task",
                8192,
                4,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        store
            .append_event(
                "owner",
                session,
                run.child_id,
                Uuid::now_v7(),
                ChildEventKind::Progress,
                Some("started"),
                None,
                None,
                now,
            )
            .await
            .unwrap();
        let action_id = Uuid::now_v7();
        let digest = "e".repeat(64);
        store
            .create_action(
                "owner",
                session,
                run.child_id,
                action(action_id, &run.parent_run_id, now, &digest),
                now,
                Uuid::now_v7(),
            )
            .await
            .unwrap();
        let nested = store
            .create_child(
                "owner",
                session,
                &run.child_id.to_string(),
                "nested",
                "Nested work",
                4096,
                4,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        let cancel = store
            .request_cancel("owner", session, run.child_id, Uuid::now_v7(), now)
            .await
            .unwrap();
        assert_eq!(cancel.kind, ChildEventKind::CancelRequested);
        let (state, events) = store
            .events_after("owner", session, run.child_id, 0, 32)
            .await
            .unwrap();
        assert_eq!(state.status, ChildRunStatus::Running);
        assert!(
            events
                .iter()
                .any(|event| event.action_id == Some(action_id) && event.approved == Some(false))
        );
        let (_, nested_events) = store
            .events_after("owner", session, nested.child_id, 0, 32)
            .await
            .unwrap();
        assert!(
            nested_events
                .iter()
                .any(|event| event.kind == ChildEventKind::CancelRequested)
        );
        assert_eq!(
            store
                .append_event(
                    "owner",
                    session,
                    run.child_id,
                    Uuid::now_v7(),
                    ChildEventKind::Completed,
                    None,
                    Some("done"),
                    None,
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
        let stopped = store
            .append_event(
                "owner",
                session,
                run.child_id,
                Uuid::now_v7(),
                ChildEventKind::Cancelled,
                None,
                None,
                None,
                now,
            )
            .await
            .unwrap();
        assert_eq!(stopped.kind, ChildEventKind::Cancelled);
        assert_eq!(
            store
                .events_after("owner", session, run.child_id, stopped.seq, 10)
                .await
                .unwrap()
                .0
                .status,
            ChildRunStatus::Cancelled
        );
    }

    #[tokio::test]
    async fn expired_actions_and_unconfirmed_cancel_do_not_authorize_effects() {
        let store = InMemoryAgentRunStore::default();
        let session = Uuid::now_v7();
        store.register_session(session, "owner").await;
        let now = Utc::now();
        let run = store
            .create_child(
                "owner",
                session,
                &session.to_string(),
                "worker",
                "Prepare",
                4096,
                4,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        let mut expired = action(Uuid::now_v7(), &run.parent_run_id, now, &"c".repeat(64));
        expired.expires_at = now - Duration::seconds(1);
        assert_eq!(
            store
                .create_action("owner", session, run.child_id, expired, now, Uuid::now_v7())
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
    }

    #[tokio::test]
    async fn active_child_and_depth_limits_are_enforced() {
        let store = InMemoryAgentRunStore::default();
        let session = Uuid::now_v7();
        store.register_session(session, "owner").await;
        let now = Utc::now();
        let root = store
            .create_child(
                "owner",
                session,
                &session.to_string(),
                "parent",
                "Root",
                8192,
                2,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        let nested = store
            .create_child(
                "owner",
                session,
                &root.child_id.to_string(),
                "child",
                "Nested",
                4096,
                2,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        assert_eq!(nested.depth, 2);
        assert_eq!(
            store
                .create_child(
                    "owner",
                    session,
                    &nested.child_id.to_string(),
                    "grandchild",
                    "Too deep",
                    2048,
                    2,
                    2,
                    8192,
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
        assert_eq!(
            store
                .create_child(
                    "owner",
                    session,
                    &session.to_string(),
                    "extra",
                    "Over capacity",
                    8192,
                    2,
                    2,
                    8192,
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
    }

    #[tokio::test]
    async fn simultaneous_local_and_remote_answers_have_one_winner() {
        let store = InMemoryAgentRunStore::default();
        let session = Uuid::now_v7();
        store.register_session(session, "owner").await;
        let now = Utc::now();
        let run = store
            .create_child(
                "owner",
                session,
                &session.to_string(),
                "worker",
                "Review",
                4096,
                4,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        let action_id = Uuid::now_v7();
        let digest = "d".repeat(64);
        store
            .create_action(
                "owner",
                session,
                run.child_id,
                action(action_id, &run.parent_run_id, now, &digest),
                now,
                Uuid::now_v7(),
            )
            .await
            .unwrap();
        let (local, remote) = tokio::join!(
            store.answer_action(
                "owner",
                session,
                run.child_id,
                action_id,
                true,
                &digest,
                Uuid::now_v7(),
                now,
            ),
            store.answer_action(
                "owner",
                session,
                run.child_id,
                action_id,
                false,
                &digest,
                Uuid::now_v7(),
                now,
            ),
        );
        assert_eq!(usize::from(local.is_ok()) + usize::from(remote.is_ok()), 1);
        assert_eq!(
            usize::from(local == Err(ChildRunError::Conflict))
                + usize::from(remote == Err(ChildRunError::Conflict)),
            1
        );
    }

    #[tokio::test]
    async fn child_token_budgets_cannot_exceed_parent_or_be_overallocated() {
        let store = InMemoryAgentRunStore::default();
        let session = Uuid::now_v7();
        store.register_session(session, "owner").await;
        let now = Utc::now();
        let root = store
            .create_child(
                "owner",
                session,
                &session.to_string(),
                "parent",
                "Root",
                8192,
                4,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        store
            .create_child(
                "owner",
                session,
                &root.child_id.to_string(),
                "child-a",
                "First allocation",
                5000,
                4,
                2,
                8192,
                now,
            )
            .await
            .unwrap();
        assert_eq!(
            store
                .create_child(
                    "owner",
                    session,
                    &root.child_id.to_string(),
                    "child-b",
                    "Overallocated",
                    4000,
                    4,
                    2,
                    8192,
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Conflict
        );
        assert_eq!(
            store
                .create_child(
                    "owner",
                    session,
                    &session.to_string(),
                    "too-large",
                    "Exceeds cap",
                    8193,
                    4,
                    2,
                    8192,
                    now
                )
                .await
                .unwrap_err(),
            ChildRunError::Invalid
        );
    }
}
