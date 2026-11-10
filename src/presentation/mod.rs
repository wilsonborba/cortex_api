pub mod response;
use crate::core::process_numbers;
use axum::{
    Router,
    extract::{FromRequestParts, Request},
    http::{StatusCode, request::Parts},
    middleware::{self, Next},
    response::Response,
};
use response::StructuredResponse;

/// Required only on routes that need a user. LAN A2A calls can omit the user.
pub struct UserId(pub String);
impl<S: Send + Sync> FromRequestParts<S> for UserId {
    type Rejection = Response;
    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        parts
            .headers
            .get("x-uuid")
            .and_then(|v| v.to_str().ok())
            .filter(|s| !s.trim().is_empty())
            .map(|s| Self(s.to_owned()))
            .ok_or_else(|| response::error(StatusCode::BAD_REQUEST, process_numbers::MISSING_USER))
    }
}

/// Apply this outer layer after adding routes. It also wraps extractor errors.
pub fn with_response_contract(router: Router) -> Router {
    router.layer(middleware::from_fn(normalize_framework_error))
}
async fn normalize_framework_error(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    if (status.is_client_error() || status.is_server_error())
        && response.extensions().get::<StructuredResponse>().is_none()
    {
        let mut wrapped = response::error(status, process_numbers::for_status(status));
        // Keep protocol headers such as Allow from framework rejections.
        wrapped.headers_mut().extend(
            response
                .headers()
                .iter()
                .filter(|(name, _)| {
                    *name != axum::http::header::CONTENT_LENGTH
                        && *name != axum::http::header::CONTENT_TYPE
                })
                .map(|(name, value)| (name.clone(), value.clone())),
        );
        wrapped
    } else {
        response
    }
}
pub mod balancer;
pub mod identity;
pub mod model_error;
pub mod models;
pub mod telemetry;

pub mod events;

pub mod chat_delete;
pub mod chat_inputs;
pub mod chats;
pub mod files;
pub mod logs;
pub mod memory_graph;
pub mod personas;
pub mod telemetry_reads;
