use axum::{
    Json, Router,
    body::Body,
    extract::State,
    http::{Request, StatusCode},
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
        models::{chat_session::*, identity::Caller},
        services::{
            chat_delete::ChatDelete, chat_history::ChatHistory, chat_session::new_permanent,
        },
    },
    presentation::{chat_delete::router, with_response_contract},
};
use serde_json::{Value, json};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use tower::ServiceExt;
use uuid::Uuid;
#[derive(Clone, Default)]
struct Double {
    calls: Arc<Mutex<Vec<Value>>>,
    fail: Arc<AtomicBool>,
}
#[tokio::test]
#[ignore = "requires cortex_api_test; uses a unique temporary schema"]
async fn hippocampus_first_then_sql_failure_keeps_chat_repeat_and_other_user() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_delete_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = sqlx::postgres::PgPoolOptions::new()
        .max_connections(3)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();
    let repo = Chats { pool: pool.clone() };
    let config = Config::default();
    let caller = Caller {
        app_id: "cortex".into(),
        user_id: Some("alice".into()),
        is_admin: false,
    };
    let body: NewChat =
        serde_json::from_value(json!({"app_id":"cortex","text":"question","file_ids":["r-a"]}))
            .unwrap();
    let files = [SessionFile {
        file_id: "r-a".into(),
        memory_id: "m-file".into(),
        name: "a.txt".into(),
    }];
    let prepared = new_permanent(&config, &caller, body, &files, chrono::Utc::now()).unwrap();
    repo.save_accepted(&prepared).await.unwrap();
    sqlx::query("UPDATE chat_outputs SET status='done', memory_id='m-exchange'")
        .execute(&pool)
        .await
        .unwrap();
    let session = prepared.input.session_id;

    let double = Double::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let remote = Router::new()
        .route(
            "/api/v1/memories/delete",
            post(
                |State(d): State<Double>, Json(body): Json<Value>| async move {
                    d.calls.lock().unwrap().push(body);
                    if d.fail.load(Ordering::SeqCst) {
                        (StatusCode::BAD_GATEWAY, Json(json!({})))
                    } else {
                        (
                            StatusCode::OK,
                            Json(json!({"data":["m-exchange","m-file"]})),
                        )
                    }
                },
            ),
        )
        .with_state(double.clone());
    tokio::spawn(async move { axum::serve(listener, remote).await.unwrap() });
    let app = with_response_contract(router(ChatDelete {
        history: ChatHistory {
            repo: repo.clone(),
            config: config.clone(),
            hippocampus: Hippocampus::new(&config).unwrap(),
        },
        hippocampus: Hippocampus::new(&Config {
            hippocampus_url: url,
            ..Default::default()
        })
        .unwrap(),
    }));
    let delete = |user: &'static str| {
        let app = app.clone();
        async move {
            let response = app
                .oneshot(
                    Request::delete(format!("/chats/{session}"))
                        .header("x-uuid", user)
                        .body(Body::empty())
                        .unwrap(),
                )
                .await
                .unwrap();
            let status = response.status();
            let body: Value = serde_json::from_slice(
                &axum::body::to_bytes(response.into_body(), 4096)
                    .await
                    .unwrap(),
            )
            .unwrap();
            (status, body)
        }
    };
    let rows = |table: &'static str| {
        let pool = pool.clone();
        async move {
            sqlx::query_scalar::<_, i64>(&format!("SELECT count(*) FROM {table}"))
                .fetch_one(&pool)
                .await
                .unwrap()
        }
    };
    // Other user: refused, no Hippocampus call.
    let (status, body) = delete("bob").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["data"]["error_number"], 1016);
    assert!(double.calls.lock().unwrap().is_empty());
    // Hippocampus failure keeps the chat.
    double.fail.store(true, Ordering::SeqCst);
    let (status, body) = delete("alice").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["data"]["error_number"], 1044);
    assert_eq!(rows("chats").await, 1);
    // Success: one call with every memory id and the scope, then a hard delete.
    double.fail.store(false, Ordering::SeqCst);
    let (status, body) = delete("alice").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["message"], "Chat deleted");
    let calls = double.calls.lock().unwrap().clone();
    assert_eq!(calls.len(), 2);
    assert_eq!(
        calls[1],
        json!({"workspace_id":"cortex","app_id":"cortex","user_id":"alice","memory_ids":["m-exchange","m-file"]})
    );
    for table in ["chats", "chat_inputs", "chat_outputs", "chat_input_files"] {
        assert_eq!(rows(table).await, 0, "{table}");
    }
    // Repeat: safe, nothing to delete, no new call.
    let (status, _) = delete("alice").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(double.calls.lock().unwrap().len(), 2);
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
