use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::{Multipart, Path, Query, State},
    http::{Request, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cortex_api::{
    core::config::Config,
    dal::remote::{cache::RedisCache, hippocampus::Hippocampus},
    domain::services::{
        extraction::DocumentExtractor, files::FileProxy, temporary::TemporaryChats,
    },
    presentation::{files::router, with_response_contract},
};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tower::ServiceExt;
type Calls = Arc<Mutex<Vec<(String, HashMap<String, String>)>>>;
#[derive(Clone, Default)]
struct Double {
    calls: Calls,
    fail: Arc<AtomicBool>,
}
async fn save(State(d): State<Double>, mut form: Multipart) -> Response {
    let mut fields = HashMap::new();
    while let Some(field) = form.next_field().await.unwrap() {
        let name = field.name().unwrap().to_owned();
        if name == "file" {
            fields.insert("file_name".into(), field.file_name().unwrap().to_owned());
            fields.insert(
                "file".into(),
                String::from_utf8_lossy(&field.bytes().await.unwrap()).into(),
            );
        } else {
            fields.insert(name, field.text().await.unwrap());
        }
    }
    d.calls.lock().unwrap().push(("save".into(), fields));
    if d.fail.load(Ordering::SeqCst) {
        return (StatusCode::BAD_GATEWAY, Json(json!({"detail":"FSM down"}))).into_response();
    }
    (
        StatusCode::CREATED,
        Json(json!({"data":{"memory_id":"m-a","resource_id":"r-a"}})),
    )
        .into_response()
}
/// Only alice owns file r-a, linked to her file memory m-a.
async fn node(
    State(d): State<Double>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    d.calls
        .lock()
        .unwrap()
        .push((format!("node {id}"), q.clone()));
    if id != "resource:r-a" || q["user_id"] != "alice" {
        return (StatusCode::NOT_FOUND, Json(json!({"detail":"not found"}))).into_response();
    }
    Json(json!({"data":{"nodes":[
        {"id":"m-a","node_type":"memory","label":"a.txt","metadata":{"id":"m-a","workspace_id":"cortex","app_id":"cortex","user_id":"alice"}},
        {"id":"resource:r-a","node_type":"file","label":"a.txt","metadata":{}}],
        "edges":[{"source_id":"m-a","target_id":"resource:r-a","edge_type":"attachment"}],"truncated":false,
        "node":{"id":"resource:r-a"}}})).into_response()
}
async fn content(
    State(d): State<Double>,
    Path(id): Path<String>,
    Query(q): Query<HashMap<String, String>>,
) -> Response {
    d.calls
        .lock()
        .unwrap()
        .push((format!("content {id}"), q.clone()));
    if id != "r-a" || q["user_id"] != "alice" {
        return (StatusCode::NOT_FOUND, Json(json!({"detail":"not found"}))).into_response();
    }
    (
        [
            (header::CONTENT_TYPE, "text/plain"),
            (
                header::CONTENT_DISPOSITION,
                "attachment; filename*=UTF-8''a.txt",
            ),
        ],
        "hello world",
    )
        .into_response()
}
async fn server() -> (Double, Config) {
    let double = Double::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        hippocampus_url: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let app = Router::new()
        .route("/api/v1/files", post(save))
        .route("/api/v1/graph/nodes/{id}", get(node))
        .route("/api/v1/resources/{id}/content", get(content))
        .with_state(double.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (double, config)
}
/// Transcriber double: "spoken text" when the audio bytes start with "ok", failure otherwise.
struct Speech;
impl cortex_api::domain::services::transcription::AudioTranscriber for Speech {
    fn transcribe<'a>(
        &'a self,
        _: &'a cortex_api::domain::models::identity::Caller,
        _: &'a str,
        bytes: Arc<Vec<u8>>,
        _: &'a cortex_api::domain::services::events::EventSink,
    ) -> cortex_api::domain::models::usage::Pending<
        'a,
        Result<String, cortex_api::core::process_numbers::ProcessNumber>,
    > {
        Box::pin(async move {
            if bytes.starts_with(b"ok") {
                Ok("spoken text".into())
            } else {
                Err(cortex_api::core::process_numbers::AUDIO_FAILED)
            }
        })
    }
}
fn app(config: &Config, cache: RedisCache) -> Router {
    with_response_contract(router(FileProxy {
        hippocampus: Hippocampus::new(config).unwrap(),
        extractor: DocumentExtractor::new(config),
        transcriber: Arc::new(Speech),
        temporary: TemporaryChats::new(config, cache),
        config: config.clone(),
    }))
}
fn no_redis() -> RedisCache {
    RedisCache::new(None, Duration::from_millis(20))
}
/// Raw multipart body: `(name, file content type or None, value)`.
fn form(parts: &[(&str, Option<&str>, &str)]) -> Body {
    let mut body = String::new();
    for (name, kind, value) in parts {
        body += "--XB\r\n";
        match kind {
            Some(kind) => {
                body += &format!(
                    "Content-Disposition: form-data; name=\"{name}\"; filename=\"a.txt\"\r\nContent-Type: {kind}\r\n\r\n"
                )
            }
            None => body += &format!("Content-Disposition: form-data; name=\"{name}\"\r\n\r\n"),
        }
        body += value;
        body += "\r\n";
    }
    body += "--XB--\r\n";
    Body::from(body)
}
async fn send(
    app: Router,
    req: axum::http::request::Builder,
    user: Option<&str>,
    body: Body,
) -> (StatusCode, Vec<u8>) {
    let mut req = req.extension(axum::extract::ConnectInfo(
        "203.0.113.1:1234".parse::<std::net::SocketAddr>().unwrap(),
    ));
    if let Some(user) = user {
        req = req.header("x-uuid", user);
    }
    let response = app.oneshot(req.body(body).unwrap()).await.unwrap();
    (
        response.status(),
        to_bytes(response.into_body(), 1 << 20)
            .await
            .unwrap()
            .to_vec(),
    )
}
async fn upload(
    app: Router,
    user: Option<&str>,
    parts: &[(&str, Option<&str>, &str)],
) -> (StatusCode, Value) {
    let (status, body) = send(
        app,
        Request::post("/files").header("content-type", "multipart/form-data; boundary=XB"),
        user,
        form(parts),
    )
    .await;
    (status, serde_json::from_slice(&body).unwrap())
}
#[tokio::test]
async fn upload_sends_file_scope_and_text_in_one_call() {
    let (double, config) = server().await;
    let (status, body) = upload(
        app(&config, no_redis()),
        Some("alice"),
        &[
            ("app_id", None, "cortex"),
            ("file", Some("text/plain"), "hello world"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["file_id"], "r-a");
    let calls = double.calls.lock().unwrap();
    assert_eq!(calls.len(), 1);
    let (kind, f) = &calls[0];
    assert_eq!(kind, "save");
    assert_eq!(f["workspace_id"], "cortex");
    assert_eq!(f["app_id"], "cortex");
    assert_eq!(f["user_id"], "alice");
    assert_eq!(f["extracted_text"], "hello world");
    assert_eq!(f["file"], "hello world");
    assert_eq!(f["file_name"], "a.txt");
}
#[tokio::test]
async fn extraction_failure_still_saves_with_empty_text() {
    let (double, config) = server().await;
    let (status, body) = upload(
        app(&config, no_redis()),
        Some("alice"),
        &[
            ("app_id", None, "cortex"),
            ("file", Some("application/octet-stream"), "raw"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["data"]["file_id"], "r-a");
    let calls = double.calls.lock().unwrap();
    assert_eq!(calls[0].1["extracted_text"], "");
    assert_eq!(calls[0].1["file"], "raw");
}
#[tokio::test]
async fn save_failure_is_a_file_not_saved_notice() {
    let (double, config) = server().await;
    double.fail.store(true, Ordering::SeqCst);
    let (status, body) = upload(
        app(&config, no_redis()),
        Some("alice"),
        &[
            ("app_id", None, "cortex"),
            ("file", Some("text/plain"), "hello"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["data"]["notice_number"], 1040);
    assert!(body["data"].get("file_id").is_none());
}
#[tokio::test]
async fn bad_forms_are_refused_before_hippocampus() {
    let (double, config) = server().await;
    let file = ("file", Some("text/plain"), "hello");
    for (user, parts, number) in [
        (Some("alice"), vec![file], 1003),
        (Some("alice"), vec![("app_id", None, "cortex")], 1003),
        (Some("alice"), vec![("app_id", None, "unknown"), file], 1009),
        (None, vec![("app_id", None, "cortex"), file], 1005),
        (
            Some("alice"),
            vec![("app_id", None, "cortex"), ("temporary", None, "yes"), file],
            1003,
        ),
    ] {
        let (status, body) = upload(app(&config, no_redis()), user, &parts).await;
        assert_eq!(status, StatusCode::BAD_REQUEST, "{number}");
        assert_eq!(body["data"]["error_number"], number);
    }
    let small = Config {
        file_bytes: 4,
        ..config.clone()
    };
    let (status, body) = upload(
        app(&small, no_redis()),
        Some("alice"),
        &[("app_id", None, "cortex"), file],
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["data"]["error_number"], 1031);
    assert!(double.calls.lock().unwrap().is_empty());
}
#[tokio::test]
async fn download_checks_scope_first_and_other_user_gets_an_error() {
    let (double, config) = server().await;
    let (status, body) = send(
        app(&config, no_redis()),
        Request::get("/files/r-a/content?app_id=cortex"),
        Some("alice"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body, b"hello world");
    {
        let calls = double.calls.lock().unwrap();
        assert_eq!(
            calls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(),
            ["node resource:r-a", "content r-a"]
        );
        assert_eq!(calls[1].1["workspace_id"], "cortex");
        assert_eq!(calls[1].1["user_id"], "alice");
    }
    let (status, body) = send(
        app(&config, no_redis()),
        Request::get("/files/r-a/content?app_id=cortex"),
        Some("bob"),
        Body::empty(),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(
        serde_json::from_slice::<Value>(&body).unwrap()["data"]["error_number"],
        1041
    );
    // The content route is never called for a file outside the scope.
    assert_eq!(double.calls.lock().unwrap().len(), 3);
}
#[tokio::test]
async fn temporary_upload_with_redis_down_is_refused_and_not_sent_to_hippocampus() {
    let (double, config) = server().await;
    let (status, body) = upload(
        app(&config, no_redis()),
        Some("alice"),
        &[
            ("app_id", None, "cortex"),
            ("temporary", None, "true"),
            ("file", Some("text/plain"), "hello"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["data"]["error_number"], 1035);
    assert!(double.calls.lock().unwrap().is_empty());
}
#[tokio::test]
#[ignore = "requires approved Valkey; only one unique cortex:temp UUID key is written"]
async fn temporary_upload_stays_in_redis() {
    let (double, config) = server().await;
    let url = std::env::var("CORTEX_REDIS_URL").unwrap();
    let user = format!("test-{}", uuid::Uuid::new_v4());
    let (status, body) = upload(
        app(&config, RedisCache::new(Some(&url), Duration::from_secs(2))),
        Some(&user),
        &[
            ("app_id", None, "cortex"),
            ("temporary", None, "true"),
            ("file", Some("text/plain"), "temporary text"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    let id = body["data"]["file_id"].as_str().unwrap().to_owned();
    assert!(uuid::Uuid::parse_str(&id).is_ok());
    assert!(double.calls.lock().unwrap().is_empty());
    let mut redis = redis::Client::open(url)
        .unwrap()
        .get_multiplexed_async_connection()
        .await
        .unwrap();
    let key = cortex_api::domain::models::cache::CacheKey::TemporaryChat(id.clone()).name();
    let raw: String = redis::cmd("GET")
        .arg(&key)
        .query_async(&mut redis)
        .await
        .unwrap();
    let raw: Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(raw["user_id"], user.as_str());
    assert_eq!(raw["file"]["pages"][0]["text"], "temporary text");
    let ttl: i64 = redis::cmd("PTTL")
        .arg(&key)
        .query_async(&mut redis)
        .await
        .unwrap();
    assert!((3_590_000..=3_600_000).contains(&ttl));
    let _: () = redis::cmd("DEL")
        .arg(&key)
        .query_async(&mut redis)
        .await
        .unwrap();
}

#[tokio::test]
async fn audio_is_transcribed_and_a_failure_still_saves_the_file() {
    let (double, config) = server().await;
    for (bytes, expected) in [("ok audio", "spoken text"), ("broken", "")] {
        let (status, body) = upload(
            app(&config, no_redis()),
            Some("alice"),
            &[
                ("app_id", None, "cortex"),
                ("file", Some("audio/ogg"), bytes),
            ],
        )
        .await;
        assert_eq!(status, StatusCode::CREATED, "{body}");
        assert_eq!(
            double.calls.lock().unwrap().last().unwrap().1["extracted_text"],
            expected
        );
    }
}
