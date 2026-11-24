use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{
        IntoResponse, Response,
        sse::{Event, KeepAlive, Sse},
    },
    routing::{delete, get, post},
};
use futures_util::StreamExt;
use serde::Deserialize;
use std::collections::VecDeque;
use std::convert::Infallible;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::process_numbers;
use crate::domain::services::remote_cli::{
    RemoteCliError, RemoteCliEvent, RemoteCliService, RemoteEventKind,
};
use crate::presentation::{
    UserId,
    response::{self, Envelope},
};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn map_err(err: RemoteCliError) -> Response {
    match err {
        RemoteCliError::NotFound => response::error(
            StatusCode::NOT_FOUND,
            process_numbers::REMOTE_SESSION_NOT_FOUND,
        ),
        RemoteCliError::Forbidden => response::error(
            StatusCode::FORBIDDEN,
            process_numbers::REMOTE_SESSION_FORBIDDEN,
        ),
        RemoteCliError::AlreadyAnswered => response::error(
            StatusCode::CONFLICT,
            process_numbers::REMOTE_APPROVAL_ANSWERED,
        ),
        RemoteCliError::BadRequest(_) => {
            response::error(StatusCode::BAD_REQUEST, process_numbers::INVALID_DATA)
        }
        RemoteCliError::Unavailable => response::error(
            StatusCode::SERVICE_UNAVAILABLE,
            process_numbers::REMOTE_CLI_UNAVAILABLE,
        ),
    }
}

#[derive(Deserialize)]
pub struct RegisterReq {
    pub device_name: String,
    pub platform: String,
}

#[derive(Deserialize)]
pub struct PromptReq {
    pub prompt: String,
    #[serde(default)]
    pub file_ids: Vec<String>,
}

#[derive(Deserialize)]
pub struct PostCliEventReq {
    pub kind: RemoteEventKind,
    pub payload: serde_json::Value,
    pub ack_command_id: Option<u64>,
}

#[derive(Deserialize)]
pub struct ApprovalReq {
    pub approved: bool,
}

#[derive(Deserialize)]
pub struct CancelReq {
    pub turn_id: Option<String>,
}

#[derive(Deserialize)]
pub struct CursorQuery {
    pub cursor: Option<u64>,
    pub after: Option<u64>,
}

fn parse_cursor(q: CursorQuery, headers: &HeaderMap) -> u64 {
    q.cursor.or(q.after).unwrap_or_else(|| {
        headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
    })
}

pub fn routes(service: RemoteCliService) -> Router {
    Router::new()
        .route(
            "/remote/sessions",
            post(register_session).get(list_sessions),
        )
        .route("/remote/sessions/{id}", delete(unregister_session))
        .route("/remote/sessions/{id}/commands", get(cli_commands_sse))
        .route(
            "/remote/sessions/{id}/events",
            post(post_cli_event).get(ctrl_events_sse),
        )
        .route("/remote/sessions/{id}/prompts", post(send_prompt))
        .route(
            "/remote/sessions/{id}/approvals/{approval_id}",
            post(answer_approval),
        )
        .route("/remote/sessions/{id}/cancel", post(cancel_turn))
        .with_state(service)
}

async fn register_session(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Json(req): Json<RegisterReq>,
) -> Response {
    match svc
        .register(&user, &req.device_name, &req.platform, now_ms())
        .await
    {
        Ok(view) => (
            StatusCode::CREATED,
            Envelope::new("Remote session registered", view),
        )
            .into_response(),
        Err(e) => map_err(e),
    }
}

async fn list_sessions(State(svc): State<RemoteCliService>, UserId(user): UserId) -> Response {
    match svc.list_sessions(&user, now_ms()).await {
        Ok(sessions) => Envelope::new(
            "Remote sessions loaded",
            serde_json::json!({ "sessions": sessions }),
        )
        .into_response(),
        Err(e) => map_err(e),
    }
}

async fn unregister_session(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
) -> Response {
    match svc.unregister(&id, &user).await {
        Ok(()) => Envelope::new(
            "Remote session unregistered",
            serde_json::json!({ "session_id": id }),
        )
        .into_response(),
        Err(e) => map_err(e),
    }
}

async fn send_prompt(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Json(req): Json<PromptReq>,
) -> Response {
    match svc
        .send_prompt(&id, &user, &req.prompt, req.file_ids, now_ms())
        .await
    {
        Ok(ev) => (
            StatusCode::ACCEPTED,
            Envelope::new("Remote prompt queued", ev),
        )
            .into_response(),
        Err(e) => map_err(e),
    }
}

async fn post_cli_event(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Json(req): Json<PostCliEventReq>,
) -> Response {
    match svc
        .post_cli_event(
            &id,
            &user,
            req.kind,
            req.payload,
            req.ack_command_id,
            now_ms(),
        )
        .await
    {
        Ok(ev) => (
            StatusCode::CREATED,
            Envelope::new("Remote event recorded", ev),
        )
            .into_response(),
        Err(e) => map_err(e),
    }
}

