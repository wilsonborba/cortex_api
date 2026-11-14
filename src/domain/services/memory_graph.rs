use crate::{
    core::config::Config,
    dal::{
        local::memory_records::MemoryRecords,
        remote::{cache::RedisCache, hippocampus::Hippocampus},
    },
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
    /// Cortex chat rows for memory sources and usage (#121). Without it every
    /// memory is a saved note with no usage.
    pub records: Option<MemoryRecords>,
    /// Overview cache (30 s per app and user).
    pub cache: RedisCache,
}
impl MemoryGraph {
    /// Graph routes only: no chat rows and no cache.
    pub fn new(hippocampus: Hippocampus, config: Config) -> Self {
        let cache = RedisCache::new(None, config.cache_operation_timeout);
        Self {
            hippocampus,
            config,
            records: None,
            cache,
        }
    }
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
