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
    response::{
        IntoResponse, Response,
        sse::{Event, Sse},
    },
    routing::{get, post},
};
use futures_util::stream;
use serde_json::json;
use std::collections::VecDeque;
use std::sync::Arc;
use uuid::Uuid;
pub fn router(runtime: Arc<ChatRuntime>) -> Router {
    Router::new()
        .route("/chats", post(create))
        .route("/chats/{session_id}/inputs", post(next))
        .route(
            "/chats/{session_id}/outputs/{output_id}/events",
            get(events),
        )
        .route(
            "/chats/{session_id}/outputs/{output_id}",
            axum::routing::patch(control),
        )
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
    if process.number == process_numbers::OUTPUT_NOT_RUNNING.number {
        return StatusCode::CONFLICT;
    }
    match process.number {
        n if n == process_numbers::CHAT_NOT_FOUND.number
            || n == process_numbers::FILE_NOT_FOUND.number
            || n == process_numbers::OUTPUT_NOT_FOUND.number =>
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
    // A LAN backend may call without x-uuid; it uses only A2A memory (21.3).
    let caller = context
        .for_app(&body.app_id, false)
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
    context: CallerContext,
    Path(session): Path<Uuid>,
    Json(body): Json<NextInput>,
) -> Result<Response, Response> {
    let failure = |process: ProcessNumber| response::error(status(process), process);
    // The app comes from the chat, never from the input body (21.2).
    context.check_peer().map_err(IntoResponse::into_response)?;
    let user = context.user().to_owned();
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
        .for_app(&app, false)
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

/// x-uuid must own the chat and the output (10.4.5). Returns true for a
/// temporary chat.
async fn owned_output(
    runtime: &ChatRuntime,
    context: &CallerContext,
    user: &str,
    session: Uuid,
    output: Uuid,
) -> Result<bool, ProcessNumber> {
    match runtime.chats.owned_chat(user, session).await {
        Ok(Some(chat)) => {
            let scope = crate::domain::models::storage::AppUser {
                app_id: chat.app_id,
                user_id: user.into(),
            };
            match runtime.chats.output(&scope, output).await {
                Ok(Some(o)) if o.session_id == session => Ok(false),
                Ok(_) => Err(process_numbers::OUTPUT_NOT_FOUND),
                Err(_) => Err(process_numbers::HISTORY_FAILED),
            }
        }
        Ok(None) => {
            let app = runtime.temporary.app_of(user, session).await?;
            let caller = context
                .for_app(&app, true)
                .map_err(|_| process_numbers::CHAT_NOT_FOUND)?;
            let chat = runtime.temporary.get(&caller, session).await?;
            if chat.outputs.iter().any(|o| o.output.output_id == output) {
                Ok(true)
            } else {
                Err(process_numbers::OUTPUT_NOT_FOUND)
            }
        }
        Err(_) => Err(process_numbers::HISTORY_FAILED),
    }
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct EventsQuery {
    after: Option<u64>,
}
fn sse_event(event: &crate::domain::models::replay::StoredEvent) -> Event {
    Event::default()
        .id(event.id.to_string())
        .event(event.name.clone())
        .json_data(&event.body)
        .expect("event data serializes")
}
fn last(event: &crate::domain::models::replay::StoredEvent) -> bool {
    matches!(event.name.as_str(), "done" | "error")
}
/// Saved events after `after` (from event 1 without it), then the new events.
async fn events(
    State(runtime): State<Arc<ChatRuntime>>,
    UserId(user): UserId,
    context: CallerContext,
    Path((session, output)): Path<(Uuid, Uuid)>,
    axum::extract::Query(query): axum::extract::Query<EventsQuery>,
) -> Result<Response, Response> {
    let failure = |process: ProcessNumber| response::error(status(process), process);
    // Temporary chats have no reconnect (10.4.8).
    if owned_output(&runtime, &context, &user, session, output)
        .await
        .map_err(failure)?
    {
        return Err(failure(process_numbers::EVENTS_UNAVAILABLE));
    }
    // Subscribe before reading the journal, so no event falls between them.
    let live = runtime.controls.get(output).map(|c| c.live.subscribe());
    let mut saved = VecDeque::new();
    let mut after = query.after.unwrap_or(0);
    loop {
        match runtime.journal.after(output, after).await {
            Ok(Some(page)) if !page.is_empty() => {
                after = page.last().map(|e| e.id).unwrap_or(after);
                saved.extend(page);
            }
            Ok(Some(_)) => break,
            Ok(None) if live.is_some() => break,
            // Expired after one hour: the answer is in the history (10.4.6).
            Ok(None) => {
                return Err(response::error(
                    StatusCode::NOT_FOUND,
                    process_numbers::EVENTS_UNAVAILABLE,
                ));
            }
            Err(_) => {
                return Err(response::error(
                    StatusCode::SERVICE_UNAVAILABLE,
                    process_numbers::EVENTS_UNAVAILABLE,
                ));
            }
        }
    }
    let ended = saved.iter().any(last);
    let live = if ended { None } else { live };
    let body = stream::unfold(
        (saved, live, after, false),
        |(mut saved, mut live, after, done)| async move {
            if done {
                return None;
            }
            if let Some(event) = saved.pop_front() {
                let done = last(&event);
                return Some((
                    Ok::<_, std::convert::Infallible>(sse_event(&event)),
                    (saved, live, after, done),
                ));
            }
            loop {
                match live.as_mut()?.recv().await {
                    Ok(event) if event.id > after => {
                        let done = last(&event);
                        let id = event.id;
                        return Some((Ok(sse_event(&event)), (saved, live, id, done)));
                    }
                    Ok(_) => continue,
                    // A slow reader lost live events; it can reconnect with `after`.
                    Err(_) => return None,
                }
            }
        },
    );
    Ok(Sse::new(body).into_response())
}
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct OutputPatch {
    status: Option<String>,
    thinking: Option<bool>,
}
/// `{"status": "cancelled"}` stops the output (10.5). `{"thinking": false}`
/// ends thinking and goes to the final answer (11).
async fn control(
    State(runtime): State<Arc<ChatRuntime>>,
    UserId(user): UserId,
    context: CallerContext,
    Path((session, output)): Path<(Uuid, Uuid)>,
    Json(patch): Json<OutputPatch>,
) -> Result<Envelope<serde_json::Value>, Response> {
    let failure = |process: ProcessNumber| response::error(status(process), process);
    let cancel = match (patch.status.as_deref(), patch.thinking) {
        (Some("cancelled"), None) => true,
        (None, Some(false)) => false,
        _ => return Err(failure(process_numbers::INVALID_DATA)),
    };
    owned_output(&runtime, &context, &user, session, output)
        .await
        .map_err(failure)?;
    let control = runtime
        .controls
        .get(output)
        .ok_or_else(|| failure(process_numbers::OUTPUT_NOT_RUNNING))?;
    let data = json!({"session_id": session, "output_id": output});
    if cancel {
        let _ = control.cancel.send(true);
        Ok(Envelope::new("Output cancel sent", data))
    } else {
        let _ = control.stop_thinking.send(true);
        Ok(Envelope::new("Thinking stop sent", data))
    }
}
