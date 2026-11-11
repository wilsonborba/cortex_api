use crate::{
    core::config::Config,
    dal::remote::hippocampus::Hippocampus,
    domain::models::{
        memory_graph::{GraphError, GraphQuery},
        storage::AppUser,
    },
};
use serde_json::Value;
#[derive(Clone)]
pub struct MemoryGraph {
    pub hippocampus: Hippocampus,
    pub config: Config,
}
impl MemoryGraph {
    /// One Hippocampus call for the whole scope, or for one node with its context.
    pub async fn load(
        &self,
        scope: &AppUser,
        node: Option<&str>,
        query: &GraphQuery,
    ) -> Result<Value, GraphError> {
        if query.limit == Some(0)
            || query
                .depth
                .is_some_and(|d| d == 0 || d > self.config.memory_graph_max_depth)
            || node.is_some_and(|n| n.trim().is_empty())
        {
            return Err(GraphError::Invalid);
        }
        self.hippocampus
            .graph(scope, node, query.limit, query.depth)
            .await
    }
}
