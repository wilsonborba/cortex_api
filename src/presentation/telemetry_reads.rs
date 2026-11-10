use crate::{
    core::process_numbers,
    domain::{models::telemetry_reads::*, services::telemetry_reads::TelemetryReads},
    presentation::{
        UserId,
        response::{self, Envelope},
    },
};
use axum::{
    Router,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::Response,
    routing::get,
};
pub fn router(state: TelemetryReads) -> Router {
    Router::new()
        .route("/telemetry/timing", get(timing))
        .route("/telemetry/stats", get(stats))
        .route("/telemetry/events", get(events))
        .with_state(state)
}
fn failure(error: TelemetryReadError) -> Response {
    match error {
        TelemetryReadError::InvalidCursor => {
            response::error(StatusCode::BAD_REQUEST, process_numbers::INVALID_DATA)
        }
        TelemetryReadError::Unavailable => response::error(
            StatusCode::SERVICE_UNAVAILABLE,
            process_numbers::TELEMETRY_UNAVAILABLE,
        ),
    }
}
async fn timing(
    UserId(_): UserId,
    State(state): State<TelemetryReads>,
) -> Envelope<Vec<TimingAverage>> {
    Envelope::new("Timing averages loaded", state.timing().await)
}
async fn stats(
    UserId(_): UserId,
    State(state): State<TelemetryReads>,
) -> Result<Envelope<Vec<GroupStats>>, Response> {
    state
        .stats()
        .await
        .map(|v| Envelope::new("Statistics loaded", v))
        .map_err(failure)
}
async fn events(
    UserId(user): UserId,
    State(state): State<TelemetryReads>,
    headers: HeaderMap,
    Query(page): Query<EventPage>,
) -> Result<Envelope<Vec<CallEvent>>, Response> {
    let admin = headers.get("x-access-level").and_then(|v| v.to_str().ok()) == Some("admin");
    state
        .events(&user, admin, page)
        .await
        .map(|v| Envelope::new("Call events loaded", v))
        .map_err(failure)
}