async fn answer_approval(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path((id, approval_id)): Path<(String, String)>,
    Json(req): Json<ApprovalReq>,
) -> Response {
    match svc
        .answer_approval(&id, &user, &approval_id, req.approved, now_ms())
        .await
    {
        Ok(ev) => Envelope::new("Approval recorded", ev).into_response(),
        Err(e) => map_err(e),
    }
}

async fn cancel_turn(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Json(req): Json<CancelReq>,
) -> Response {
    match svc.cancel_turn(&id, &user, req.turn_id, now_ms()).await {
        Ok(ev) => (
            StatusCode::ACCEPTED,
            Envelope::new("Remote turn cancelled", ev),
        )
            .into_response(),
        Err(e) => map_err(e),
    }
}

struct DisconnectGuard {
    svc: RemoteCliService,
    session_id: String,
    user: String,
    active: bool,
}

impl Drop for DisconnectGuard {
    fn drop(&mut self) {
        if self.active {
            let svc = self.svc.clone();
            let session_id = self.session_id.clone();
            let user = self.user.clone();
            if let Ok(handle) = tokio::runtime::Handle::try_current() {
                handle.spawn(async move {
                    let _ = svc
                        .set_cli_connected(&session_id, &user, false, now_ms())
                        .await;
                });
            }
        }
    }
}

struct StreamState {
    svc: RemoteCliService,
    session_id: String,
    user: String,
    is_cli: bool,
    last_sent_id: u64,
    buffer: VecDeque<RemoteCliEvent>,
    rx: tokio::sync::broadcast::Receiver<u64>,
    pubsub: Option<redis::aio::PubSub>,
    _guard: DisconnectGuard,
}

fn event_to_sse(ev: &RemoteCliEvent) -> Event {
    let data = serde_json::to_string(ev).unwrap_or_default();
    Event::default().id(ev.id.to_string()).data(data)
}

async fn cli_commands_sse(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Query(q): Query<CursorQuery>,
    headers: HeaderMap,
) -> Response {
    let cursor = parse_cursor(q, &headers);
    if let Err(e) = svc.set_cli_connected(&id, &user, true, now_ms()).await {
        return map_err(e);
    }
    let pubsub = svc.subscribe_cli_pubsub(&id).await;
    let (replay, rx) = match svc.cli_commands_since(&id, &user, cursor).await {
        Ok(pair) => pair,
        Err(e) => return map_err(e),
    };
    sse_from_store(svc, id, user, true, cursor, replay, rx, pubsub).into_response()
}

async fn ctrl_events_sse(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Query(q): Query<CursorQuery>,
    headers: HeaderMap,
) -> Response {
    let cursor = parse_cursor(q, &headers);
    let pubsub = svc.subscribe_ctrl_pubsub(&id).await;
    let (replay, rx) = match svc.ctrl_events_since(&id, &user, cursor).await {
        Ok(pair) => pair,
        Err(e) => return map_err(e),
    };
    sse_from_store(svc, id, user, false, cursor, replay, rx, pubsub).into_response()
}

#[allow(clippy::too_many_arguments)]
fn sse_from_store(
    svc: RemoteCliService,
    session_id: String,
    user: String,
    is_cli: bool,
    cursor: u64,
    replay: Vec<RemoteCliEvent>,
    rx: tokio::sync::broadcast::Receiver<u64>,
    pubsub: Option<redis::aio::PubSub>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let guard = DisconnectGuard {
        svc: svc.clone(),
        session_id: session_id.clone(),
        user: user.clone(),
        active: is_cli,
    };
    let state = StreamState {
        svc,
        session_id,
        user,
        is_cli,
        last_sent_id: cursor,
        buffer: replay.into_iter().collect(),
        rx,
        pubsub,
        _guard: guard,
    };
    let stream = futures_util::stream::unfold(state, |mut st| async move {
        loop {
            if let Some(ev) = st.buffer.pop_front() {
                if ev.id > st.last_sent_id {
                    st.last_sent_id = ev.id;
                    return Some((Ok::<Event, Infallible>(event_to_sse(&ev)), st));
                }
                continue;
            }
            let woke = if let Some(ref mut ps) = st.pubsub {
                let mut msg_stream = ps.on_message();
                tokio::select! {
                    msg = msg_stream.next() => msg.is_some(),
                    res = st.rx.recv() => match res {
                        Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => false,
                    }
                }
            } else {
                match st.rx.recv().await {
                    Ok(_) | Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => true,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => false,
                }
            };
            if !woke {
                return None;
            }
            let next_rows = if st.is_cli {
                st.svc
                    .cli_commands_since(&st.session_id, &st.user, st.last_sent_id)
                    .await
                    .map(|(rows, _)| rows)
            } else {
                st.svc
                    .ctrl_events_since(&st.session_id, &st.user, st.last_sent_id)
                    .await
                    .map(|(rows, _)| rows)
            };
            match next_rows {
                Ok(rows) => st.buffer.extend(rows),
                Err(_) => return None,
            }
        }
    });
    Sse::new(stream).keep_alive(KeepAlive::default())
}
