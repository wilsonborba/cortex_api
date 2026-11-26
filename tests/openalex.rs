use axum::{Router, routing::get};
use tokio::net::TcpListener;

use cortex_api::dal::remote::openalex::OpenAlexClient;
use cortex_api::domain::services::openalex::OpenAlexResearchService;
use cortex_api::domain::services::research_router::EvidenceStage;

#[tokio::test]
async fn openalex_metadata_is_labeled_and_not_confused_with_read_content() {
    let app = Router::new()
        .route(
            "/works",
            get(|| async {
                axum::Json(serde_json::json!({
                    "results": [
                        {
                            "id": "https://openalex.org/W1",
                            "doi": "https://doi.org/10.1000/1",
                            "title": "Open Access Study",
                            "publication_year": 2026,
                            "open_access": { "is_oa": true, "oa_url": null },
                            "abstract_inverted_index": { "Climate": [0], "model": [1] }
                        },
                        {
                            "id": "https://openalex.org/W2",
                            "doi": null,
                            "title": "Closed Paper",
                            "publication_year": 2025,
                            "open_access": { "is_oa": false, "oa_url": null },
                            "abstract_inverted_index": null
                        }
                    ]
                }))
            }),
        )
        .route("/oa/paper1", get(|| async { "Full paper text body." }));

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let base = format!("http://{addr}");
    let client = OpenAlexClient::new(&base, Some("test_key_never_logged".into()), None).unwrap();
    let svc = OpenAlexResearchService::new(client);

    // Metadata-only mode
    let meta_only = svc.gather("climate", 2, false).await.unwrap();
    assert_eq!(meta_only.len(), 2);
    assert_eq!(meta_only[0].stage, EvidenceStage::MetadataOnly);
    assert!(meta_only[0].limitation_label.contains("metadata_only"));
    assert_eq!(
        meta_only[0].work.abstract_text.as_deref(),
        Some("Climate model")
    );

    // Read-when-available mode without oa_url still honestly reports MetadataOnly
    let try_read = svc.gather("climate", 2, true).await.unwrap();
    assert_eq!(try_read[0].stage, EvidenceStage::MetadataOnly);
    assert!(try_read[0].limitation_label.contains("no open-access"));

    // Direct OA content fetch succeeds
    let oa_client = OpenAlexClient::new(&base, None, None).unwrap();
    let body = oa_client
        .fetch_oa_content(&format!("{base}/oa/paper1"), 1024)
        .await
        .unwrap();
    assert_eq!(body, "Full paper text body.");
}
