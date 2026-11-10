use crate::{
    core::process_numbers::{self, ProcessNumber},
    domain::{
        models::{
            chat_runtime::{Turn, TurnRequest},
            chat_session::{NewChat, NextInput},
        },
        services::chat_runtime::{ChatRuntime, Finished},
    },
    presentation::{
        UserId,
        identity::CallerContext,
        response::{self, Envelope},
    },
};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;
pub fn router(runtime: Arc<ChatRuntime>) -> Router {
    Router::new()
        .route("/chats", post(create))
        .route("/chats/{session_id}/inputs", post(next))
        .with_state(runtime)
}
fn status(process: ProcessNumber) -> StatusCode {
    let unavailable = [
        process_numbers::SECURITY_UNAVAILABLE,
        process_numbers::HISTORY_FAILED,
        process_numbers::TEMPORARY_UNAVAILABLE,
        process_numbers::MEMORY_LOAD_FAILED,
        process_numbers::HEAVY_LOAD,
        process_numbers::QUEUE_FAULT,
        process_numbers::OLLAMA_FAILED,
    ];
    match process.number {
        n if n == process_numbers::CHAT_NOT_FOUND.number
            || n == process_numbers::FILE_NOT_FOUND.number =>
        {
            StatusCode::NOT_FOUND
        }
        n if unavailable.iter().any(|p| p.number == n) => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    }
}
async fn respond(
    runtime: &Arc<ChatRuntime>,
    turn: Turn,
    stream: bool,
    message: &'static str,
) -> Response {
    if stream {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        drop(runtime.start(turn, Some(tx)));
        return super::events::sse(rx).into_response();
    }
    let Ok(Finished { ids, error, output }) = runtime.start(turn, None).await else {
        return response::error(
            StatusCode::INTERNAL_SERVER_ERROR,
            process_numbers::SERVER_ERROR,
        );
    };
    // Rejected in Accept: nothing was saved and no id exists (9.1).
    let (Some(ids), Some(output)) = (ids, output) else {
        let process = error.unwrap_or(process_numbers::SERVER_ERROR);
        return response::error(status(process), process);
    };
    let mut data = json!({
        "session_id": ids.session_id,
        "input_id": ids.input_id,
        "output_id": ids.output_id,
        "status": output.status.name(),
        "text": output.text,
        "thinking_text": output.thinking_text,
        "memory_id": output.memory_id,
        "sources": output.sources,
        "memories": output.memories,
    });
    if let Some(process) = error {
        data["error_number"] = process.number.into();
    }
    (StatusCode::CREATED, Envelope::new(message, data)).into_response()
}
async fn create(
    State(runtime): State<Arc<ChatRuntime>>,
    context: CallerContext,
    Json(body): Json<NewChat>,
) -> Result<Response, Response> {
    let caller = context
        .for_app(&body.app_id, true)
        .map_err(IntoResponse::into_response)?;
    let stream = body.stream;
    Ok(respond(
        &runtime,
        Turn::new(caller, TurnRequest::New(body)),
        stream,
        "Chat created",
    )
    .await)
}
async fn next(
    State(runtime): State<Arc<ChatRuntime>>,
    UserId(user): UserId,
    context: CallerContext,
    Path(session): Path<Uuid>,
    Json(body): Json<NextInput>,
) -> Result<Response, Response> {
    let failure = |process: ProcessNumber| response::error(status(process), process);
    // The app comes from the chat, never from the input body (21.2).
    let app = match runtime.chats.owned_chat(&user, session).await {
        Ok(Some(chat)) => chat.app_id,
        Ok(None) => runtime
            .temporary
            .app_of(&user, session)
            .await
            .map_err(failure)?,
        Err(_) => return Err(failure(process_numbers::HISTORY_FAILED)),
    };
    let caller = context
        .for_app(&app, true)
        .map_err(IntoResponse::into_response)?;
    let stream = body.stream;
    Ok(respond(
        &runtime,
        Turn::new(caller, TurnRequest::Next(session, body)),
        stream,
        "Input accepted",
    )
    .await)
}
