use crate::{
    core::process_numbers,
    domain::{
        models::{
            memory_graph::{GraphError, GraphQuery},
            memory_overview::{ItemsQuery, OverviewQuery},
        },
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
use chrono::Utc;
use serde_json::Value;
pub fn router(state: MemoryGraph) -> Router {
    Router::new()
        .route("/memory-graph", get(graph))
        .route("/memory-graph/overview", get(overview))
        .route("/memory-graph/items", get(items))
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
    let scope = caller.storage_scope();
    let mut data = state.load(&scope, node, &query).await.map_err(failure)?;
    // A memory node also gets the readable detail (#121). The old fields
    // stay for one release (cortex #69 removes their use).
    if let Some(id) = node
        && data["node"]["node_type"] == "memory"
    {
        let index = state.index(&scope).await.map_err(failure)?;
        if let (Some(detail), Some(out)) = (index.detail(id), data.as_object_mut()) {
            for (key, value) in detail.as_object().into_iter().flatten() {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    Ok(Envelope::new(message, data))
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
async fn overview(
    State(state): State<MemoryGraph>,
    context: CallerContext,
    Query(query): Query<OverviewQuery>,
) -> Result<Envelope<Value>, Response> {
    let caller = context
        .for_app(&query.app_id, true)
        .map_err(IntoResponse::into_response)?;
    state
        .overview(&caller.storage_scope(), Utc::now())
        .await
        .map(|v| Envelope::new("Memory overview loaded", v))
        .map_err(failure)
}
async fn items(
    State(state): State<MemoryGraph>,
    context: CallerContext,
    Query(query): Query<ItemsQuery>,
) -> Result<Envelope<Value>, Response> {
    let caller = context
        .for_app(&query.app_id, true)
        .map_err(IntoResponse::into_response)?;
    state
        .items(&caller.storage_scope(), &query, Utc::now())
        .await
        .map(|v| Envelope::new("Memory items loaded", v))
        .map_err(failure)
}
