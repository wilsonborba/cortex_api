use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{get, post},
};
use chrono::{DateTime, Utc};
use futures_util::stream;
use serde::Deserialize;
use std::{collections::VecDeque, convert::Infallible, time::Duration};
use uuid::Uuid;

use crate::{
    domain::services::agent_runs::{
        AgentRunService, ChildEventKind, ChildRunError, PendingChildAction,
    },
    presentation::{
        UserId,
        response::{self, Envelope},
    },
};

fn now() -> DateTime<Utc> {
    Utc::now()
}
fn terminal_text_is_safe(value: &str) -> bool {
    value.chars().all(|ch| {
        (!ch.is_control() || ch == '\n' || ch == '\t')
            && !matches!(ch as u32, 0x202a..=0x202e | 0x2066..=0x2069)
    })
}
fn safe_error_code(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 80
        && value
            .bytes()
            .all(|byte| byte.is_ascii_uppercase() || byte.is_ascii_digit() || byte == b'_')
}
fn map_error(error: ChildRunError) -> Response {
    match error {
        ChildRunError::NotFound => response::error(
            StatusCode::NOT_FOUND,
            crate::core::process_numbers::AGENT_RUN_NOT_FOUND,
        ),
        ChildRunError::Forbidden => response::error(
            StatusCode::FORBIDDEN,
            crate::core::process_numbers::REMOTE_SESSION_FORBIDDEN,
        ),
        ChildRunError::Conflict => response::error(
            StatusCode::CONFLICT,
            crate::core::process_numbers::AGENT_RUN_CONFLICT,
        ),
        ChildRunError::Invalid => response::error(
            StatusCode::BAD_REQUEST,
            crate::core::process_numbers::INVALID_DATA,
        ),
        ChildRunError::Unavailable => response::error(
            StatusCode::SERVICE_UNAVAILABLE,
            crate::core::process_numbers::REMOTE_CLI_UNAVAILABLE,
        ),
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateChild {
    parent_run_id: String,
    role: String,
    task: String,
    budget_tokens: u32,
}
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum AppendKind {
    Progress,
    Completed,
    Failed,
    Cancelled,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppendEvent {
    event_id: Uuid,
    kind: AppendKind,
    message: Option<String>,
    result: Option<String>,
    error_code: Option<String>,
}
#[derive(Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum ActionKind {
    FileChange,
    Command,
}
impl ActionKind {
    fn as_str(&self) -> &'static str {
        match self {
            Self::FileChange => "file_change",
            Self::Command => "command",
        }
    }
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateAction {
    action_id: Uuid,
    parent_run_id: String,
    kind: ActionKind,
    payload_digest: String,
    display: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerAction {
    approved: bool,
    payload_digest: String,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EventQuery {
    after: Option<u64>,
}

pub fn router(service: AgentRunService) -> Router {
    Router::new()
        .route("/remote/sessions/{session_id}/children", post(create_child))
        .route(
            "/remote/sessions/{session_id}/children/{child_id}/events",
            get(events).post(append_event),
        )
        .route(
            "/remote/sessions/{session_id}/children/{child_id}/cancel",
            post(cancel),
        )
        .route(
            "/remote/sessions/{session_id}/children/{child_id}/actions",
            post(create_action),
        )
        .route(
            "/remote/sessions/{session_id}/children/{child_id}/actions/{action_id}/answer",
            post(answer_action),
        )
        .with_state(service)
}

async fn create_child(
    State(service): State<AgentRunService>,
    UserId(owner): UserId,
    Path(session): Path<Uuid>,
    Json(body): Json<CreateChild>,
) -> Response {
    if body.parent_run_id.trim().is_empty()
        || body.parent_run_id.len() > 128
        || body.role.trim().is_empty()
        || body.role.len() > 80
        || body.task.trim().is_empty()
        || body.task.len() > 32_000
        || body.budget_tokens == 0
        || body.budget_tokens > service.max_budget_tokens
    {
        return response::error(
            StatusCode::BAD_REQUEST,
            crate::core::process_numbers::INVALID_DATA,
        );
    }
    match service
        .store
        .create_child(
            &owner,
            session,
            &body.parent_run_id,
            &body.role,
            &body.task,
            body.budget_tokens,
            service.max_active_children,
            service.max_depth,
            service.max_budget_tokens,
            now(),
        )
        .await
    {
        Ok(run) => (StatusCode::CREATED, Envelope::new("Child run created", run)).into_response(),
        Err(e) => map_error(e),
    }
}

async fn append_event(
    State(service): State<AgentRunService>,
    UserId(owner): UserId,
    Path((session, child)): Path<(Uuid, Uuid)>,
    Json(body): Json<AppendEvent>,
) -> Response {
    let valid = match body.kind {
        AppendKind::Progress => {
            body.message.as_ref().is_some_and(|s| {
                !s.trim().is_empty() && s.len() <= 4096 && terminal_text_is_safe(s)
            }) && body.result.is_none()
                && body.error_code.is_none()
        }
        AppendKind::Completed => {
            body.result
                .as_ref()
                .is_some_and(|s| s.len() <= 32_000 && terminal_text_is_safe(s))
                && body.message.is_none()
                && body.error_code.is_none()
        }
        AppendKind::Failed => {
            body.error_code.as_ref().is_some_and(|s| safe_error_code(s))
                && body.message.is_none()
                && body.result.is_none()
        }
        AppendKind::Cancelled => {
            body.message.is_none() && body.result.is_none() && body.error_code.is_none()
        }
    };
    if !valid {
        return response::error(
            StatusCode::BAD_REQUEST,
            crate::core::process_numbers::INVALID_DATA,
        );
    }
    let kind = match body.kind {
        AppendKind::Progress => ChildEventKind::Progress,
        AppendKind::Completed => ChildEventKind::Completed,
        AppendKind::Failed => ChildEventKind::Failed,
        AppendKind::Cancelled => ChildEventKind::Cancelled,
    };
    match service
        .store
        .append_event(
            &owner,
            session,
            child,
            body.event_id,
            kind,
            body.message.as_deref(),
            body.result.as_deref(),
            body.error_code.as_deref(),
            now(),
        )
        .await
    {
        Ok(event) => Envelope::new("Child event recorded", event).into_response(),
        Err(e) => map_error(e),
    }
}

async fn cancel(
    State(service): State<AgentRunService>,
    UserId(owner): UserId,
    Path((session, child)): Path<(Uuid, Uuid)>,
) -> Response {
    match service
        .store
        .request_cancel(&owner, session, child, Uuid::now_v7(), now())
        .await
    {
        Ok(event) => (
            StatusCode::ACCEPTED,
            Envelope::new("Cancellation requested", event),
        )
            .into_response(),
        Err(e) => map_error(e),
    }
}

async fn create_action(
    State(service): State<AgentRunService>,
    UserId(owner): UserId,
    Path((session, child)): Path<(Uuid, Uuid)>,
    Json(body): Json<CreateAction>,
) -> Response {
    let has_required_display = match body.kind {
        ActionKind::Command => {
            body.display.lines().any(|line| line.starts_with("cwd: "))
                && body
                    .display
                    .lines()
                    .any(|line| line.starts_with("command: "))
        }
        ActionKind::FileChange => {
            body.display.lines().any(|line| line.starts_with("path: "))
                && body.display.lines().any(|line| line.starts_with("diff: "))
        }
    };
    if !has_required_display
        || body.parent_run_id.trim().is_empty()
        || body.parent_run_id.len() > 128
        || !is_lower_hex_digest(&body.payload_digest)
        || body.display.trim().is_empty()
        || body.display.len() > 64_000
    {
        return response::error(
            StatusCode::BAD_REQUEST,
            crate::core::process_numbers::INVALID_DATA,
        );
    }
    let created = now();
    let action = PendingChildAction {
        action_id: body.action_id,
        parent_run_id: body.parent_run_id,
        action_kind: body.kind.as_str().into(),
        payload_digest: body.payload_digest,
        display: body.display,
        expires_at: created + chrono::Duration::milliseconds(service.action_ttl_ms as i64),
    };
    match service
        .store
        .create_action(&owner, session, child, action, created, Uuid::now_v7())
        .await
    {
        Ok(event) => (
            StatusCode::CREATED,
            Envelope::new("Action approval requested", event),
        )
            .into_response(),
        Err(e) => map_error(e),
    }
}

async fn answer_action(
    State(service): State<AgentRunService>,
    UserId(owner): UserId,
    Path((session, child, action)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<AnswerAction>,
) -> Response {
    if !is_lower_hex_digest(&body.payload_digest) {
        return response::error(
            StatusCode::BAD_REQUEST,
            crate::core::process_numbers::INVALID_DATA,
        );
    }
    match service
        .store
        .answer_action(
            &owner,
            session,
            child,
            action,
            body.approved,
            &body.payload_digest,
            Uuid::now_v7(),
            now(),
        )
        .await
    {
        Ok(event) => Envelope::new("Action decision recorded", event).into_response(),
        Err(e) => map_error(e),
    }
}

fn is_lower_hex_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

async fn events(
    State(service): State<AgentRunService>,
    UserId(owner): UserId,
    Path((session, child)): Path<(Uuid, Uuid)>,
    Query(query): Query<EventQuery>,
) -> Response {
    let first = match service
        .store
        .events_after(&owner, session, child, query.after.unwrap_or(0), 128)
        .await
    {
        Ok(v) => v,
        Err(e) => return map_error(e),
    };
    let (run, events) = first;
    let after = events
        .last()
        .map(|event| event.seq)
        .unwrap_or(query.after.unwrap_or(0));
    let queue = events
        .into_iter()
        .map(|event| {
            Ok::<_, Infallible>(
                Event::default()
                    .id(event.seq.to_string())
                    .event(event.kind.as_str())
                    .json_data(event)
                    .expect("serialized child event"),
            )
        })
        .collect::<VecDeque<_>>();
    let done = run.status.terminal() && queue.len() < 128;
    let state = (service, owner, session, child, after, queue, done);
    let event_stream = stream::unfold(
        state,
        |(service, owner, session, child, mut after, mut queue, mut done)| async move {
            loop {
                if let Some(event) = queue.pop_front() {
                    return Some((event, (service, owner, session, child, after, queue, done)));
                }
                if done {
                    return None;
                }
                tokio::time::sleep(Duration::from_millis(500)).await;
                match service
                    .store
                    .events_after(&owner, session, child, after, 128)
                    .await
                {
                    Ok((run, events)) => {
                        done = run.status.terminal() && events.len() < 128;
                        if events.is_empty() {
                            if done {
                                return None;
                            }
                            return Some((
                                Ok(Event::default().comment("heartbeat")),
                                (service, owner, session, child, after, queue, done),
                            ));
                        }
                        for event in events {
                            after = event.seq;
                            queue.push_back(Ok(Event::default()
                                .id(event.seq.to_string())
                                .event(event.kind.as_str())
                                .json_data(event)
                                .expect("serialized child event")));
                        }
                    }
                    Err(_) => {
                        return Some((
                            Ok(Event::default()
                                .event("error")
                                .data("event stream unavailable")),
                            (service, owner, session, child, after, queue, true),
                        ));
                    }
                }
            }
        },
    );
    Sse::new(event_stream)
        .keep_alive(KeepAlive::new().interval(Duration::from_secs(15)))
        .into_response()
}
