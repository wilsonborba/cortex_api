use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::post,
};
use cortex_api::{
    core::config::Config,
    dal::{
        local::{
            chats::Chats,
            database::{self, Target},
        },
        remote::hippocampus::Hippocampus,
    },
    domain::{
        models::{
            chat_session::{NewChat, PreparedInput},
            history::{FinishedOutput, FinishedStatus},
            identity::Caller,
            storage::AppUser,
        },
        services::{chat_session::new_permanent, exchange_memory::ExchangeMemories},
    },
};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicU8, Ordering},
    },
    time::Duration,
};
use uuid::Uuid;
#[derive(Clone, Default)]
struct Double {
    calls: Arc<Mutex<Vec<Value>>>,
    mode: Arc<AtomicU8>,
}
async fn create(State(state): State<Double>, Json(body): Json<Value>) -> Response {
    let count = {
        let mut calls = state.calls.lock().unwrap();
        calls.push(body);
        calls.len()
    };
    match state.mode.load(Ordering::SeqCst) {
        1 => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error":"test failure"})),
        )
            .into_response(),
        2 => Json(json!({"data":{}})).into_response(),
        3 => (
            StatusCode::TEMPORARY_REDIRECT,
            [("location", "http://127.0.0.1:1/api/v1/memories")],
        )
            .into_response(),
        _ => {
            tokio::time::sleep(Duration::from_millis(10)).await;
            (
                StatusCode::CREATED,
                Json(json!({"data":{"id":format!("memory-{count}")}})),
            )
                .into_response()
        }
    }
}
async fn server() -> (Double, Config, tokio::task::JoinHandle<()>) {
    let state = Double::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        hippocampus_url: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let app = Router::new()
        .route("/api/v1/memories", post(create))
        .with_state(state.clone());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    (state, config, task)
}
#[tokio::test]
async fn json_scope_content_and_failed_responses() {
    let (state, config, task) = server().await;
    assert_eq!(config.hippocampus_timeout, Duration::from_secs(5));
    let client = Hippocampus::new(&config).unwrap();
    let scope = AppUser {
        app_id: "cortex".into(),
        user_id: "user-a".into(),
    };
    assert_eq!(
        client
            .create_exchange(&scope, "Title", "question", "answer")
            .await
            .unwrap(),
        "memory-1"
    );
    let body = state.calls.lock().unwrap()[0].clone();
    assert_eq!(
        body,
        json!({"workspace_id":"cortex","app_id":"cortex","user_id":"user-a","title":"Title","content":"Input:\nquestion\n\nAnswer:\nanswer"})
    );
    let a2a = AppUser {
        app_id: "certifications".into(),
        user_id: String::new(),
    };
    client
        .create_exchange(&a2a, "Title", "question", "answer")
        .await
        .unwrap();
    assert!(state.calls.lock().unwrap()[1]["user_id"].is_null());
    for mode in [1, 2, 3] {
        state.mode.store(mode, Ordering::SeqCst);
        assert!(
            client
                .create_exchange(&scope, "Title", "question", "answer")
                .await
                .is_err()
        );
    }
    assert_eq!(state.calls.lock().unwrap().len(), 5);
    task.abort();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        hippocampus_url: format!("http://{}", listener.local_addr().unwrap()),
        hippocampus_timeout: Duration::from_millis(50),
        ..Default::default()
    };
    let stalled = tokio::spawn(async move {
        let (_socket, _) = listener.accept().await.unwrap();
        std::future::pending::<()>().await;
    });
    let client = Hippocampus::new(&config).unwrap();
    assert!(
        tokio::time::timeout(
            Duration::from_secs(1),
            client.create_exchange(&scope, "Title", "q", "a")
        )
        .await
        .unwrap()
        .is_err()
    );
    stalled.abort();
}
async fn output(
    repo: &Chats,
    config: &Config,
    actor: &Caller,
    text: &str,
    status: FinishedStatus,
) -> PreparedInput {
    let body: NewChat = serde_json::from_value(json!({"app_id":actor.app_id,"text":text})).unwrap();
    let now = chrono::Utc::now();
    let prepared =
        new_permanent(config, actor, body, &[], now).unwrap_or_else(|_| panic!("prepare failed"));
    repo.save_accepted(&prepared).await.unwrap();
    repo.finish_output(
        &actor.storage_scope(),
        &FinishedOutput {
            session_id: prepared.input.session_id,
            output_id: prepared.output.output_id,
            status,
            text: "saved answer".into(),
            thinking_text: None,
            memory_id: None,
            sources: vec![],
            memories: vec![],
            finished_at: now,
        },
    )
    .await
    .unwrap();
    prepared
}
#[tokio::test]
#[ignore = "requires cortex_api_test; local Hippocampus double and a unique temporary schema"]
async fn one_memory_per_done_exchange_mapping_temporary_and_failure() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_exchange_memory_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(4)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = Chats { pool: pool.clone() };
    let (state, config, task) = server().await;
    let service = ExchangeMemories {
        repo: repo.clone(),
        hippocampus: Hippocampus::new(&config).unwrap(),
    };
    let actor = Caller {
        app_id: "cortex".into(),
        user_id: Some("user-a".into()),
        is_admin: false,
    };
    let scope = actor.storage_scope();
    let first = output(&repo, &config, &actor, "question", FinishedStatus::Done).await;
    let (left, right) = tokio::join!(
        service.remember(
            &scope,
            first.input.session_id,
            first.output.output_id,
            false
        ),
        service.remember(
            &scope,
            first.input.session_id,
            first.output.output_id,
            false
        )
    );
    assert_eq!(left.memory_id.as_deref(), Some("memory-1"));
    assert_eq!(right.memory_id, left.memory_id);
    assert!(left.notice.is_none() && right.notice.is_none());
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    assert_eq!(
        repo.output(&scope, first.output.output_id)
            .await
            .unwrap()
            .unwrap()
            .memory_id
            .as_deref(),
        Some("memory-1")
    );
    let repeat = service
        .remember(
            &scope,
            first.input.session_id,
            first.output.output_id,
            false,
        )
        .await;
    assert_eq!(repeat.memory_id.as_deref(), Some("memory-1"));
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    // Temporary outputs do not even need a SQL row. There is no remote write.
    let temporary = service
        .remember(&scope, Uuid::now_v7(), Uuid::now_v7(), true)
        .await;
    assert!(temporary.memory_id.is_none() && temporary.notice.is_none());
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    for status in [FinishedStatus::Failed, FinishedStatus::Cancelled] {
        let prepared = output(&repo, &config, &actor, "partial", status).await;
        let result = service
            .remember(
                &scope,
                prepared.input.session_id,
                prepared.output.output_id,
                false,
            )
            .await;
        assert!(result.memory_id.is_none() && result.notice.is_none());
    }
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    let mut other = scope.clone();
    other.user_id = "user-b".into();
    let forbidden = service
        .remember(
            &other,
            first.input.session_id,
            first.output.output_id,
            false,
        )
        .await;
    assert_eq!(forbidden.notice.unwrap().number, 1019);
    assert_eq!(state.calls.lock().unwrap().len(), 1);
    let failure = output(
        &repo,
        &config,
        &actor,
        "remote failure",
        FinishedStatus::Done,
    )
    .await;
    state.mode.store(1, Ordering::SeqCst);
    let result = service
        .remember(
            &scope,
            failure.input.session_id,
            failure.output.output_id,
            false,
        )
        .await;
    assert_eq!(result.notice.unwrap().number, 1019);
    let saved = repo
        .output(&scope, failure.output.output_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.status, "done");
    assert_eq!(saved.text, "saved answer");
    assert!(saved.memory_id.is_none());
    assert_eq!(state.calls.lock().unwrap().len(), 2);
    task.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
