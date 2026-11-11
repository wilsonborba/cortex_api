use crate::{
    core::{config::Config, process_numbers},
    domain::models::model_catalog::{InputTooLarge, ModelsData},
    presentation::response::{self, Envelope},
};
use axum::{
    Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
pub fn router(config: Config) -> Router {
    Router::new()
        .route(
            "/models",
            get(|State(config): State<Config>| async move {
                Envelope::new("Active model", ModelsData::from(&config))
            }),
        )
        .with_state(config)
}
impl IntoResponse for InputTooLarge {
    fn into_response(self) -> Response {
        response::error(StatusCode::BAD_REQUEST, process_numbers::INPUT_TOKEN_LIMIT)
    }
}
