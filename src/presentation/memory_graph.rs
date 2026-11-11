use crate::{
    core::process_numbers,
    domain::{
        models::memory_graph::{GraphError, GraphQuery},
        services::memory_graph::MemoryGraph,
    },
    presentation::{
        identity::CallerContext,
        response::{self, Envelope},
    },
};
use axum::{
    Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::get,
};
use serde_json::Value;
pub fn router(state: MemoryGraph) -> Router {
    Router::new()
        .route("/memory-graph", get(graph))
        .route("/memory-graph/nodes/{node_id}", get(node))
        .with_state(state)
}
fn failure(error: GraphError) -> Response {
    match error {
        GraphError::Invalid => {
            response::error(StatusCode::BAD_REQUEST, process_numbers::INVALID_DATA)
        }
        GraphError::NotFound => response::error(
            StatusCode::NOT_FOUND,
            process_numbers::MEMORY_NODE_NOT_FOUND,
        ),
        GraphError::Unavailable => response::error(
            StatusCode::SERVICE_UNAVAILABLE,
            process_numbers::MEMORY_LOAD_FAILED,
        ),
    }
}
async fn load(
    state: &MemoryGraph,
    context: CallerContext,
    node: Option<&str>,
    query: GraphQuery,
    message: &'static str,
) -> Result<Envelope<Value>, Response> {
    let caller = context
        .for_app(&query.app_id, true)
        .map_err(IntoResponse::into_response)?;
    state
        .load(&caller.storage_scope(), node, &query)
        .await
        .map(|v| Envelope::new(message, v))
        .map_err(failure)
}
async fn graph(
    State(state): State<MemoryGraph>,
    context: CallerContext,
    Query(query): Query<GraphQuery>,
) -> Result<Envelope<Value>, Response> {
    load(&state, context, None, query, "Memory graph loaded").await
}
async fn node(
    State(state): State<MemoryGraph>,
    context: CallerContext,
    Path(node_id): Path<String>,
    Query(query): Query<GraphQuery>,
) -> Result<Envelope<Value>, Response> {
    load(
        &state,
        context,
        Some(&node_id),
        query,
        "Memory graph node loaded",
    )
    .await
}
