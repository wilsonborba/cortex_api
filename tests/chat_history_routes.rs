use axum::{
    Json, Router,
    body::{Body, to_bytes},
    extract::State,
    http::{Method, Request, StatusCode},
    routing::post,
};
use chrono::{TimeZone, Utc};
use cortex_api::{
    core::config::Config,
    dal::local::{
        chats::Chats,
        database::{self, Target},
    },
    domain::{
        models::{
            chat_session::*,
            events::{Memory, Source},
            history::{FinishedOutput, FinishedStatus},
            identity::Caller,
            run_record::{RunCounts, RunMode, RunRecord, StageRecord, WebMode},
        },
        services::chat_session::{continue_permanent, new_permanent},
    },
    presentation::{self, chats},
};
use cortex_api::{dal::remote::runs::RunStore, domain::services::run_evidence::RunEvidence};
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};
use tower::ServiceExt;
use uuid::Uuid;
async fn call(
    app: &Router,
    method: Method,
    path: &str,
    user: Option<&str>,
    admin: bool,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(user) = user {
        req = req.header("x-uuid", user);
    }
    if admin {
        req = req.header("x-access-level", "admin");
    }
    let body = if let Some(body) = body {
        req = req.header("content-type", "application/json");
        Body::from(serde_json::to_vec(&body).unwrap())
    } else {
        Body::empty()
    };
    let response = app.clone().oneshot(req.body(body).unwrap()).await.unwrap();
    let status = response.status();
    let value =
        serde_json::from_slice(&to_bytes(response.into_body(), 1000000).await.unwrap()).unwrap();
    (status, value)
}
fn actor(user: &str) -> Caller {
    Caller {
        app_id: "cortex".into(),
        user_id: Some(user.into()),
        is_admin: false,
    }
}
fn body(text: &str) -> NewChat {
    serde_json::from_value(json!({"app_id":"cortex","text":text})).unwrap()
}
#[derive(Clone, Default)]
struct RunDocs(Arc<Mutex<BTreeMap<String, Value>>>);
async fn all_run_docs(State(state): State<RunDocs>, Json(body): Json<Value>) -> Json<Value> {
    let docs = state.0.lock().unwrap();
    let rows: Vec<_> = body["keys"]
        .as_array()
        .unwrap()
        .iter()
        .map(|key| {
            let id = key.as_str().unwrap();
            match docs.get(id) {
                Some(doc) => json!({"id":id,"key":id,"value":{"rev":doc["_rev"]},"doc":doc}),
                None => json!({"key":id,"error":"not_found"}),
            }
        })
        .collect();
    Json(json!({"total_rows":rows.len(),"offset":0,"rows":rows}))
}
fn run_record(
    output_id: Uuid,
    session_id: Uuid,
    user_id: &str,
    stages: Vec<StageRecord>,
) -> RunRecord {
    RunRecord {
        output_id,
        session_id,
        app_id: "cortex".into(),
        user_id: user_id.into(),
        created_at: Utc::now(),
        mode: RunMode {
            web_mode: WebMode::Fast,
            thinking: false,
        },
        total_ms: 25,
        stages,
        calls: vec![],
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
#[ignore = "requires cortex_api_test; real HTTP handlers and a unique temporary schema"]
async fn routes_order_page_rename_pin_and_refuse_other_users_even_admins() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_routes_{}", Uuid::new_v4().simple());
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
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let config = Config {
        hippocampus_url: format!("http://{}", listener.local_addr().unwrap()),
        ..Default::default()
    };
    let remote = tokio::spawn(async move { axum::serve(listener, Router::new()).await.unwrap() });
    let a = actor("user-a");
    let b = actor("user-b");
    let now = Utc.with_ymd_and_hms(2030, 1, 1, 0, 0, 0).unwrap();
    let first = new_permanent(&config, &a, body("old first"), &[], now)
        .unwrap_or_else(|_| panic!("prepare failed"));
    let newer = new_permanent(
        &config,
        &a,
        body("new first"),
        &[],
        now + chrono::Duration::seconds(60),
    )
    .unwrap_or_else(|_| panic!("prepare failed"));
    let foreign = new_permanent(&config, &b, body("foreign first"), &[], now)
        .unwrap_or_else(|_| panic!("prepare failed"));
    for input in [&first, &newer, &foreign] {
        repo.save_accepted(input).await.unwrap();
    }
    let chat = first.new_chat.clone().unwrap();
    let session = chat.session_id;
    let mut inputs = vec![first.clone()];
    for index in 1..3 {
        let body: NextInput =
            serde_json::from_value(json!({"text":format!("question{index}")})).unwrap();
        let input = continue_permanent(
            &config,
            &a,
            &chat,
            body,
            &[],
            now + chrono::Duration::seconds(index),
        )
        .unwrap_or_else(|_| panic!("prepare failed"));
        repo.save_accepted(&input).await.unwrap();
        inputs.push(input);
    }
    for (index, input) in inputs.iter().enumerate() {
        repo.finish_output(
            &a.storage_scope(),
            &FinishedOutput {
                session_id: session,
                output_id: input.output.output_id,
                status: if index == 1 {
                    FinishedStatus::Failed
                } else {
                    FinishedStatus::Done
                },
                text: format!("answer for {}", input.input.text),
                thinking_text: Some("thinking text".into()),
                memory_id: Some("exchange memory".into()),
                sources: vec![Source {
                    title: "Page".into(),
                    url: "https://example.org".into(),
                }],
                memories: vec![Memory {
                    id: "memory-id".into(),
                    title: "Memory".into(),
                }],
                finished_at: now + chrono::Duration::seconds(5),
            },
        )
        .await
        .unwrap();
    }
    let run_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let run_url = format!("http://{}/runs", run_listener.local_addr().unwrap());
    let run_docs = RunDocs::default();
    for (index, input) in inputs.iter().enumerate() {
        let stages = if index == 0 {
            vec![
                StageRecord {
                    name: "gather".into(),
                    status: "notice".into(),
                    error_number: None,
                    notice_number: Some(1042),
                    time_ms: 3,
                },
                StageRecord {
                    name: "save".into(),
                    status: "ok".into(),
                    error_number: None,
                    notice_number: None,
                    time_ms: 2,
                },
            ]
        } else if index == 1 {
            vec![
                StageRecord {
                    name: "gather".into(),
                    status: "notice".into(),
                    error_number: None,
                    notice_number: Some(1042),
                    time_ms: 3,
                },
                StageRecord {
                    name: "answer".into(),
                    status: "failed".into(),
                    error_number: Some(1043),
                    notice_number: None,
                    time_ms: 4,
                },
            ]
        } else {
            vec![StageRecord {
                name: "answer".into(),
                status: "failed".into(),
                error_number: Some(1042),
                notice_number: None,
                time_ms: 3,
            }]
        };
        let user = if index == 2 { "forged-user" } else { "user-a" };
        let record = run_record(input.output.output_id, session, user, stages);
        let id = record.output_id.to_string();
        let mut value = serde_json::to_value(record).unwrap();
        value["_id"] = id.clone().into();
        value["_rev"] = "1-test".into();
        run_docs.0.lock().unwrap().insert(id, value);
    }
    let run_remote = tokio::spawn(async move {
        axum::serve(
            run_listener,
            Router::new()
                .route("/runs/_all_docs", post(all_run_docs))
                .with_state(run_docs),
        )
        .await
        .unwrap();
    });
    let run_store = RunStore::new(
        &config,
        run_url.parse().unwrap(),
        "test".into(),
        "test".into(),
    )
    .unwrap();
    let app = presentation::with_response_contract(chats::router_with_runs(
        repo.clone(),
        config.clone(),
        Some(RunEvidence::new(Arc::new(run_store))),
    ));
    let path = format!("/chats/{session}");
    let (status, list) = call(&app, Method::GET, "/chats", Some("user-a"), false, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(list["data"].as_array().unwrap().len(), 2);
    assert_eq!(
        list["data"][0]["session_id"],
        newer.input.session_id.to_string()
    );
    let (status, pinned) = call(
        &app,
        Method::PATCH,
        &path,
        Some("user-a"),
        false,
        Some(json!({"is_pinned":true})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(pinned["data"]["is_pinned"], true);
    let (_, list) = call(&app, Method::GET, "/chats", Some("user-a"), false, None).await;
    assert_eq!(list["data"][0]["session_id"], session.to_string());
    let (status, renamed) = call(
        &app,
        Method::PATCH,
        &path,
        Some("user-a"),
        false,
        Some(json!({"title":"Renamed"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(renamed["data"]["title"], "Renamed");
    assert_eq!(
        renamed["data"]["updated_at"],
        (now + chrono::Duration::seconds(2)).to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
    );
    let (status, page) = call(
        &app,
        Method::GET,
        &format!("{path}?limit=2"),
        Some("user-a"),
        false,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["data"]["inputs"].as_array().unwrap().len(), 2);
    assert_eq!(
        page["data"]["inputs"][0]["input_id"],
        inputs[2].input.input_id.to_string()
    );
    assert_eq!(
        page["data"]["inputs"][1]["input_id"],
        inputs[1].input.input_id.to_string()
    );
    assert_eq!(page["data"]["outputs"].as_array().unwrap().len(), 2);
    let (_, older) = call(
        &app,
        Method::GET,
        &format!("{path}?before={}&limit=2", inputs[1].input.input_id),
        Some("user-a"),
        false,
        None,
    )
    .await;
    assert_eq!(older["data"]["inputs"].as_array().unwrap().len(), 1);
    assert_eq!(
        older["data"]["inputs"][0]["input_id"],
        first.input.input_id.to_string()
    );
    let output_path = format!("{path}/outputs/{}", first.output.output_id);
    let (status, output) = call(&app, Method::GET, &output_path, Some("user-a"), false, None).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(output["data"]["thinking_text"], "thinking text");
    assert_eq!(output["data"]["sources"][0]["url"], "https://example.org");
    assert_eq!(output["data"]["memories"][0]["id"], "memory-id");
    assert_eq!(output["data"]["status"], "done");
    assert_eq!(output["data"]["run"]["total_ms"], 25);
    assert!(output["data"]["run"].get("tokens").is_none());
    assert!(output["data"]["run"].get("error_number").is_none());
    let failed_path = format!("{path}/outputs/{}", inputs[1].output.output_id);
    let (_, failed) = call(&app, Method::GET, &failed_path, Some("user-a"), false, None).await;
    assert_eq!(failed["data"]["run"]["error_number"], 1043);
    let forged_path = format!("{path}/outputs/{}", inputs[2].output.output_id);
    let (_, forged) = call(&app, Method::GET, &forged_path, Some("user-a"), false, None).await;
    assert!(forged["data"].get("run").is_none());
    for is_admin in [false, true] {
        for (method, target, body) in [
            (Method::GET, path.clone(), None),
            (Method::GET, output_path.clone(), None),
            (Method::PATCH, path.clone(), Some(json!({"title":"stolen"}))),
        ] {
            let (status, error) = call(&app, method, &target, Some("user-b"), is_admin, body).await;
            assert_eq!(status, StatusCode::NOT_FOUND);
            assert_eq!(error["data"]["error_number"], 1016);
        }
    }
    let (status, error) = call(&app, Method::GET, "/chats", None, false, None).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["data"]["error_number"], 1005);
    let (status, error) = call(
        &app,
        Method::GET,
        &format!("{path}?before={}", foreign.input.input_id),
        Some("user-a"),
        false,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["data"]["error_number"], 1016);
    let (status, error) = call(
        &app,
        Method::GET,
        &format!("{path}/outputs/{}", newer.output.output_id),
        Some("user-a"),
        false,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(error["data"]["error_number"], 1018);
    for patch in [
        json!({}),
        json!({"title":"bad","is_pinned":true}),
        json!({"app_id":"reports"}),
        json!({"is_pinned":"yes"}),
    ] {
        let (status, error) = call(
            &app,
            Method::PATCH,
            &path,
            Some("user-a"),
            false,
            Some(patch),
        )
        .await;
        assert!(status.is_client_error());
        assert!(error["data"]["error_number"].is_number());
    }
    let (status, error) = call(
        &app,
        Method::GET,
        &format!("{path}?limit=0"),
        Some("user-a"),
        false,
        None,
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(error["data"]["error_number"], 1003);
    let old = continue_permanent(
        &a,
        &chat,
        serde_json::from_value(json!({"text":"legacy run record"})).unwrap(),
        &[],
        now + chrono::Duration::seconds(120),
    )
    .unwrap();
    repo.save_accepted(&old).await.unwrap();
    repo.finish_output(
        &a.storage_scope(),
        &FinishedOutput {
            session_id: session,
            output_id: old.output.output_id,
            status: FinishedStatus::Done,
            text: "legacy answer".into(),
            thinking_text: None,
            memory_id: None,
            sources: vec![],
            memories: vec![],
            finished_at: now + chrono::Duration::seconds(121),
        },
    )
    .await
    .unwrap();
    let legacy_path = format!("{path}/outputs/{}", old.output.output_id);
    let (_, legacy) = call(&app, Method::GET, &legacy_path, Some("user-a"), false, None).await;
    assert_eq!(legacy["data"]["status"], "done");
    assert!(legacy["data"].get("run").is_none());
    run_remote.abort();
    let (_, outage) = call(&app, Method::GET, &path, Some("user-a"), false, None).await;
    assert_eq!(outage["data"]["chat"]["session_id"], session.to_string());
    assert!(outage["data"]["outputs"].as_array().unwrap().len() >= 3);
    assert!(
        outage["data"]["outputs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|item| item.get("run").is_none())
    );
    remote.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
