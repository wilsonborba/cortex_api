use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use cortex_api::{
    core::config::Config,
    dal::remote::{cache::RedisCache, runs::RunStore, timing::TimingStore},
    domain::{
        models::{cache::CacheKey, history::FinishedStatus, run_record::*},
        services::telemetry_reads::TelemetryReads,
    },
    presentation,
};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    process::{Child, Command, Stdio},
    sync::Arc,
    time::Duration,
};
use tower::ServiceExt;
use uuid::Uuid;
struct LocalValkey {
    child: Child,
    dir: PathBuf,
}
impl Drop for LocalValkey {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
async fn valkey() -> (LocalValkey, RedisCache, String) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let bin = root.join(".scratch/valkey/usr/bin/valkey-server");
    assert!(bin.exists(), "run scripts/setup-test-valkey.sh");
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let dir = root.join(format!(".scratch/valkey-test-{}", Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let child = Command::new(bin)
        .args([
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--save",
            "",
            "--appendonly",
            "no",
            "--daemonize",
            "no",
        ])
        .current_dir(&dir)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("local test Valkey did not start");
    let process = LocalValkey { child, dir };
    for _ in 0..100 {
        if tokio::net::TcpStream::connect(("127.0.0.1", port))
            .await
            .is_ok()
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let url = format!("redis://127.0.0.1:{port}/0");
    (
        process,
        RedisCache::new(Some(&url), Duration::from_secs(1)),
        url,
    )
}
fn state(config: Config, cache: RedisCache, store: Arc<RunStore>) -> TelemetryReads {
    TelemetryReads {
        timing: TimingStore::new(&config, cache),
        store,
        config,
    }
}
fn unavailable() -> Arc<RunStore> {
    Arc::new(
        RunStore::new(
            &Config {
                document_timeout: Duration::from_millis(20),
                ..Default::default()
            },
            "http://127.0.0.1:1/runs".parse().unwrap(),
            String::new(),
            String::new(),
        )
        .unwrap(),
    )
}
async fn request(
    router: &Router,
    path: &str,
    user: Option<&str>,
    admin: bool,
) -> (StatusCode, Value) {
    let mut request = Request::get(path);
    if let Some(user) = user {
        request = request.header("x-uuid", user);
    }
    if admin {
        request = request.header("x-access-level", "admin");
    }
    let response = router
        .clone()
        .oneshot(request.body(Body::empty()).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1_000_000).await.unwrap()).unwrap();
    (status, value)
}
#[tokio::test]
async fn signed_in_reads_only_and_unavailable_data_has_a_number() {
    let config = Config::default();
    let state = state(
        config.clone(),
        RedisCache::new(None, Duration::from_millis(10)),
        unavailable(),
    );
    let router = presentation::with_response_contract(presentation::telemetry_reads::router(state));
    for path in ["/telemetry/timing", "/telemetry/stats", "/telemetry/events"] {
        assert_eq!(
            request(&router, path, None, true).await.0,
            StatusCode::BAD_REQUEST
        );
    }
    let (status, body) = request(&router, "/telemetry/timing", Some("user-a"), false).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 6);
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["expected_ms"].is_null())
    );
    let (status, body) = request(&router, "/telemetry/stats", Some("user-a"), false).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body["data"]["error_number"].is_u64());
    for path in [
        "/telemetry/events?limit=0",
        "/telemetry/events?limit=201",
        "/telemetry/events?user_id=other",
    ] {
        assert_eq!(
            request(&router, path, Some("user-a"), false).await.0,
            StatusCode::BAD_REQUEST
        );
    }
}
#[tokio::test]
#[ignore = "requires the isolated local Valkey binary from scripts/setup-test-valkey.sh; no shared cache keys"]
async fn last_twenty_per_kind_only_completed_and_first_event_average() {
    let (_process, cache, url) = valkey().await;
    let config = Config::default();
    let state = state(config.clone(), cache.clone(), unavailable());
    for web_mode in [WebMode::Fast, WebMode::Medium, WebMode::Deep] {
        for thinking in [false, true] {
            let mode = RunMode { web_mode, thinking };
            assert_eq!(state.expected(&mode).await, None);
            let mut last = Uuid::nil();
            for i in 1..=25 {
                last = Uuid::new_v4();
                state
                    .completed(&mode, last, i * 100, FinishedStatus::Done)
                    .await;
            }
            state
                .completed(&mode, last, 999_999, FinishedStatus::Done)
                .await;
            state
                .completed(&mode, Uuid::new_v4(), 999_999, FinishedStatus::Cancelled)
                .await;
            state
                .completed(&mode, Uuid::new_v4(), 999_999, FinishedStatus::Failed)
                .await;
            assert_eq!(state.expected(&mode).await, Some(1550));
            let raw = cache
                .get(&CacheKey::Timing(cortex_api::dal::remote::timing::kind(
                    &mode,
                )))
                .await
                .unwrap()
                .unwrap();
            assert_eq!(raw.as_array().unwrap().len(), 20);
        }
    }
    let client = redis::Client::open(url).unwrap();
    let mut audit = client.get_multiplexed_async_connection().await.unwrap();
    let mode = RunMode {
        web_mode: WebMode::Fast,
        thinking: false,
    };
    let ttl: i64 = redis::cmd("PTTL")
        .arg(CacheKey::Timing("fast:false".into()).name())
        .query_async(&mut audit)
        .await
        .unwrap();
    assert!(ttl > 0 && ttl <= config.timing_cache_ttl.as_millis() as i64);
    let (sink, mut rx, events) =
        cortex_api::domain::services::events::channel(&config, state.expected(&mode).await);
    sink.send(cortex_api::domain::models::events::NativeEvent::Stage(
        cortex_api::domain::models::events::StageProgress::start(
            cortex_api::domain::models::pipeline::StageName::Accept,
        ),
    ));
    drop(sink);
    events.await.unwrap();
    assert_eq!(rx.recv().await.unwrap().event.data()["expected_ms"], 1550);
    let router = presentation::with_response_contract(presentation::telemetry_reads::router(state));
    let (status, body) = request(&router, "/telemetry/timing", Some("user-a"), false).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body["data"]
            .as_array()
            .unwrap()
            .iter()
            .all(|r| r["expected_ms"] == 1550)
    );
}
fn record(user: &str, total_ms: u64, success: bool) -> RunRecord {
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T00:00:00.123456Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    RunRecord {
        session_id: Uuid::now_v7(),
        output_id: Uuid::new_v4(),
        app_id: "cortex".into(),
        user_id: user.into(),
        created_at: now,
        mode: RunMode {
            web_mode: WebMode::Fast,
            thinking: false,
        },
        total_ms,
        stages: vec![StageRecord {
            name: "answer".into(),
            status: if success { "ok" } else { "failed" }.into(),
            error_number: if success { None } else { Some(1012) },
            notice_number: None,
            time_ms: total_ms,
        }],
        calls: (0..2)
            .map(|i| ModelCall {
                images: None,
                audio: None,
                role: if i == 0 { "queries" } else { "answer" }.into(),
                model: "fixture-model".into(),
                input_tokens: 10,
                output_tokens: 5,
                time_to_first_token_ms: Some(1),
                total_ms: 2,
                started_at: now,
                finished_at: now,
            })
            .collect(),
        counts: RunCounts {
            pages_read: 0,
            chunks_used: 0,
            files: 0,
            file_pages: 0,
            audio_seconds: 0.0,
        },
    }
}
#[tokio::test]
#[ignore = "requires Cortex CouchDB credentials; creates and removes one unique telemetry test database"]
async fn real_couch_views_stats_scope_admin_a2a_and_stable_call_paging() {
    dotenvy::dotenv().ok();
    let config = Config::default();
    let mut url = reqwest::Url::parse(&std::env::var("CORTEX_COUCHDB_URL").unwrap())
        .unwrap_or_else(|_| panic!("Cortex test URL is not valid"));
    let name = format!("cortex_runs_test_telemetry_{}", Uuid::new_v4().simple());
    url.path_segments_mut().unwrap().pop_if_empty().push(&name);
    let user = std::env::var("CORTEX_COUCHDB_USERNAME").unwrap();
    let password = std::env::var("CORTEX_COUCHDB_PASSWORD").unwrap();
    let store =
        Arc::new(RunStore::new(&config, url.clone(), user.clone(), password.clone()).unwrap());
    store.initialize().await.unwrap();
    let a = record("user-a", 100, true);
    let b = record("user-b", 300, false);
    let machine = record("", 200, true);
    for r in [&a, &b, &machine] {
        store.put(r).await.unwrap();
    }
    let router =
        presentation::with_response_contract(presentation::telemetry_reads::router(state(
            config.clone(),
            RedisCache::new(None, Duration::from_millis(10)),
            store,
        )));
    let (status, body) = request(&router, "/telemetry/stats", Some("user-a"), false).await;
    assert_eq!(status, StatusCode::OK);
    let stats = &body["data"][0];
    assert_eq!(stats["total_runs"], 3);
    assert_eq!(stats["avg_latency_ms"], 200.0);
    assert_eq!(stats["avg_input_tokens"], 20.0);
    assert_eq!(stats["avg_output_tokens"], 10.0);
    assert_eq!(stats["avg_total_tokens"], 30.0);
    assert!((stats["success_rate"].as_f64().unwrap() - 2.0 / 3.0).abs() < 1e-9);
    let raw = body.to_string();
    for field in [
        "provider",
        "tier_requested",
        "strategy_id",
        "cost",
        "user_id",
    ] {
        assert!(!raw.contains(field));
    }
    for (owner, id) in [("user-a", a.output_id), ("user-b", b.output_id)] {
        let (status, body) = request(&router, "/telemetry/events", Some(owner), false).await;
        assert_eq!(status, StatusCode::OK);
        let rows = body["data"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        assert!(rows.iter().all(|r| r["output_id"] == id.to_string()));
    }
    let (status, body) = request(&router, "/telemetry/events", Some("admin"), true).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"].as_array().unwrap().len(), 6);
    assert_eq!(
        request(&router, "/telemetry/events", Some("admin"), false)
            .await
            .1["data"],
        json!([])
    );
    let mut cursor = None;
    let mut seen = std::collections::HashSet::new();
    loop {
        let path = cursor
            .as_ref()
            .map(|c| format!("/telemetry/events?limit=1&before={c}"))
            .unwrap_or_else(|| "/telemetry/events?limit=1".into());
        let (status, body) = request(&router, &path, Some("admin"), true).await;
        assert_eq!(status, StatusCode::OK);
        let rows = body["data"].as_array().unwrap();
        if rows.is_empty() {
            break;
        }
        let id = rows[0]["request_id"].as_str().unwrap().to_owned();
        assert!(seen.insert(id.clone()));
        cursor = Some(id);
        assert!(seen.len() <= 6);
    }
    assert_eq!(seen.len(), 6);
    let cursor = format!("/telemetry/events?before={}:0", b.output_id);
    assert_eq!(
        request(&router, &cursor, Some("user-a"), false).await.0,
        StatusCode::BAD_REQUEST
    );
    assert_eq!(
        request(
            &router,
            "/telemetry/events?before=bad",
            Some("user-a"),
            false
        )
        .await
        .0,
        StatusCode::BAD_REQUEST
    );
    let response = reqwest::Client::new()
        .delete(url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .unwrap_or_else(|_| panic!("Cortex telemetry fixture cleanup failed"));
    assert!(response.status().is_success());
}
