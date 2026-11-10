use axum::{
    Json, Router,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cortex_api::{
    core::config::Config,
    dal::remote::runs::{RunStore, RunStoreError},
    domain::{
        models::{pipeline::*, run_evidence::RunIdentity, run_record::*},
        services::run_evidence::RunEvidence,
        tasks::run_retention,
    },
};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex,
        atomic::{AtomicUsize, Ordering},
    },
};
use uuid::Uuid;
#[derive(Clone, Default)]
struct Double {
    docs: Arc<Mutex<BTreeMap<String, Value>>>,
    puts: Arc<AtomicUsize>,
    finds: Arc<AtomicUsize>,
    conflict: Arc<Mutex<Option<String>>>,
}
async fn read(State(s): State<Double>, Path(id): Path<String>) -> Response {
    match s.docs.lock().unwrap().get(&id) {
        Some(value) => Json(value.clone()).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    }
}
async fn put(
    State(s): State<Double>,
    Path(id): Path<String>,
    Json(mut v): Json<Value>,
) -> Response {
    s.puts.fetch_add(1, Ordering::SeqCst);
    v["_id"] = id.clone().into();
    v["_rev"] = "1-test".into();
    s.docs.lock().unwrap().insert(id, v);
    Json(json!({"ok":true})).into_response()
}
async fn find(State(s): State<Double>, Json(body): Json<Value>) -> Json<Value> {
    s.finds.fetch_add(1, Ordering::SeqCst);
    let before = body["selector"]["created_at"]["$lt"].as_str().unwrap();
    let bookmark = body["bookmark"].as_str().unwrap_or("");
    let limit = body["limit"].as_u64().unwrap() as usize;
    let docs: Vec<_> = s
        .docs
        .lock()
        .unwrap()
        .iter()
        .filter(|(id, v)| {
            id.as_str() > bookmark && v["created_at"].as_str().is_some_and(|t| t < before)
        })
        .take(limit)
        .map(|(_, v)| v.clone())
        .collect();
    let next = docs
        .last()
        .map(|d| d["_id"].as_str().unwrap())
        .unwrap_or(bookmark);
    Json(json!({"docs":docs,"bookmark":next}))
}
async fn remove(
    State(s): State<Double>,
    Path(id): Path<String>,
    Query(q): Query<BTreeMap<String, String>>,
) -> Response {
    let mut docs = s.docs.lock().unwrap();
    if s.conflict.lock().unwrap().as_ref() == Some(&id) {
        let doc = docs.get_mut(&id).unwrap();
        doc["_rev"] = "2-new".into();
        doc["created_at"] = chrono::Utc::now().to_rfc3339().into();
        return StatusCode::CONFLICT.into_response();
    }
    match docs.get(&id) {
        None => StatusCode::NOT_FOUND.into_response(),
        Some(v) if v["_rev"].as_str() != q.get("rev").map(String::as_str) => {
            StatusCode::CONFLICT.into_response()
        }
        Some(_) => {
            docs.remove(&id);
            Json(json!({"ok":true})).into_response()
        }
    }
}
async fn server() -> (Double, Arc<RunStore>, tokio::task::JoinHandle<()>) {
    let state = Double::default();
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/runs", socket.local_addr().unwrap());
    let app = Router::new()
        .route("/runs/_find", post(find))
        .route("/runs/{id}", get(read).put(put).delete(remove))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(socket, app).await.unwrap();
    });
    let store = Arc::new(
        RunStore::new(
            &Config::default(),
            url.parse().unwrap(),
            String::new(),
            String::new(),
        )
        .unwrap(),
    );
    (state, store, task)
}
fn counts() -> RunCounts {
    RunCounts {
        pages_read: 0,
        chunks_used: 0,
        files: 0,
        file_pages: 0,
        audio_seconds: 0.0,
    }
}
fn record(created: chrono::DateTime<chrono::Utc>) -> RunRecord {
    RunRecord {
        session_id: Uuid::now_v7(),
        output_id: Uuid::new_v4(),
        app_id: "cortex".into(),
        user_id: "fixture".into(),
        created_at: created,
        mode: RunMode {
            web_mode: WebMode::Fast,
            thinking: false,
        },
        total_ms: 5,
        stages: vec![],
        calls: vec![],
        counts: counts(),
    }
}
fn stage(name: StageName, calls: Vec<ModelCall>) -> StageResult<String> {
    StageResult {
        stage: name,
        status: StageStatus::Ok,
        time_ms: 1,
        output: StageOutput {
            data: "PRIVATE_USER_TEXT PRIVATE_MODEL_OUTPUT".into(),
            texts: vec![LabelledText {
                label: "fixture".into(),
                text: "PRIVATE_FILE_TEXT".into(),
                source: Some("PRIVATE_URL".into()),
            }],
            calls,
            notices: vec![],
        },
    }
}
#[tokio::test]
async fn doubled_output_writes_one_typed_record_without_private_stage_data() {
    let (s, store, task) = server().await;
    let now = chrono::Utc::now();
    let call = ModelCall {
        images: None,
        audio: None,
        role: "answer".into(),
        model: "fixture-model".into(),
        input_tokens: 20,
        output_tokens: 10,
        time_to_first_token_ms: Some(1),
        total_ms: 3,
        started_at: now,
        finished_at: now,
    };
    let result = PipelineResult {
        stages: vec![
            stage(StageName::Accept, vec![]),
            stage(StageName::Answer, vec![call.clone()]),
            stage(StageName::Save, vec![call]),
        ],
    };
    let identity = record(now);
    let id = identity.output_id;
    RunEvidence::new(store.clone())
        .save(
            RunIdentity {
                session_id: identity.session_id,
                output_id: id,
                app_id: identity.app_id,
                user_id: identity.user_id,
                created_at: now,
                mode: identity.mode,
            },
            &result,
            counts(),
            8,
        )
        .await
        .unwrap();
    assert_eq!(s.puts.load(Ordering::SeqCst), 1);
    let saved = store.get(id).await.unwrap().unwrap();
    assert_eq!(saved.calls.len(), 1);
    assert_eq!(saved.stages.len(), 3);
    assert_eq!(saved.total_ms, 8);
    let raw = serde_json::to_string(&saved).unwrap();
    assert!(!raw.contains("PRIVATE"));
    assert!(!raw.contains("schema_version"));
    task.abort();
}
#[tokio::test]
async fn retention_is_strict_paged_and_revision_safe() {
    let (s, store, task) = server().await;
    let config = Config::default();
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-10T12:00:00.500000Z")
        .unwrap()
        .with_timezone(&chrono::Utc);
    let cutoff = now - chrono::Duration::days(30);
    for _ in 0..225 {
        store
            .put(&record(cutoff - chrono::Duration::days(1)))
            .await
            .unwrap();
    }
    let edge_old = record(cutoff - chrono::Duration::microseconds(1));
    store.put(&edge_old).await.unwrap();
    let conflict = record(cutoff - chrono::Duration::days(1));
    store.put(&conflict).await.unwrap();
    *s.conflict.lock().unwrap() = Some(conflict.output_id.to_string());
    let equal = record(cutoff);
    let newer = record(cutoff + chrono::Duration::microseconds(1));
    let current = record(now);
    let future = record(now + chrono::Duration::days(1));
    for r in [&equal, &newer, &current, &future] {
        store.put(r).await.unwrap();
    }
    assert_eq!(
        run_retention::purge(&store, &config, now).await.unwrap(),
        226
    );
    assert_eq!(s.finds.load(Ordering::SeqCst), 2);
    assert!(store.get(edge_old.output_id).await.unwrap().is_none());
    for id in [
        equal.output_id,
        newer.output_id,
        current.output_id,
        future.output_id,
        conflict.output_id,
    ] {
        assert!(store.get(id).await.unwrap().is_some());
    }
    assert_eq!(s.docs.lock().unwrap().len(), 5);
    assert_eq!(run_retention::purge(&store, &config, now).await.unwrap(), 0);
    task.abort();
}
#[tokio::test]
async fn invalid_metadata_is_not_deleted() {
    let (s, store, task) = server().await;
    let now = chrono::Utc::now();
    let old = record(now - chrono::Duration::days(40));
    store.put(&old).await.unwrap();
    s.docs
        .lock()
        .unwrap()
        .get_mut(&old.output_id.to_string())
        .unwrap()["prompt"] = "invalid text field".into();
    assert_eq!(
        run_retention::purge(&store, &Config::default(), now).await,
        Err(RunStoreError::InvalidRecord)
    );
    assert_eq!(s.docs.lock().unwrap().len(), 1);
    task.abort();
}
#[tokio::test]
#[ignore = "requires Cortex CouchDB credentials; creates and removes a unique Cortex retention test database"]
async fn real_couchdb_retention_keeps_boundary_and_newer_records() {
    dotenvy::dotenv().ok();
    let config = Config::default();
    assert_eq!(
        config.run_purge_interval,
        std::time::Duration::from_secs(86400)
    );
    let mut url = reqwest::Url::parse(&std::env::var("CORTEX_COUCHDB_URL").unwrap())
        .unwrap_or_else(|_| panic!("Cortex CouchDB test address is not valid"));
    let name = format!("cortex_runs_test_retention_{}", Uuid::new_v4().simple());
    url.path_segments_mut().unwrap().pop_if_empty().push(&name);
    let user = std::env::var("CORTEX_COUCHDB_USERNAME").unwrap();
    let password = std::env::var("CORTEX_COUCHDB_PASSWORD").unwrap();
    let store = RunStore::new(&config, url.clone(), user.clone(), password.clone()).unwrap();
    store.initialize().await.unwrap();
    let now = chrono::Utc::now();
    let cutoff = now - chrono::Duration::days(30);
    let old = record(cutoff - chrono::Duration::microseconds(1));
    let equal = record(cutoff);
    let newer = record(cutoff + chrono::Duration::microseconds(1));
    let current = record(now);
    for r in [&old, &equal, &newer, &current] {
        store.put(r).await.unwrap();
    }
    assert_eq!(run_retention::purge(&store, &config, now).await.unwrap(), 1);
    assert!(store.get(old.output_id).await.unwrap().is_none());
    for r in [&equal, &newer, &current] {
        assert!(store.get(r.output_id).await.unwrap().is_some());
    }
    let response = reqwest::Client::new()
        .delete(url)
        .basic_auth(user, Some(password))
        .send()
        .await
        .unwrap_or_else(|_| panic!("Cortex retention fixture cleanup failed"));
    assert!(response.status().is_success());
}
