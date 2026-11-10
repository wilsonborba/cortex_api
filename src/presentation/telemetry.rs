use crate::{
    core::{config::Config, process_numbers},
    domain::models::{
        telemetry::{UsageData, UsageUnavailable},
        usage::UsageLedger,
    },
    presentation::{
        UserId,
        response::{self, Envelope},
    },
};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use std::sync::Arc;
#[derive(Clone)]
struct UsageState {
    ledger: Arc<dyn UsageLedger>,
    window_hours: i64,
}
pub fn router(ledger: Arc<dyn UsageLedger>, config: &Config) -> Router {
    Router::new()
        .route("/telemetry/usage", get(usage))
        .with_state(UsageState {
            ledger,
            window_hours: config.usage_window_hours,
        })
}
async fn usage(
    UserId(user): UserId,
    State(state): State<UsageState>,
) -> Result<Envelope<UsageData>, UsageUnavailable> {
    let share = state
        .ledger
        .share(&user, chrono::Utc::now())
        .await
        .map_err(|_| UsageUnavailable)?;
    Ok(Envelope::new(
        "Usage loaded",
        UsageData::new(state.window_hours, share),
    ))
}
impl IntoResponse for UsageUnavailable {
    fn into_response(self) -> Response {
        response::error(
            StatusCode::SERVICE_UNAVAILABLE,
            process_numbers::USAGE_UNAVAILABLE,
        )
    }
}
