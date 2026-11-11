use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::local::chats::Chats,
    domain::{
        models::chat_history::{ChatPatch, ListQuery, PageQuery},
        services::chat_history::ChatHistory,
    },
    presentation::{
        UserId,
        response::{self, Envelope},
    },
};
use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::Response,
    routing::get,
};
use uuid::Uuid;
pub fn router(repo: Chats, config: Config) -> Router {
    Router::new()
        .route("/chats", get(list))
        .route("/chats/{session_id}", get(page).patch(patch))
        .route("/chats/{session_id}/outputs/{output_id}", get(output))
        .with_state(ChatHistory {
            repo,
            hippocampus: crate::dal::remote::hippocampus::Hippocampus::new(&config)
                .expect("validated Hippocampus configuration"),
            config,
        })
}
fn failure(process: ProcessNumber) -> Response {
    let status = match process.number {
        n if n == process_numbers::CHAT_NOT_FOUND.number
            || n == process_numbers::OUTPUT_NOT_FOUND.number =>
        {
            StatusCode::NOT_FOUND
        }
        n if n == process_numbers::HISTORY_FAILED.number => StatusCode::SERVICE_UNAVAILABLE,
        _ => StatusCode::BAD_REQUEST,
    };
    response::error(status, process)
}
async fn list(
    UserId(user): UserId,
    State(state): State<ChatHistory>,
    Query(query): Query<ListQuery>,
) -> Result<Envelope<impl serde::Serialize>, Response> {
    state
        .list(&user, query)
        .await
        .map(|chats| Envelope::new("Chats loaded", chats))
        .map_err(failure)
}
async fn page(
    UserId(user): UserId,
    State(state): State<ChatHistory>,
    Path(session): Path<Uuid>,
    Query(query): Query<PageQuery>,
) -> Result<Envelope<impl serde::Serialize>, Response> {
    state
        .page(&user, session, query)
        .await
        .map(|page| Envelope::new("Chat loaded", page))
        .map_err(failure)
}
async fn output(
    UserId(user): UserId,
    State(state): State<ChatHistory>,
    Path((session, output)): Path<(Uuid, Uuid)>,
) -> Result<Envelope<impl serde::Serialize>, Response> {
    state
        .output(&user, session, output)
        .await
        .map(|output| Envelope::new("Output loaded", output))
        .map_err(failure)
}
async fn patch(
    UserId(user): UserId,
    State(state): State<ChatHistory>,
    Path(session): Path<Uuid>,
    Json(patch): Json<ChatPatch>,
) -> Result<Envelope<impl serde::Serialize>, Response> {
    state
        .patch(&user, session, patch)
        .await
        .map(|chat| Envelope::new("Chat changed", chat))
        .map_err(failure)
}
