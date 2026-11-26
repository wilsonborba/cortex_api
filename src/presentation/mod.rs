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

pub mod agent_runs;
pub mod chat_delete;
pub mod chat_inputs;
pub mod chats;
pub mod file_worker;
pub mod files;
pub mod logs;
pub mod memory_graph;
pub mod personas;
pub mod remote_cli;
pub mod telemetry_reads;

pub struct AppServices {
    pub config: crate::core::config::Config,
    pub runtime: std::sync::Arc<crate::domain::services::chat_runtime::ChatRuntime>,
    pub personas: crate::domain::services::personas::Personas,
    pub chat_delete: crate::domain::services::chat_delete::ChatDelete,
    pub telemetry_reads: crate::domain::services::telemetry_reads::TelemetryReads,
    pub memory_graph: crate::domain::services::memory_graph::MemoryGraph,
    pub chats: crate::dal::local::chats::Chats,
    pub logs: crate::core::logs::LogHub,
    pub ledger: std::sync::Arc<dyn crate::domain::models::usage::UsageLedger>,
    pub remote_cli: crate::domain::services::remote_cli::RemoteCliService,
    pub agent_runs: crate::domain::services::agent_runs::AgentRunService,
}

/// Application router used by the live server binary and end-to-end router tests.
pub fn app_router(services: AppServices) -> Router {
    let files = services.runtime.files.clone();
    let output_action_runtime = services.runtime.clone();
    let output_action_service = services.agent_runs.clone();
    let output_action_remote_cli = services.remote_cli.clone();
    let runs = services.runtime.runs.clone();
    let routes = models::router(services.config.clone())
        .merge(chat_inputs::router(services.runtime))
        .merge(personas::router(services.personas))
        .merge(chat_delete::router(services.chat_delete))
        .merge(telemetry_reads::router(services.telemetry_reads))
        .merge(memory_graph::router(services.memory_graph))
        .merge(files::router(files))
        .merge(logs::router(services.logs))
        .merge(telemetry::router(services.ledger, &services.config))
        .merge(chats::router_with_runs(
            services.chats,
            services.config,
            Some(runs),
        ))
        .merge(remote_cli::routes(services.remote_cli));
    let routes = routes.merge(agent_runs::router(services.agent_runs));
    let routes = routes.merge(agent_runs::output_router(
        output_action_service,
        output_action_runtime,
        output_action_remote_cli,
    ));
    with_response_contract(routes)
}
