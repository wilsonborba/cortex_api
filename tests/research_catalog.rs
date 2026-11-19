use cortex_api::dal::local::research_sources::{
    ResearchCatalogSnapshot, ResearchSource, ResearchSourceStore,
};
use cortex_api::domain::services::research_catalog::ResearchCatalogService;
use std::collections::BTreeMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, RwLock};

struct CountingSourceStore {
    loads: AtomicUsize,
    snapshot: RwLock<ResearchCatalogSnapshot>,
}

impl ResearchSourceStore for CountingSourceStore {
    fn load_catalog(
        &self,
    ) -> Pin<Box<dyn Future<Output = Result<ResearchCatalogSnapshot, String>> + Send + '_>> {
        Box::pin(async move {
            self.loads.fetch_add(1, Ordering::SeqCst);
            Ok(self.snapshot.read().unwrap().clone())
        })
    }
}

fn sample_source(host: &str, subject: &str, access_mode: &str, is_enabled: bool) -> ResearchSource {
    ResearchSource {
        host: host.to_string(),
        name: host.to_string(),
        url: format!("https://{host}/"),
        description: "Test source".to_string(),
        languages: "English".to_string(),
        regions: "Global".to_string(),
        access_note: "Public".to_string(),
        access_mode: access_mode.to_string(),
        is_enabled,
        min_spacing_ms: 1500,
        max_requests_per_run: 3,
        subjects: vec![subject.to_string()],
    }
}

#[tokio::test]
async fn cached_research_catalog_runs_zero_queries_per_request_and_refreshes_on_change() {
    let store = Arc::new(CountingSourceStore {
        loads: AtomicUsize::new(0),
        snapshot: RwLock::new(ResearchCatalogSnapshot {
            sources: vec![
                sample_source(
                    "openalex.org",
                    "reference-and-scholarly-discovery",
                    "credentialed_api",
                    true,
                ),
                sample_source(
                    "www.wikipedia.org",
                    "reference-and-scholarly-discovery",
                    "public_api",
                    true,
                ),
                sample_source(
                    "blocked.example",
                    "reference-and-scholarly-discovery",
                    "unavailable",
                    true,
                ),
            ],
            settings: BTreeMap::from([("global_max_concurrency".to_string(), "4".to_string())]),
        }),
    });
    let catalog = ResearchCatalogService::new(store.clone());

    for _ in 0..20 {
        let enabled = catalog.list_enabled_sources().await.unwrap();
        assert_eq!(enabled.len(), 2);
        let oa = catalog.get_by_host("openalex.org").await.unwrap().unwrap();
        assert_eq!(oa.access_mode, "credentialed_api");
        let by_subj = catalog
            .select_by_subject("reference-and-scholarly-discovery")
            .await
            .unwrap();
        assert_eq!(by_subj.len(), 2);
        assert_eq!(
            catalog
                .setting("global_max_concurrency")
                .await
                .unwrap()
                .as_deref(),
            Some("4")
        );
    }

    assert_eq!(store.loads.load(Ordering::SeqCst), 1);

    // Simulate an admin change followed by explicit refresh.
    store.snapshot.write().unwrap().sources.push(sample_source(
        "www.crossref.org",
        "reference-and-scholarly-discovery",
        "public_api",
        true,
    ));
    catalog.refresh().await.unwrap();
    assert_eq!(store.loads.load(Ordering::SeqCst), 2);
    assert_eq!(catalog.list_enabled_sources().await.unwrap().len(), 3);
    assert_eq!(store.loads.load(Ordering::SeqCst), 2);
}
