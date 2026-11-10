use crate::{core::process_numbers, domain::models::usage::QueueError};
use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
};
impl IntoResponse for QueueError {
    fn into_response(self) -> Response {
        match self {
            Self::HeavyLoad => {
                super::response::error(StatusCode::SERVICE_UNAVAILABLE, process_numbers::HEAVY_LOAD)
            }
            Self::ServerFault => super::response::error(
                StatusCode::INTERNAL_SERVER_ERROR,
                process_numbers::QUEUE_FAULT,
            ),
        }
    }
}
