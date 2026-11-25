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

use crate::domain::services::chat_runtime::Finished;
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
            "/remote/sessions/{session_id}/children/{child_id}",
            get(child_detail),
        )
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
struct RegisterChildCall {
    call_id: Uuid,
}

#[derive(Serialize)]
struct BoundChildView {
    child_id: Uuid,
    parent_call_id: Uuid,
    status: crate::domain::services::agent_runs::ChildRunStatus,
    depth: u8,
    budget_tokens: u32,
    child_chat_session_id: Option<Uuid>,
    child_output_id: Option<Uuid>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PendingSubagentArgs {
    role: String,
    task: String,
    context: Option<String>,
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
            "/remote/sessions/{remote_session_id}/outputs/{chat_session_id}/{output_id}/children",
            post(create_bound_child),
        )
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

async fn create_bound_child(
    State(state): State<OutputActionState>,
    UserId(owner): UserId,
    context: CallerContext,
    Path((remote_session, parent_chat, parent_output)): Path<(Uuid, Uuid, Uuid)>,
    Json(body): Json<RegisterChildCall>,
) -> Response {
    let pending = match live_pending(
        &state,
        &owner,
        &context,
        remote_session,
        parent_chat,
        parent_output,
    )
    .await
    {
        Ok(pending) => pending,
        Err(error) => return error,
    };
    if pending.call_id != body.call_id
        || pending.name != crate::domain::models::tools::ToolName::InvokeSubagent
    {
        return map_error(ChildRunError::Conflict);
    }
    let args: PendingSubagentArgs = match serde_json::from_value(pending.arguments.clone()) {
        Ok(args) => args,
        Err(_) => return map_error(ChildRunError::Invalid),
    };
    let task = match args
        .context
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    {
        Some(context) => format!(
            "Role: {}\n\nTask:\n{}\n\nExplicit context:\n{}",
            args.role, args.task, context
        ),
        None => format!("Role: {}\n\nTask:\n{}", args.role, args.task),
    };
    if task.len() > 32_000 {
        return map_error(ChildRunError::Invalid);
    }
    let parent_budget = match state
        .service
        .store
        .child_for_output(&owner, remote_session, parent_chat, parent_output)
        .await
    {
        Ok(Some(parent)) => match state
            .service
            .active_child_budget(remote_session, parent.child_id)
        {
            Some(budget) => Some(budget),
            None => return map_error(ChildRunError::Conflict),
        },
        Ok(None) => None,
        Err(error) => return map_error(error),
    };
    let parent_used = parent_budget.as_ref().map_or(0, |budget| budget.used());
    let request = crate::domain::services::agent_runs::BoundChildRequest {
        parent_chat_session_id: parent_chat,
        parent_output_id: parent_output,
        parent_call_id: pending.call_id,
        parent_budget_used_tokens: parent_used,
        role: args.role,
        task: task.clone(),
        budget_tokens: state.service.max_budget_tokens,
    };
    let child = match state
        .service
        .store
        .create_bound_child(
            &owner,
            remote_session,
            request,
            state.service.max_active_children,
            state.service.max_depth,
            state.service.max_budget_tokens,
            now(),
        )
        .await
    {
        Ok(child) => child,
        Err(error) => return map_error(error),
    };
    if !child.budget_settled
        && let Some(parent_budget) = &parent_budget
        && !parent_budget.reserve(child.child_id, u64::from(child.budget_tokens))
    {
        return map_error(ChildRunError::Conflict);
    }
    if !child.status.terminal() && child.child_output_id.is_none() {
        let parent_control = match state.runtime.controls.get(parent_output) {
            Some(control) => control,
            None => return map_error(ChildRunError::Conflict),
        };
        let caller = match context.for_app("cortex", true) {
            Ok(caller) => caller,
            Err(error) => return error.into_response(),
        };
        start_bound_child(
            state.clone(),
            owner,
            remote_session,
            child.clone(),
            task,
            caller,
            parent_control,
            parent_budget,
        );
    }
    (
        if child.status == crate::domain::services::agent_runs::ChildRunStatus::Queued
            && child.child_output_id.is_none()
        {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Envelope::new(
            "Bound child run registered",
            BoundChildView {
                child_id: child.child_id,
                parent_call_id: child.parent_call_id.unwrap_or(body.call_id),
                status: child.status,
                depth: child.depth,
                budget_tokens: child.budget_tokens,
                child_chat_session_id: child.child_chat_session_id,
                child_output_id: child.child_output_id,
            },
        ),
    )
        .into_response()
}

fn child_provider_may_start(
    child: &crate::domain::services::agent_runs::ChildRun,
    parent_cancelled: bool,
    child_cancelled: bool,
) -> bool {
    !child.status.terminal() && !child.cancel_requested && !parent_cancelled && !child_cancelled
}

fn cancel_bound_output_if_requested(
    control: &crate::domain::services::output_controls::Control,
    parent_cancel: &tokio::sync::watch::Receiver<bool>,
    child_cancel: &tokio::sync::watch::Receiver<bool>,
) -> bool {
    if *parent_cancel.borrow() || *child_cancel.borrow() {
        control.request_cancel();
        true
    } else {
        false
    }
}

#[allow(clippy::too_many_arguments)]
fn start_bound_child(
    state: OutputActionState,
    owner: String,
    remote_session: Uuid,
    child: crate::domain::services::agent_runs::ChildRun,
    task: String,
    caller: crate::domain::models::identity::Caller,
    parent_control: crate::domain::services::output_controls::Control,
    parent_budget: Option<Arc<crate::domain::models::thinking::TokenBudget>>,
) {
    let (cancel_tx, cancel_rx) = tokio::sync::watch::channel(false);
    if !state
        .service
        .register_child_task(remote_session, child.child_id, cancel_tx)
    {
        return;
    }
    tokio::spawn(async move {
        let mut cancel_rx = cancel_rx;
        let mut parent_cancel = parent_control.cancel.subscribe();
        let runtime = state.runtime.clone();
        let token_budget = Arc::new(crate::domain::models::thinking::TokenBudget::new(
            u64::from(child.budget_tokens),
        ));
        state
            .service
            .register_child_budget(remote_session, child.child_id, token_budget.clone());
        let current = state
            .service
            .store
            .events_after(&owner, remote_session, child.child_id, 0, 1)
            .await;
        let Ok((stored_child, _)) = current else {
            let _ = state
                .service
                .store
                .append_event(
                    &owner,
                    remote_session,
                    child.child_id,
                    Uuid::now_v7(),
                    ChildEventKind::Failed,
                    None,
                    None,
                    Some("CHILD_FAILED"),
                    now(),
                )
                .await;
            state
                .service
                .unregister_child_task(remote_session, child.child_id);
            return;
        };
        let parent_cancelled = *parent_cancel.borrow();
        let child_cancelled = *cancel_rx.borrow();
        if !child_provider_may_start(&stored_child, parent_cancelled, child_cancelled) {
            let cancellation_seen = parent_cancelled || child_cancelled;
            let cancel_recorded = stored_child.cancel_requested
                || (cancellation_seen
                    && !stored_child.status.terminal()
                    && state
                        .service
                        .store
                        .request_cancel(
                            &owner,
                            remote_session,
                            child.child_id,
                            Uuid::now_v7(),
                            now(),
                        )
                        .await
                        .is_ok());
            if cancel_recorded && !stored_child.status.terminal() {
                let _ = state
                    .service
                    .store
                    .settle_child_budget(&owner, remote_session, child.child_id, 0, now())
                    .await;
                if let Some(parent_budget) = &parent_budget {
                    let _ = parent_budget.settle_reservation(child.child_id, 0);
                }
                let _ = state
                    .service
                    .store
                    .append_event(
                        &owner,
                        remote_session,
                        child.child_id,
                        Uuid::now_v7(),
                        ChildEventKind::Cancelled,
                        None,
                        None,
                        None,
                        now(),
                    )
                    .await;
            }
            state
                .service
                .unregister_child_task(remote_session, child.child_id);
            return;
        }
        let body = crate::domain::models::chat_session::NewChat {
            app_id: "cortex".into(),
            text: task,
            file_ids: vec![],
            stream: true,
            needs_web: false,
            web_mode: crate::domain::models::run_record::WebMode::Fast,
            use_memory: false,
            memory_topic: None,
            thinking: false,
            persona_id: None,
            temporary: false,
        };
        let mut turn = crate::domain::models::chat_runtime::Turn::new(
            caller,
            crate::domain::models::chat_runtime::TurnRequest::New(body),
        );
        turn.tools_enabled = true;
        turn.token_budget = Some(token_budget.clone());
        let (events_tx, mut events_rx) = tokio::sync::mpsc::unbounded_channel();
        let mut finished = runtime.start(turn, Some(events_tx));
        let mut child_chat = None;
        let mut child_output = None;
        let mut cancel_open = true;
        let mut parent_cancel_open = true;
        let mut events_open = true;
        let finished = loop {
            tokio::select! {
                biased;
                changed = cancel_rx.changed(), if cancel_open => {
                    if changed.is_err() { cancel_open = false; }
                    if *cancel_rx.borrow()
                        && let Some(output) = child_output
                        && let Some(control) = runtime.controls.get(output)
                    {
                        control.request_cancel();
                    }
                }
                changed = parent_cancel.changed(), if parent_cancel_open => {
                    if changed.is_err() { parent_cancel_open = false; }
                    if *parent_cancel.borrow() {
                        let _ = state.service.store.request_cancel(
                            &owner, remote_session, child.child_id, Uuid::now_v7(), now(),
                        ).await;
                        state.service.cancel_child_task(remote_session, child.child_id);
                        if let Some(output) = child_output
                            && let Some(control) = runtime.controls.get(output)
                        {
                            control.request_cancel();
                        }
                    }
                }
                event = events_rx.recv(), if events_open => match event {
                    Some(event) => {
                        if let crate::domain::models::events::NativeEvent::Stage(stage) = &event.event {
                            if let (Some(chat), Some(output)) = (stage.session_id, stage.output_id)
                                && child_output.is_none()
                            {
                                    match state.service.store.bind_child_output(&owner, remote_session, child.child_id, chat, output, now()).await {
                                        Ok(_) => { child_chat = Some(chat); child_output = Some(output); }
                                        Err(_) => {
                                            if let Some(control) = runtime.controls.get(output) { control.request_cancel(); }
                                        }
                                    }
                                    if let Some(control) = runtime.controls.get(output) {
                                        cancel_bound_output_if_requested(
                                            &control,
                                            &parent_cancel,
                                            &cancel_rx,
                                        );
                                    }
                            }
                            if stage.status != "started" {
                                let message = format!("Stage {:?}: {}", stage.stage, stage.status);
                                let _ = state.service.store.append_event(
                                    &owner, remote_session, child.child_id, Uuid::now_v7(),
                                    ChildEventKind::Progress, Some(&message), None, None, now(),
                                ).await;
                            }
                        }
                    }
                    None => events_open = false,
                },
                done = &mut finished => {
                    break done.ok();
                }
            }
        };
        let cancelled = *cancel_rx.borrow();
        let provider_completed = finished
            .as_ref()
            .is_some_and(|run| run.error.is_none() && run.output.is_some());
        let settlement_usage = token_budget.settlement_usage();
        let budget_settled = if provider_completed && !cancelled {
            state
                .service
                .store
                .settle_child_budget(
                    &owner,
                    remote_session,
                    child.child_id,
                    settlement_usage,
                    now(),
                )
                .await
                .is_ok()
                && parent_budget.as_ref().is_none_or(|budget| {
                    budget.settle_reservation(child.child_id, settlement_usage)
                })
        } else if cancelled || *parent_control.cancel.borrow() {
            state
                .service
                .store
                .settle_child_budget(
                    &owner,
                    remote_session,
                    child.child_id,
                    settlement_usage,
                    now(),
                )
                .await
                .is_ok()
                && parent_budget.as_ref().is_none_or(|budget| {
                    budget.settle_reservation(child.child_id, settlement_usage)
                })
        } else {
            false
        };
        let parent_cancelled = *parent_control.cancel.borrow();
        let (kind, tool_result, event_result, error_code) = if cancelled || parent_cancelled {
            (
                ChildEventKind::Cancelled,
                "The child task was cancelled after its provider stopped.".to_owned(),
                None,
                None,
            )
        } else if budget_settled
            && let Some(Finished {
                error: None,
                output: Some(output),
                ..
            }) = &finished
        {
            let safe = bounded_child_text(&output.text);
            (
                ChildEventKind::Completed,
                output.text.clone(),
                Some(safe),
                None,
            )
        } else {
            (
                ChildEventKind::Failed,
                "The child task did not complete.".to_owned(),
                None,
                Some("CHILD_FAILED".to_owned()),
            )
        };
        if let Some(event_result) = event_result {
            let _ = state
                .service
                .store
                .append_event(
                    &owner,
                    remote_session,
                    child.child_id,
                    Uuid::now_v7(),
                    kind,
                    None,
                    Some(&event_result),
                    None,
                    now(),
                )
                .await;
        } else if kind == ChildEventKind::Failed {
            let _ = state
                .service
                .store
                .append_event(
                    &owner,
                    remote_session,
                    child.child_id,
                    Uuid::now_v7(),
                    kind,
                    None,
                    None,
                    error_code.as_deref(),
                    now(),
                )
                .await;
        } else {
            let _ = state
                .service
                .store
                .append_event(
                    &owner,
                    remote_session,
                    child.child_id,
                    Uuid::now_v7(),
                    kind,
                    None,
                    None,
                    None,
                    now(),
                )
                .await;
        }
        if !parent_cancelled
            && !*parent_control.cancel.borrow()
            && let Some(pending) = parent_control.tools.pending()
            && Some(pending.call_id) == child.parent_call_id
            && pending.name == crate::domain::models::tools::ToolName::InvokeSubagent
        {
            let parent_result = crate::domain::models::tools::ToolResult {
                call_id: pending.call_id,
                name: pending.name,
                output: bounded_child_text(&tool_result),
            };
            let _ = parent_control.submit_tool_if_active(parent_result);
        }
        state
            .service
            .unregister_child_task(remote_session, child.child_id);
        let _ = (child_chat, child_output);
    });
}

fn bounded_child_text(value: &str) -> String {
    let mut safe = String::new();
    for ch in value.chars() {
        if (ch.is_control() && ch != '\n' && ch != '\t')
            || matches!(ch as u32, 0x202a..=0x202e | 0x2066..=0x2069)
        {
            continue;
        }
        if safe.len() + ch.len_utf8() > 32_000 {
            break;
        }
        safe.push(ch);
    }
    safe
}

async fn live_pending(
    state: &OutputActionState,
    owner: &str,
    context: &CallerContext,
    remote_session_id: Uuid,
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
    if state
        .service
        .store
        .child_for_output(owner, remote_session_id, chat_session_id, output_id)
        .await
        .map_err(map_error)?
        .is_some_and(|child| child.status.terminal() || child.cancel_requested)
    {
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
    let pending = match live_pending(
        &state,
        &owner,
        &context,
        remote_session,
        chat_session,
        output,
    )
    .await
    {
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
            "action": created.display.clone(),
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
    let pending = match live_pending(
        &state,
        &owner,
        &context,
        remote_session,
        chat_session,
        output,
    )
    .await
    {
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
        .remote_cli
        .answer_bound_output_action(
            &remote_session.to_string(),
            &owner,
            chat_session,
            output,
            action_id,
            body.approved,
            now.timestamp_millis().max(0) as u64,
        )
        .await
    {
        Ok(_) => match state
            .service
            .store
            .output_action(&owner, remote_session, chat_session, output, action_id)
            .await
        {
            Ok(action) => Envelope::new(
                "Output action answered",
                OutputActionView::from_action(action, now),
            )
            .into_response(),
            Err(error) => map_error(error),
        },
        Err(error) => map_remote_error(error),
    }
}

async fn claim_output_action(
    State(state): State<OutputActionState>,
    UserId(owner): UserId,
    context: CallerContext,
    Path((remote_session, chat_session, output, action_id)): Path<(Uuid, Uuid, Uuid, Uuid)>,
    Json(body): Json<ClaimOutputAction>,
) -> Response {
    let pending = match live_pending(
        &state,
        &owner,
        &context,
        remote_session,
        chat_session,
        output,
    )
    .await
    {
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

async fn child_detail(
    State(service): State<AgentRunService>,
    UserId(owner): UserId,
    Path((session, child)): Path<(Uuid, Uuid)>,
) -> Response {
    match service
        .store
        .events_after(&owner, session, child, i64::MAX as u64, 1)
        .await
    {
        Ok((run, _)) => Envelope::new("Child run loaded", run).into_response(),
        Err(error) => map_error(error),
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
        Ok(event) => {
            service.cancel_child_task(session, child);
            (
                StatusCode::ACCEPTED,
                Envelope::new("Cancellation requested", event),
            )
                .into_response()
        }
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

#[cfg(test)]
mod child_start_tests {
    use super::{cancel_bound_output_if_requested, child_provider_may_start};
    use crate::domain::{
        models::tools::ToolControl,
        services::{
            agent_runs::{AgentRunService, AgentRunStore, ChildRunStatus},
            output_controls::Control,
        },
    };
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };
    use tokio::sync::{broadcast, mpsc, watch};
    use uuid::Uuid;

    fn control() -> (Control, watch::Receiver<bool>) {
        let (cancel, cancelled) = watch::channel(false);
        let (stop_thinking, _) = watch::channel(false);
        let (live, _) = broadcast::channel(1);
        let (tool_results, _) = mpsc::unbounded_channel();
        (
            Control::new(
                Arc::new(cancel),
                Arc::new(stop_thinking),
                live,
                ToolControl::new(tool_results),
            ),
            cancelled,
        )
    }

    #[tokio::test]
    async fn cancelled_child_before_task_registration_never_starts_provider() {
        let (service, store) = AgentRunService::in_memory_with_store(30_000);
        let session = Uuid::now_v7();
        store.register_session(session, "owner").await;
        let child = store
            .create_child(
                "owner",
                session,
                &session.to_string(),
                "researcher",
                "Find one answer",
                1024,
                4,
                3,
                8192,
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        store
            .request_cancel(
                "owner",
                session,
                child.child_id,
                Uuid::now_v7(),
                chrono::Utc::now(),
            )
            .await
            .unwrap();
        // Cancellation persisted before the worker registered its watch sender.
        service.cancel_child_task(session, child.child_id);
        let (cancel_tx, cancel_rx) = watch::channel(false);
        assert!(service.register_child_task(session, child.child_id, cancel_tx));
        let (stored, _) = store
            .events_after("owner", session, child.child_id, 0, 1)
            .await
            .unwrap();
        let provider_started = AtomicBool::new(false);
        if child_provider_may_start(&stored, false, *cancel_rx.borrow()) {
            provider_started.store(true, Ordering::Release);
        }
        assert!(!provider_started.load(Ordering::Acquire));
        assert!(stored.cancel_requested);
        assert_eq!(stored.status, ChildRunStatus::Queued);
    }

    #[test]
    fn cancelled_parent_and_child_block_start_and_cancel_output_at_binding() {
        let child = crate::domain::services::agent_runs::ChildRun {
            session_id: Uuid::now_v7(),
            child_id: Uuid::now_v7(),
            parent_run_id: "root".into(),
            parent_chat_session_id: None,
            parent_output_id: None,
            parent_call_id: None,
            child_chat_session_id: None,
            child_output_id: None,
            role: "researcher".into(),
            task: "Find one answer".into(),
            budget_tokens: 1024,
            depth: 1,
            status: ChildRunStatus::Queued,
            cancel_requested: false,
            budget_used_tokens: 0,
            budget_reserved_tokens: 0,
            budget_settled: false,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        assert!(!child_provider_may_start(&child, true, false));
        assert!(!child_provider_may_start(&child, false, true));

        let (parent_tx, parent_rx) = watch::channel(false);
        let (child_tx, child_rx) = watch::channel(false);
        let (child_output_control, cancelled) = control();
        child_tx.send(true).unwrap();
        assert!(cancel_bound_output_if_requested(
            &child_output_control,
            &parent_rx,
            &child_rx
        ));
        assert!(*cancelled.borrow());

        let (parent_control, parent_cancelled) = control();
        parent_tx.send(true).unwrap();
        assert!(cancel_bound_output_if_requested(
            &parent_control,
            &parent_rx,
            &child_rx
        ));
        assert!(*parent_cancelled.borrow());
    }
}
