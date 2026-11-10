use crate::{
    core::process_numbers,
    domain::services::chat_delete::ChatDelete,
    presentation::{
        UserId,
        response::{self, Envelope},
    },
};
use axum::{
    Router,
    extract::{Path, State},
    http::StatusCode,
    response::Response,
    routing::delete,
};
use serde_json::{Value, json};
use uuid::Uuid;
pub fn router(state: ChatDelete) -> Router {
    Router::new()
        .route("/chats/{session_id}", delete(remove))
        .with_state(state)
}
async fn remove(
    UserId(user): UserId,
    State(state): State<ChatDelete>,
    Path(session): Path<Uuid>,
) -> Result<Envelope<Value>, Response> {
    state
        .delete(&user, session)
        .await
        .map(|()| Envelope::new("Chat deleted", json!({"session_id": session})))
        .map_err(|process| {
            let status = match process.number {
                n if n == process_numbers::CHAT_NOT_FOUND.number => StatusCode::NOT_FOUND,
                n if n == process_numbers::CHAT_DELETE_FAILED.number
                    || n == process_numbers::HISTORY_FAILED.number =>
                {
                    StatusCode::SERVICE_UNAVAILABLE
                }
                _ => StatusCode::BAD_REQUEST,
            };
            response::error(status, process)
        })
}
