use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use serde_json::{Value, json};
use tower::ServiceExt;
use uuid::Uuid;

use cortex_api::dal::local::{
    agent_runs::PostgresAgentRunStore,
    database::{self, Target},
};
use cortex_api::domain::services::agent_runs::{
    AgentRunService, AgentRunStore, ChildEventKind, ChildRunStatus, InMemoryAgentRunStore,
    PendingChildAction,
};
use cortex_api::presentation::agent_runs::router;

async fn send(
    app: &axum::Router,
    method: &str,
    uri: &str,
    user: Option<&str>,
    body: Option<Value>,
) -> (StatusCode, Value) {
    let mut request = Request::builder().method(method).uri(uri);
    if let Some(user) = user {
        request = request.header("x-uuid", user);
    }
    let request = if let Some(body) = body {
        request
            .header("content-type", "application/json")
            .body(Body::from(body.to_string()))
            .unwrap()
    } else {
        request.body(Body::empty()).unwrap()
    };
    let response = app.clone().oneshot(request).await.unwrap();
    let status = response.status();
    let bytes = to_bytes(response.into_body(), 1_000_000).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

#[tokio::test]
async fn child_lifecycle_action_binding_and_account_scope_are_enforced() {
    let (service, store) = AgentRunService::in_memory_with_store(120_000);
    let session = Uuid::now_v7();
    store.register_session(session, "owner-a").await;
    let app = router(service);
    let create = format!("/remote/sessions/{session}/children");
    let(status,created)=send(&app,"POST",&create,Some("owner-a"),Some(json!({"parent_run_id":session.to_string(),"role":"reviewer","task":"Inspect the patch","budget_tokens":4096}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let child = created["data"]["child_id"].as_str().unwrap();
    assert_eq!(created["data"]["depth"], 1);

    let event_uri = format!("/remote/sessions/{session}/children/{child}/events");
    let (status, _) = send(&app, "GET", &event_uri, Some("owner-b"), None).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    let(status,_)=send(&app,"POST",&create,Some("owner-a"),Some(json!({"parent_run_id":session.to_string(),"role":"reviewer","task":"x","budget_tokens":4096,"unmapped":true}))).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let(status,_)=send(&app,"POST",&create,Some("owner-a"),Some(json!({"parent_run_id":session.to_string(),"role":"reviewer","task":"Too much","budget_tokens":8193}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let(status,_)=send(&app,"POST",&create,Some("owner-a"),Some(json!({"parent_run_id":Uuid::now_v7().to_string(),"role":"reviewer","task":"Forged parent","budget_tokens":1024}))).await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = send(
        &app,
        "POST",
        &event_uri,
        Some("owner-a"),
        Some(json!({"event_id":Uuid::now_v7(),"kind":"progress","message":"Review started"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let (status, _) = send(
        &app,
        "POST",
        &event_uri,
        Some("owner-a"),
        Some(json!({"event_id":Uuid::now_v7(),"kind":"progress","message":"\u{1b}[31mforged terminal text"})),
    ).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let action_id = Uuid::now_v7();
    let digest = "a".repeat(64);
    let action_uri = format!("/remote/sessions/{session}/children/{child}/actions");
    let(status,_)=send(&app,"POST",&action_uri,Some("owner-a"),Some(json!({"action_id":Uuid::now_v7(),"parent_run_id":session.to_string(),"kind":"command","payload_digest":digest.clone(),"display":"command: cargo test"}))).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let(status,_)=send(&app,"POST",&action_uri,Some("owner-a"),Some(json!({"action_id":action_id,"parent_run_id":session.to_string(),"kind":"command","payload_digest":digest.clone(),"display":"cwd: workspace\ncommand: cargo test"}))).await;
    assert_eq!(status, StatusCode::CREATED);
    let answer_uri =
        format!("/remote/sessions/{session}/children/{child}/actions/{action_id}/answer");
    let (status, _) = send(
        &app,
        "POST",
        &answer_uri,
        Some("owner-a"),
        Some(json!({"approved":true,"payload_digest":"b".repeat(64)})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, answer) = send(
        &app,
        "POST",
        &answer_uri,
        Some("owner-a"),
        Some(json!({"approved":false,"payload_digest":digest})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(answer["data"]["approved"], false);
    let (status, _) = send(
        &app,
        "POST",
        &answer_uri,
        Some("owner-a"),
        Some(json!({"approved":true,"payload_digest":"a".repeat(64)})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);

    let cancel_uri = format!("/remote/sessions/{session}/children/{child}/cancel");
    let (status, _) = send(&app, "POST", &cancel_uri, Some("owner-a"), None).await;
    assert_eq!(status, StatusCode::ACCEPTED);
    let (status, _) = send(
        &app,
        "POST",
        &event_uri,
        Some("owner-a"),
        Some(json!({"event_id":Uuid::now_v7(),"kind":"completed","result":"done"})),
    )
    .await;
    assert_eq!(status, StatusCode::CONFLICT);
    let (status, _) = send(
        &app,
        "POST",
        &event_uri,
        Some("owner-a"),
        Some(json!({"event_id":Uuid::now_v7(),"kind":"cancelled"})),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
}

#[tokio::test]
async fn api_never_reports_cancelled_until_cli_reports_process_stopped() {
    let (_service, store): (AgentRunService, std::sync::Arc<InMemoryAgentRunStore>) =
        AgentRunService::in_memory_with_store(30_000);
    let session = Uuid::now_v7();
    store.register_session(session, "owner").await;
    let now = chrono::Utc::now();
    let run = store
        .create_child(
            "owner",
            session,
            &session.to_string(),
            "worker",
            "Do work",
            4096,
            4,
            2,
            8192,
            now,
        )
        .await
        .unwrap();
    store
        .append_event(
            "owner",
            session,
            run.child_id,
            Uuid::now_v7(),
            cortex_api::domain::services::agent_runs::ChildEventKind::Progress,
            Some("running"),
            None,
            None,
            now,
        )
        .await
        .unwrap();
    store
        .request_cancel("owner", session, run.child_id, Uuid::now_v7(), now)
        .await
        .unwrap();
    let (state, _) = store
        .events_after("owner", session, run.child_id, 0, 32)
        .await
        .unwrap();
    assert_eq!(state.status.as_str(), "running");
}

#[tokio::test]
async fn postgres_child_run_binding_cas_cancel_and_migration_are_consistent() {
    dotenvy::dotenv().ok();
    if std::env::var_os("CORTEX_TEST_DATABASE_URL").is_none() {
        eprintln!("Skipping PostgreSQL agent-run test: CORTEX_TEST_DATABASE_URL is not set");
        return;
    }
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_agent_runs_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    async {
        let options=database::options(Target::Test).unwrap().options([("search_path",schema.as_str())]);
        let pool=sqlx::postgres::PgPoolOptions::new().max_connections(4).connect_with(options).await.unwrap();
        sqlx::migrate!().run(&pool).await.unwrap();
        let session=Uuid::now_v7();let now=chrono::Utc::now();
        sqlx::query("INSERT INTO remote_cli_sessions(session_id,owner_user,device_name,platform,cli_connected,last_seen_at,last_disconnect_at,next_cli_seq,next_ctrl_seq,created_at) VALUES($1,'owner','test','linux',true,$2,NULL,0,0,$2)").bind(session).bind(now).execute(&pool).await.unwrap();
        let store=PostgresAgentRunStore::new(pool.clone());
        let root=store.create_child("owner",session,&session.to_string(),"parent","Root job",8192,4,2,8192,now).await.unwrap();
        assert_eq!(root.status,ChildRunStatus::Queued);
        store.append_event("owner",session,root.child_id,Uuid::now_v7(),ChildEventKind::Progress,Some("started"),None,None,now).await.unwrap();
        let action_id=Uuid::now_v7();let digest="f".repeat(64);
        store.create_action("owner",session,root.child_id,PendingChildAction{action_id,parent_run_id:root.parent_run_id.clone(),action_kind:"command".into(),payload_digest:digest.clone(),display:"cwd: workspace\ncommand: cargo test".into(),expires_at:now+chrono::Duration::seconds(30)},now,Uuid::now_v7()).await.unwrap();
        let nested=store.create_child("owner",session,&root.child_id.to_string(),"child","Nested",4096,4,2,8192,now).await.unwrap();
        store.request_cancel("owner",session,root.child_id,Uuid::now_v7(),now).await.unwrap();
        assert_eq!(store.answer_action("owner",session,root.child_id,action_id,true,&digest,Uuid::now_v7(),now).await.unwrap_err(),cortex_api::domain::services::agent_runs::ChildRunError::Conflict);
        let (_,root_events)=store.events_after("owner",session,root.child_id,0,32).await.unwrap();
        assert!(root_events.iter().any(|e|e.kind==ChildEventKind::ActionAnswered&&e.approved==Some(false)));
        let (nested_state,nested_events)=store.events_after("owner",session,nested.child_id,0,32).await.unwrap();
        assert!(nested_state.cancel_requested);
        assert!(nested_events.iter().any(|e|e.kind==ChildEventKind::CancelRequested));
        store.append_event("owner",session,root.child_id,Uuid::now_v7(),ChildEventKind::Cancelled,None,None,None,now).await.unwrap();
        assert_eq!(store.events_after("owner",session,root.child_id,0,32).await.unwrap().0.status,ChildRunStatus::Cancelled);
        assert_eq!(store.events_after("other",session,root.child_id,0,32).await.unwrap_err(),cortex_api::domain::services::agent_runs::ChildRunError::Forbidden);
        pool.close().await;
    }.await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
