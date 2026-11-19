use serde::{Deserialize, Serialize};
use sqlx::{PgPool, Row};
use std::future::Future;
use std::pin::Pin;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchSource {
    pub host: String,
    pub name: String,
    pub url: String,
    pub description: String,
    pub languages: String,
    pub regions: String,
    pub access_note: String,
    pub access_mode: String,
    pub is_enabled: bool,
    pub min_spacing_ms: i32,
    pub max_requests_per_run: i32,
    pub subjects: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchCatalogSnapshot {
    pub sources: Vec<ResearchSource>,
    pub settings: std::collections::BTreeMap<String, String>,
}

pub trait ResearchSourceStore: Send + Sync {
    fn load_catalog(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ResearchCatalogSnapshot, String>> + Send + '_>>;
}

pub struct PostgresResearchSourceStore {
    pool: PgPool,
}

impl PostgresResearchSourceStore {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }
}

impl ResearchSourceStore for PostgresResearchSourceStore {
    fn load_catalog(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ResearchCatalogSnapshot, String>> + Send + '_>> {
        Box::pin(async move {
            let rows = sqlx::query(
                "SELECT s.host, s.name, s.url, s.description, s.languages, s.regions,
                    s.access_note, s.access_mode, s.is_enabled, s.min_spacing_ms,
                    s.max_requests_per_run
             FROM research_sources s
             ORDER BY s.host",
            )
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("Failed to load research_sources: {e}"))?;

            let link_rows = sqlx::query(
            "SELECT host, subject_slug FROM research_source_subjects ORDER BY host, subject_slug",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("Failed to load research_source_subjects: {e}"))?;

            let mut subject_map: std::collections::BTreeMap<String, Vec<String>> =
                std::collections::BTreeMap::new();
            for lr in link_rows {
                let host: String = lr.try_get("host").map_err(|e| e.to_string())?;
                let slug: String = lr.try_get("subject_slug").map_err(|e| e.to_string())?;
                subject_map.entry(host).or_default().push(slug);
            }

            let mut sources = Vec::with_capacity(rows.len());
            for r in rows {
                let host: String = r.try_get("host").map_err(|e| e.to_string())?;
                let subjects = subject_map.remove(&host).unwrap_or_default();
                sources.push(ResearchSource {
                    host,
                    name: r.try_get("name").map_err(|e| e.to_string())?,
                    url: r.try_get("url").map_err(|e| e.to_string())?,
                    description: r.try_get("description").map_err(|e| e.to_string())?,
                    languages: r.try_get("languages").map_err(|e| e.to_string())?,
                    regions: r.try_get("regions").map_err(|e| e.to_string())?,
                    access_note: r.try_get("access_note").map_err(|e| e.to_string())?,
                    access_mode: r.try_get("access_mode").map_err(|e| e.to_string())?,
                    is_enabled: r.try_get("is_enabled").map_err(|e| e.to_string())?,
                    min_spacing_ms: r.try_get("min_spacing_ms").map_err(|e| e.to_string())?,
                    max_requests_per_run: r
                        .try_get("max_requests_per_run")
                        .map_err(|e| e.to_string())?,
                    subjects,
                });
            }

            let setting_rows = sqlx::query(
            "SELECT setting_key, setting_value FROM research_operational_settings ORDER BY setting_key",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("Failed to load research_operational_settings: {e}"))?;
            let mut settings = std::collections::BTreeMap::new();
            for sr in setting_rows {
                let k: String = sr.try_get("setting_key").map_err(|e| e.to_string())?;
                let v: String = sr.try_get("setting_value").map_err(|e| e.to_string())?;
                settings.insert(k, v);
            }

            Ok(ResearchCatalogSnapshot { sources, settings })
        })
    }
}
