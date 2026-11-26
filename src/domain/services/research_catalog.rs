use std::sync::Arc;
use tokio::sync::RwLock;

use crate::dal::local::research_sources::{
    ResearchCatalogSnapshot, ResearchSource, ResearchSourceStore,
};

/// Cached research source catalog. Reads from memory and refreshes only on explicit admin reload.
#[derive(Clone)]
pub struct ResearchCatalogService {
    store: Arc<dyn ResearchSourceStore>,
    cache: Arc<RwLock<Option<ResearchCatalogSnapshot>>>,
}

impl ResearchCatalogService {
    pub fn new(store: Arc<dyn ResearchSourceStore>) -> Self {
        Self {
            store,
            cache: Arc::new(RwLock::new(None)),
        }
    }

    async fn snapshot(&self) -> Result<ResearchCatalogSnapshot, String> {
        {
            let guard = self.cache.read().await;
            if let Some(snap) = guard.as_ref() {
                return Ok(snap.clone());
            }
        }
        self.refresh().await
    }

    /// Reloads the normalized catalog from PostgreSQL into the in-memory cache.
    pub async fn refresh(&self) -> Result<ResearchCatalogSnapshot, String> {
        let fresh = self.store.load_catalog().await?;
        let mut guard = self.cache.write().await;
        *guard = Some(fresh.clone());
        Ok(fresh)
    }

    pub async fn list_enabled_sources(&self) -> Result<Vec<ResearchSource>, String> {
        let snap = self.snapshot().await?;
        Ok(snap
            .sources
            .into_iter()
            .filter(|s| s.is_enabled && s.access_mode != "unavailable")
            .collect())
    }

    pub async fn get_by_host(&self, host: &str) -> Result<Option<ResearchSource>, String> {
        let needle = host.trim().to_ascii_lowercase();
        let snap = self.snapshot().await?;
        Ok(snap
            .sources
            .into_iter()
            .find(|s| s.host.eq_ignore_ascii_case(&needle)))
    }

    pub async fn select_by_subject(
        &self,
        subject_slug: &str,
    ) -> Result<Vec<ResearchSource>, String> {
        let needle = subject_slug.trim().to_ascii_lowercase();
        let snap = self.snapshot().await?;
        Ok(snap
            .sources
            .into_iter()
            .filter(|s| {
                s.is_enabled
                    && s.access_mode != "unavailable"
                    && s.subjects
                        .iter()
                        .any(|sub| sub.eq_ignore_ascii_case(&needle))
            })
            .collect())
    }

    pub async fn setting(&self, key: &str) -> Result<Option<String>, String> {
        let snap = self.snapshot().await?;
        Ok(snap.settings.get(key).cloned())
    }
}
