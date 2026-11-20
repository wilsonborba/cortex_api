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
use futures_util::stream;
use serde::Deserialize;
use std::convert::Infallible;
use std::time::{SystemTime, UNIX_EPOCH};

use crate::core::process_numbers;
use crate::domain::services::remote_cli::{RemoteCliError, RemoteCliService, RemoteEventKind};
use crate::presentation::{UserId, response};

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

fn map_err(err: RemoteCliError) -> Response {
    match err {
        RemoteCliError::NotFound => response::error(StatusCode::NOT_FOUND, process_numbers::for_status(StatusCode::NOT_FOUND)),
        RemoteCliError::Forbidden => response::error(StatusCode::FORBIDDEN, process_numbers::for_status(StatusCode::FORBIDDEN)),
        RemoteCliError::AlreadyAnswered => response::error(StatusCode::CONFLICT, process_numbers::for_status(StatusCode::CONFLICT)),
        RemoteCliError::BadRequest(_) => response::error(StatusCode::BAD_REQUEST, process_numbers::for_status(StatusCode::BAD_REQUEST)),
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
}

fn parse_cursor(q: CursorQuery, headers: &HeaderMap) -> u64 {
    q.cursor.unwrap_or_else(|| {
        headers
            .get("last-event-id")
            .and_then(|v| v.to_str().ok())
            .and_then(|s| s.trim().parse::<u64>().ok())
            .unwrap_or(0)
    })
}

pub fn routes(service: RemoteCliService) -> Router {
    Router::new()
        .route("/api/v1/remote/sessions", post(register_session).get(list_sessions))
        .route("/api/v1/remote/sessions/{id}", delete(unregister_session))
        .route("/api/v1/remote/sessions/{id}/commands", get(cli_commands_sse))
        .route("/api/v1/remote/sessions/{id}/events", post(post_cli_event).get(ctrl_events_sse))
        .route("/api/v1/remote/sessions/{id}/prompts", post(send_prompt))
        .route("/api/v1/remote/sessions/{id}/approvals/{approval_id}", post(answer_approval))
        .route("/api/v1/remote/sessions/{id}/cancel", post(cancel_turn))
        .with_state(service)
}

async fn register_session(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Json(req): Json<RegisterReq>,
) -> Response {
    match svc.register(&user, &req.device_name, &req.platform, now_ms()).await {
        Ok(view) => (StatusCode::CREATED, Json(view)).into_response(),
        Err(e) => map_err(e),
    }
}

async fn list_sessions(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
) -> Response {
    let sessions = svc.list_sessions(&user, now_ms()).await;
    (StatusCode::OK, Json(serde_json::json!({ "sessions": sessions }))).into_response()
}

async fn unregister_session(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
) -> Response {
    match svc.unregister(&id, &user).await {
        Ok(()) => StatusCode::NO_CONTENT.into_response(),
        Err(e) => map_err(e),
    }
}

async fn send_prompt(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Json(req): Json<PromptReq>,
) -> Response {
    match svc.send_prompt(&id, &user, &req.prompt, req.file_ids, now_ms()).await {
        Ok(ev) => (StatusCode::ACCEPTED, Json(ev)).into_response(),
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
        .post_cli_event(&id, &user, req.kind, req.payload, req.ack_command_id, now_ms())
        .await
    {
        Ok(ev) => (StatusCode::CREATED, Json(ev)).into_response(),
        Err(e) => map_err(e),
    }
}

async fn answer_approval(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path((id, approval_id)): Path<(String, String)>,
    Json(req): Json<ApprovalReq>,
) -> Response {
    match svc.answer_approval(&id, &user, &approval_id, req.approved, now_ms()).await {
        Ok(ev) => (StatusCode::OK, Json(ev)).into_response(),
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
        Ok(ev) => (StatusCode::ACCEPTED, Json(ev)).into_response(),
        Err(e) => map_err(e),
    }
}

async fn cli_commands_sse(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Query(q): Query<CursorQuery>,
    headers: HeaderMap,
) -> Response {
    let cursor = parse_cursor(q, &headers);
    let (replay, rx) = match svc.cli_commands_since(&id, &user, cursor).await {
        Ok(pair) => pair,
        Err(e) => return map_err(e),
    };
    sse_from_replay_and_rx(replay, rx).into_response()
}

async fn ctrl_events_sse(
    State(svc): State<RemoteCliService>,
    UserId(user): UserId,
    Path(id): Path<String>,
    Query(q): Query<CursorQuery>,
    headers: HeaderMap,
) -> Response {
    let cursor = parse_cursor(q, &headers);
    let (replay, rx) = match svc.ctrl_events_since(&id, &user, cursor).await {
        Ok(pair) => pair,
        Err(e) => return map_err(e),
    };
    sse_from_replay_and_rx(replay, rx).into_response()
}

fn sse_from_replay_and_rx(
    replay: Vec<crate::domain::services::remote_cli::RemoteCliEvent>,
    rx: tokio::sync::broadcast::Receiver<crate::domain::services::remote_cli::RemoteCliEvent>,
) -> Sse<impl futures_util::Stream<Item = Result<Event, Infallible>>> {
    let max_replay = replay.last().map(|e| e.id).unwrap_or(0);
    let replay_stream = stream::iter(replay.into_iter().map(|ev| {
        let data = serde_json::to_string(&ev).unwrap_or_default();
        Ok::<Event, Infallible>(Event::default().id(ev.id.to_string()).data(data))
    }));
    let live_stream = stream::unfold(rx, move |mut rx| async move {
        loop {
            match rx.recv().await {
                Ok(ev) if ev.id > max_replay => {
                    let data = serde_json::to_string(&ev).unwrap_or_default();
                    return Some((Ok::<Event, Infallible>(Event::default().id(ev.id.to_string()).data(data)), rx));
                }
                Ok(_) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(tokio::sync::broadcast::error::RecvError::Closed) => return None,
            }
        }
    });
    Sse::new(futures_util::StreamExt::chain(replay_stream, live_stream))
        .keep_alive(KeepAlive::default())
}
