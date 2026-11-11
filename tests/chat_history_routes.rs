use axum::{
    Router,
    body::{Body, to_bytes},
    http::{Method, Request, StatusCode},
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
        },
        services::chat_session::{continue_permanent, new_permanent},
    },
    presentation::{self, chats},
};
use serde_json::{Value, json};
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
        let input =
            continue_permanent(&a, &chat, body, &[], now + chrono::Duration::seconds(index))
                .unwrap_or_else(|_| panic!("prepare failed"));
        repo.save_accepted(&input).await.unwrap();
        inputs.push(input);
    }
    for input in &inputs {
        repo.finish_output(
            &a.storage_scope(),
            &FinishedOutput {
                session_id: session,
                output_id: input.output.output_id,
                status: FinishedStatus::Done,
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
    let app = presentation::with_response_contract(chats::router(repo.clone(), config));
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
    remote.abort();
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
