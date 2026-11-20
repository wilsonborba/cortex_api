use axum::{Router, routing::{get, post}};
use tokio::net::TcpListener;

use cortex_api::dal::remote::reddit::RedditClient;
use cortex_api::domain::services::community_research::{
    CommunityAccessOutcome, CommunityResearchService, evaluate_community_source,
};

#[tokio::test]
async fn reddit_adapter_labels_community_evidence_and_thai_policy_blocks_scraping() {
    let app = Router::new()
        .route(
            "/api/v1/access_token",
            post(|| async { axum::Json(serde_json::json!({ "access_token": "tok_test" })) }),
        )
        .route(
            "/search",
            get(|| async {
                axum::Json(serde_json::json!({
                    "data": {
                        "children": [
                            {
                                "data": {
                                    "id": "abc",
                                    "subreddit": "rust",
                                    "title": "Async tips",
                                    "permalink": "/r/rust/comments/abc/async_tips/",
                                    "score": 42,
                                    "num_comments": 5,
                                    "selftext": "Community thread."
                                }
                            }
                        ]
                    }
                }))
            }),
        );

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let base = format!("http://{addr}");

    let client = RedditClient::new(&base, &base, "cid", "csec", "asodya/1.0").unwrap();
    let svc = CommunityResearchService::new(client);
    let results = svc.search_reddit("async", 5).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].evidence_label.contains("community_evidence"));
    assert_eq!(
        results[0].canonical_url,
        "https://www.reddit.com/r/rust/comments/abc/async_tips/"
    );

    assert!(matches!(
        evaluate_community_source("pantip.com", false),
        CommunityAccessOutcome::Unavailable { .. }
    ));
    assert!(matches!(
        evaluate_community_source("bloggang.com", false),
        CommunityAccessOutcome::Unavailable { .. }
    ));
    assert!(matches!(
        evaluate_community_source("blognone.com", true),
        CommunityAccessOutcome::Allowed { .. }
    ));
}
