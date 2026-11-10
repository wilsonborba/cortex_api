use serde::Deserialize;
/// Query of `GET /memory-graph` and `GET /memory-graph/nodes/{node_id}`.
#[derive(Debug, Deserialize)]
pub struct GraphQuery {
    pub app_id: String,
    pub limit: Option<usize>,
    pub depth: Option<usize>,
}
#[derive(Debug, PartialEq, Eq)]
pub enum GraphError {
    Invalid,
    NotFound,
    Unavailable,
}
