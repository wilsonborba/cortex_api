use cortex_api::{
    core::config::Config,
    dal::remote::search::{canonical_url, duckduckgo_html, wikipedia_json},
    domain::{
        models::{search::*, usage::Pending},
        services::search::{WebSearch, rrf},
    },
};
use std::sync::Arc;
use tokio::sync::Barrier;
const DDG: &str = include_str!("fixtures/search/ddg-results-double.html");
const BLOCKED: &str = include_str!("fixtures/search/ddg-202-recorded.html");
const WIKI: &[u8] = include_bytes!("fixtures/search/wikipedia-recorded.json");
fn hit(url: &str) -> SearchHit {
    SearchHit {
        title: url.into(),
        url: url.into(),
    }
}
#[test]
fn recorded_responses_and_constructed_result_parser() {
    assert!(matches!(
        duckduckgo_html(202, BLOCKED, 10),
        Err(SearchFailure::Blocked)
    ));
    assert!(matches!(
        duckduckgo_html(200, BLOCKED, 10),
        Err(SearchFailure::Blocked)
    ));
    assert!(matches!(
        duckduckgo_html(500, "unavailable", 10),
        Err(SearchFailure::Unavailable)
    ));
    let ddg = duckduckgo_html(200, DDG, 10).unwrap();
    assert_eq!(ddg.len(), 2);
    assert_eq!(ddg[0].url, "https://www.rust-lang.org/");
    assert_eq!(ddg[0].title, "Rust language");
    assert_eq!(duckduckgo_html(200, DDG, 1).unwrap().len(), 1);
    assert!(
        duckduckgo_html(200, "<div class='no-results'>No results</div>", 10)
            .unwrap()
            .is_empty()
    );
    let wiki = wikipedia_json(WIKI, 10).unwrap();
    assert_eq!(wiki.len(), 2);
    assert_eq!(wiki[0].title, "Rust (programming language)");
    assert_eq!(
        wiki[0].url,
        "https://en.wikipedia.org/wiki/Rust_(programming_language)"
    );
    assert_eq!(wikipedia_json(WIKI, 1).unwrap().len(), 1);
    assert!(wikipedia_json(b"{}", 10).is_err());
    assert!(wikipedia_json(br#"["q",["title"],[],[]]"#, 10).is_err());
    assert_eq!(
        canonical_url("https://EXAMPLE.org:443/a#fragment").unwrap(),
        "https://example.org/a"
    );
    assert!(canonical_url("file:///etc/passwd").is_none());
}
#[test]
fn rrf_known_ranks_url_dedup_and_stable_ties() {
    let a = "https://example.org/a";
    let b = "https://example.org/b";
    let c = "https://example.org/c";
    let result = rrf(
        &[
            vec![hit(a), hit(b), hit(&format!("{a}#again"))],
            vec![hit(b), hit(c)],
        ],
        60,
    );
    assert_eq!(result.len(), 3);
    assert_eq!(result[0].hit.url, b);
    assert!((result[0].score - (1.0 / 62.0 + 1.0 / 61.0)).abs() < 1e-12);
    assert!((result[1].score - 1.0 / 61.0).abs() < 1e-12);
    assert_eq!(result[1].hit.url, a);
    assert_eq!(result[2].hit.url, c);
    let tied = rrf(&[vec![hit(a)], vec![hit(b)]], 60);
    assert_eq!(tied[0].hit.url, a);
    assert_eq!(tied[1].hit.url, b);
}
struct Adapter {
    barrier: Arc<Barrier>,
    fail: bool,
}
impl SearchAdapter for Adapter {
    fn name(&self) -> &'static str {
        "test-adapter"
    }
    fn search<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>> {
        Box::pin(async move {
            self.barrier.wait().await;
            if self.fail {
                Err(SearchFailure::Blocked)
            } else {
                Ok((0..12)
                    .map(|i| hit(&format!("https://example.org/{i}")))
                    .collect())
            }
        })
    }
}
#[tokio::test]
async fn concurrent_adapters_limit_partial_failure_and_all_failed_notice() {
    for failures in [[false, false], [true, false], [true, true]] {
        let barrier = Arc::new(Barrier::new(2));
        let adapters = failures
            .into_iter()
            .map(|fail| {
                Arc::new(Adapter {
                    barrier: barrier.clone(),
                    fail,
                }) as Arc<dyn SearchAdapter>
            })
            .collect();
        let search = WebSearch::with_adapters(&Config::default(), adapters);
        let result =
            tokio::time::timeout(std::time::Duration::from_secs(1), search.search("question"))
                .await
                .unwrap();
        if failures == [true, true] {
            assert!(result.hits.is_empty());
            assert_eq!(result.notice.unwrap().number, 1020);
        } else {
            assert_eq!(result.hits.len(), 10);
            assert!(result.notice.is_none());
        }
    }
}
#[tokio::test]
async fn real_http_adapters_send_fixed_agent_and_the_query_to_both() {
    use axum::{
        Router,
        extract::{Query, State},
        http::{HeaderMap, StatusCode},
        response::IntoResponse,
        routing::get,
    };
    use std::{collections::HashMap, sync::Mutex};
    type Calls = Arc<Mutex<Vec<(String, String)>>>;
    let calls: Calls = Default::default();
    async fn ddg(
        State(calls): State<Calls>,
        headers: HeaderMap,
        Query(query): Query<HashMap<String, String>>,
    ) -> impl IntoResponse {
        assert_eq!(headers["user-agent"], "AsodyaCortex/2");
        calls
            .lock()
            .unwrap()
            .push(("ddg".into(), query["q"].clone()));
        (StatusCode::ACCEPTED, BLOCKED)
    }
    async fn wiki(
        State(calls): State<Calls>,
        headers: HeaderMap,
        Query(query): Query<HashMap<String, String>>,
    ) -> impl IntoResponse {
        assert_eq!(headers["user-agent"], "AsodyaCortex/2");
        assert_eq!(query["limit"], "10");
        assert_eq!(query["action"], "opensearch");
        calls
            .lock()
            .unwrap()
            .push(("wiki".into(), query["search"].clone()));
        WIKI
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let app = Router::new()
        .route("/ddg", get(ddg))
        .route("/wiki", get(wiki))
        .with_state(calls.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let config = Config {
        duckduckgo_html_url: format!("http://{address}/ddg"),
        wikipedia_api_url: format!("http://{address}/wiki"),
        ..Default::default()
    };
    let result = WebSearch::new(&config).unwrap().search("Rust ไทย").await;
    assert_eq!(result.hits.len(), 2);
    assert!(result.notice.is_none());
    let recorded = calls.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert!(recorded.iter().all(|(_, query)| query == "Rust ไทย"));
    drop(recorded);
    task.abort();
}
