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
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{collections::VecDeque, convert::Infallible, sync::Arc, time::Duration};
use uuid::Uuid;

use crate::{
    domain::services::agent_runs::{
        AgentRunService, ChildEventKind, ChildRunError, OutputAction, PendingChildAction,
    },
    domain::services::{
        chat_runtime::ChatRuntime,
        remote_cli::{RemoteCliError, RemoteCliService, RemoteEventKind},
    },
    presentation::{
        UserId,
        identity::CallerContext,
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

#[derive(Clone)]
struct OutputActionState {
    service: AgentRunService,
    runtime: Arc<ChatRuntime>,
    remote_cli: RemoteCliService,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CreateOutputAction {
    call_id: Uuid,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AnswerOutputAction {
    approved: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ClaimOutputAction {
    call_id: Uuid,
    action_digest: String,
}

#[derive(Serialize)]
struct OutputActionView {
    action_id: Uuid,
    call_id: Uuid,
    name: String,
    action_kind: String,
    action_digest: String,
    display: String,
    expires_at: DateTime<Utc>,
    status: &'static str,
}

impl OutputActionView {
    fn from_action(action: OutputAction, now: DateTime<Utc>) -> Self {
        let status = action.status(now);
        Self {
            action_id: action.action_id,
            call_id: action.call_id,
            name: action.tool_name,
            action_kind: action.action_kind,
            action_digest: action.action_digest,
            display: action.display,
            expires_at: action.expires_at,
            status,
        }
    }
}

pub fn output_router(
    service: AgentRunService,
    runtime: Arc<ChatRuntime>,
    remote_cli: RemoteCliService,
) -> Router {
    let state = OutputActionState {
        service,
        runtime,
        remote_cli,
    };
    Router::new()
        .route(
            "/remote/sessions/{remote_session_id}/outputs/{chat_session_id}/{output_id}/actions",
            post(create_output_action),
        )
        .route(
            "/remote/sessions/{remote_session_id}/outputs/{chat_session_id}/{output_id}/actions/{action_id}",
            get(get_output_action),
        )
        .route(
            "/remote/sessions/{remote_session_id}/outputs/{chat_session_id}/{output_id}/actions/{action_id}/answer",
            post(answer_output_action),
        )
        .route(
            "/remote/sessions/{remote_session_id}/outputs/{chat_session_id}/{output_id}/actions/{action_id}/claim",
            post(claim_output_action),
        )
        .with_state(state)
}

fn status(process: crate::core::process_numbers::ProcessNumber) -> StatusCode {
    match process.number {
        n if n == crate::core::process_numbers::OUTPUT_NOT_RUNNING.number => StatusCode::NOT_FOUND,
        n if n == crate::core::process_numbers::AGENT_RUN_CONFLICT.number => StatusCode::CONFLICT,
        n if n == crate::core::process_numbers::TEMPORARY_UNAVAILABLE.number => {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::BAD_REQUEST,
    }
}

async fn live_pending(
    state: &OutputActionState,
    owner: &str,
    context: &CallerContext,
    chat_session_id: Uuid,
    output_id: Uuid,
) -> Result<crate::domain::models::tools::PendingTool, Response> {
    let failure = |process: crate::core::process_numbers::ProcessNumber| {
        response::error(status(process), process)
    };
    crate::presentation::chat_inputs::owned_output(
        &state.runtime,
        context,
        owner,
        chat_session_id,
        output_id,
    )
    .await
    .map_err(failure)?;
    let control = state
        .runtime
        .controls
        .get(output_id)
        .ok_or_else(|| failure(crate::core::process_numbers::OUTPUT_NOT_RUNNING))?;
    if *control.cancel.borrow() {
        return Err(failure(crate::core::process_numbers::OUTPUT_NOT_RUNNING));
    }
    control
        .tools
        .pending()
        .ok_or_else(|| failure(crate::core::process_numbers::OUTPUT_NOT_RUNNING))
}

fn action_display(
    name: crate::domain::models::tools::ToolName,
    args: &Value,
) -> Option<(String, String)> {
    match name {
        crate::domain::models::tools::ToolName::RunCommand => {
            let cwd = args.get("cwd")?;
            let argv = args.get("argv")?;
            Some(("command".into(), format!("cwd: {}\ncommand: {}", cwd, argv)))
        }
        crate::domain::models::tools::ToolName::ReplaceFileContent => {
            let path = args.get("path")?;
            let find = args.get("find")?;
            let replace = args.get("replace")?;
            Some((
                "file_change".into(),
                format!("path: {path}\ndiff: replace {find} with {replace}"),
            ))
        }
        _ => None,
    }
}

fn map_remote_error(error: RemoteCliError) -> Response {
    use crate::core::process_numbers as p;
    let (status, process) = match error {
        RemoteCliError::NotFound => (StatusCode::NOT_FOUND, p::REMOTE_SESSION_NOT_FOUND),
        RemoteCliError::Forbidden => (StatusCode::FORBIDDEN, p::REMOTE_SESSION_FORBIDDEN),
        RemoteCliError::AlreadyAnswered => (StatusCode::CONFLICT, p::REMOTE_APPROVAL_ANSWERED),
        RemoteCliError::BadRequest(_) => (StatusCode::BAD_REQUEST, p::INVALID_DATA),
        RemoteCliError::Unavailable => (StatusCode::SERVICE_UNAVAILABLE, p::REMOTE_CLI_UNAVAILABLE),
    };
    response::error(status, process)
}

async fn create_output_action(
    State(state): State<OutputActionState>,
    UserId(owner): UserId,
    context: CallerContext,
    Path((remote_session, chat_session, output)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<CreateOutputAction>,
) -> Response {
    let pending = match live_pending(&state, &owner, &context, chat_session, output).await {
        Ok(pending) => pending,
        Err(error) => return error,
    };
    if pending.call_id != body.call_id {
        return map_error(ChildRunError::Conflict);
    }
    let Some((action_kind, display)) = action_display(pending.name, &pending.arguments) else {
        return map_error(ChildRunError::Conflict);
    };
    if display.len() > 64_000 {
        return map_error(ChildRunError::Invalid);
    }
    let now = Utc::now();
    let action = OutputAction {
        action_id: Uuid::now_v7(),
        chat_session_id: chat_session,
        output_id: output,
        call_id: pending.call_id,
        tool_name: pending.name.as_str().into(),
        action_kind,
        action_digest: crate::domain::models::tools::action_digest(
            pending.call_id,
            pending.name,
            &pending.arguments,
        ),
        display,
        expires_at: now + chrono::Duration::milliseconds(state.service.action_ttl_ms as i64),
        approved: None,
        cancelled_at: None,
        claimed_at: None,
        created_at: now,
    };
    let requested_action_id = action.action_id;
    let created = match state
        .service
        .store
        .create_output_action(&owner, remote_session, action, now)
        .await
    {
        Ok(action) => action,
        Err(error) => return map_error(error),
    };
    let is_new = created.action_id == requested_action_id;
    if is_new {
        let event = json!({
            "approval_id": created.action_id.to_string(),
            "action_text": created.display.clone(),
        });
        let remote_session_text = remote_session.to_string();
        if let Err(error) = state
            .remote_cli
            .post_cli_event(
                &remote_session_text,
                &owner,
                RemoteEventKind::ApprovalRequest,
                event,
                None,
                now.timestamp_millis().max(0) as u64,
            )
            .await
        {
            let _ = state
                .service
                .store
                .cancel_output_actions(&owner, chat_session, output, Utc::now())
                .await;
            return map_remote_error(error);
        }
    }
    (
        if is_new {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Envelope::new(
            "Output action approval requested",
            OutputActionView::from_action(created, now),
        ),
    )
        .into_response()
}

async fn get_output_action(
    State(state): State<OutputActionState>,
    UserId(owner): UserId,
    context: CallerContext,
    Path((remote_session, chat_session, output, action_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
) -> Response {
    let failure = |process: crate::core::process_numbers::ProcessNumber| {
        response::error(status(process), process)
    };
    if let Err(process) = crate::presentation::chat_inputs::owned_output(
        &state.runtime,
        &context,
        &owner,
        chat_session,
        output,
    )
    .await
    {
        return failure(process);
    }
    match state
        .service
        .store
        .output_action(&owner, remote_session, chat_session, output, action_id)
        .await
    {
        Ok(action) => Envelope::new(
            "Output action loaded",
            OutputActionView::from_action(action, Utc::now()),
        )
        .into_response(),
        Err(error) => map_error(error),
    }
}

async fn answer_output_action(
    State(state): State<OutputActionState>,
    UserId(owner): UserId,
    context: CallerContext,
    Path((remote_session, chat_session, output, action_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    Json(body): Json<AnswerOutputAction>,
) -> Response {
    let pending = match live_pending(&state, &owner, &context, chat_session, output).await {
        Ok(pending) => pending,
        Err(error) => return error,
    };
    let now = Utc::now();
    let stored = match state
        .service
        .store
        .output_action(&owner, remote_session, chat_session, output, action_id)
        .await
    {
        Ok(action) => action,
        Err(error) => return map_error(error),
    };
    if pending.call_id != stored.call_id
        || crate::domain::models::tools::action_digest(
            pending.call_id,
            pending.name,
            &pending.arguments,
        ) != stored.action_digest
    {
        return map_error(ChildRunError::Conflict);
    }
    match state
        .service
        .store
        .answer_output_action(
            &owner,
            remote_session,
            chat_session,
            output,
            action_id,
            body.approved,
            now,
        )
        .await
    {
        Ok(action) => Envelope::new(
            "Output action answered",
            OutputActionView::from_action(action, now),
        )
        .into_response(),
        Err(error) => map_error(error),
    }
}

async fn claim_output_action(
    State(state): State<OutputActionState>,
    UserId(owner): UserId,
    context: CallerContext,
    Path((remote_session, chat_session, output, action_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    Json(body): Json<ClaimOutputAction>,
) -> Response {
    let pending = match live_pending(&state, &owner, &context, chat_session, output).await {
        Ok(pending) => pending,
        Err(error) => return error,
    };
    if !is_lower_hex_digest(&body.action_digest)
        || pending.call_id != body.call_id
        || crate::domain::models::tools::action_digest(
            pending.call_id,
            pending.name,
            &pending.arguments,
        ) != body.action_digest
    {
        return map_error(ChildRunError::Conflict);
    }
    match state
        .service
        .store
        .claim_output_action(
            &owner,
            remote_session,
            chat_session,
            output,
            action_id,
            body.call_id,
            &body.action_digest,
            Utc::now(),
        )
        .await
    {
        Ok(action) => Envelope::new(
            "Output action claimed",
            OutputActionView::from_action(action, Utc::now()),
        )
        .into_response(),
        Err(error) => map_error(error),
    }
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
