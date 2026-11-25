use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use serde_json::json;
use sqlx::postgres::PgPoolOptions;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, watch};
use tower::ServiceExt;
use uuid::Uuid;

use cortex_api::core::{config::Config, logs::LogHub, process_numbers};
use cortex_api::dal::{
    local::{
        agent_runs::PostgresAgentRunStore,
        chats::Chats,
        database::{self, Target},
        remote_cli::PostgresRemoteCliStore,
    },
    remote::{
        cache::RedisCache, event_journal::EventJournal, hippocampus::Hippocampus, runs::RunStore,
        timing::TimingStore, usage::RedisLedger, web_page::PageReader,
    },
};
use cortex_api::domain::{
    models::{
        driver::{CallInput, CallOutcome, DriverError, DriverEvent, ModelDriver, OutputModel},
        run_record::ModelCall,
        security::{ClassifierUnavailable, InjectionClassifier},
        usage::Pending,
    },
    services::{
        chat_delete::ChatDelete,
        chat_history::ChatHistory,
        chat_runtime::ChatRuntime,
        crawler::WebRetriever,
        exchange_memory::ExchangeMemories,
        extraction::DocumentExtractor,
        files::FileProxy,
        history::History,
        memory_graph::MemoryGraph,
        personas::Personas,
        recall::MemoryRecall,
        remote_cli::{RemoteCliEvent, RemoteCliService, RemoteEventKind},
        run_evidence::RunEvidence,
        search::WebSearch,
        security::SecurityShield,
        telemetry_reads::TelemetryReads,
        temporary::TemporaryChats,
        thinking::Thinking,
        transcription::AudioTranscriber,
    },
};
use cortex_api::presentation::{AppServices, app_router};

struct NoAudio;
impl AudioTranscriber for NoAudio {
    fn transcribe<'a>(
        &'a self,
        _: &'a cortex_api::domain::models::identity::Caller,
        _: &'a str,
        _: Arc<Vec<u8>>,
        _: &'a cortex_api::domain::services::events::EventSink,
    ) -> Pending<'a, Result<String, process_numbers::ProcessNumber>> {
        Box::pin(async { Err(process_numbers::AUDIO_FAILED) })
    }
}

struct CleanShield;
impl InjectionClassifier for CleanShield {
    fn probability(&self, _: &str) -> Result<f32, ClassifierUnavailable> {
        Ok(0.0)
    }
}

struct StubDriver;
impl ModelDriver for StubDriver {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        _: mpsc::UnboundedSender<DriverEvent>,
        _: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            let now = chrono::Utc::now();
            Ok(CallOutcome {
                inspection: None,
                text: String::new(),
                usage: ModelCall {
                    images: None,
                    audio: None,
                    role: input.role,
                    model: output.name(&Config::default()).into(),
                    input_tokens: 0,
                    output_tokens: 0,
                    time_to_first_token_ms: None,
                    total_ms: 0,
                    started_at: now,
                    finished_at: now,
                },
                error: None,
                load_ms: 0,
                tokens_per_second: None,
                tool_request: None,
            })
        })
    }
}

fn build_real_app_router(
    pool: sqlx::PgPool,
    cache: RedisCache,
    remote_cli: RemoteCliService,
) -> axum::Router {
    build_real_app_router_with_agent_runs(
        pool,
        cache,
        remote_cli,
        cortex_api::domain::services::agent_runs::AgentRunService::in_memory(120_000),
    )
    .0
}

fn build_real_app_router_with_agent_runs(
    pool: sqlx::PgPool,
    cache: RedisCache,
    remote_cli: RemoteCliService,
    agent_runs: cortex_api::domain::services::agent_runs::AgentRunService,
) -> (axum::Router, Arc<ChatRuntime>) {
    let config = Config {
        hippocampus_url: "http://127.0.0.1:1".into(),
        ..Default::default()
    };
    let store = Arc::new(
        RunStore::new(
            &config,
            reqwest::Url::parse("http://127.0.0.1:1/runs").unwrap(),
            "u".into(),
            "p".into(),
        )
        .unwrap(),
    );
    let ledger = Arc::new(RedisLedger::new(
        cache.clone(),
        store.clone(),
        config.local_queue_name.into(),
    ));
    let driver: Arc<dyn ModelDriver> = Arc::new(StubDriver);
    let shield = Arc::new(SecurityShield::new(&config, Arc::new(CleanShield)).unwrap());
    let hippocampus = Hippocampus::new(&config)
        .unwrap()
        .with_overview_cache(cache.clone());
    let chats = Chats { pool: pool.clone() };
    let temporary = TemporaryChats::new(&config, cache.clone());
    let personas = Personas {
        hippocampus: hippocampus.clone(),
        shield: shield.clone(),
    };
    let runtime = Arc::new(ChatRuntime {
        history: History::new(chats.clone(), cache.clone(), &config).unwrap(),
        files: FileProxy {
            hippocampus: hippocampus.clone(),
            extractor: DocumentExtractor::new(&config),
            transcriber: Arc::new(NoAudio),
            temporary: temporary.clone(),
            config: config.clone(),
        },
        temporary,
        recall: MemoryRecall::new(&config, hippocampus.clone(), shield.clone()),
        web: WebRetriever::new(
            &config,
            WebSearch::new(&config).unwrap(),
            PageReader::new(&config).unwrap(),
            shield.clone(),
            driver.clone(),
        ),
        thinking: Thinking::new(&config, driver),
        memories: ExchangeMemories {
            repo: chats.clone(),
            hippocampus: hippocampus.clone(),
        },
        runs: RunEvidence::new(store.clone()),
        telemetry: TelemetryReads {
            store: store.clone(),
            timing: TimingStore::new(&config, cache.clone()),
            config: config.clone(),
        },
        journal: EventJournal::new(cache.clone(), &config).unwrap(),
        controls: Default::default(),
        agent_runs: agent_runs.clone(),
        personas: personas.clone(),
        shield,
        chats: chats.clone(),
        config: config.clone(),
    });
    let logs = LogHub::new(&config);
    let app = app_router(AppServices {
        config: config.clone(),
        runtime: runtime.clone(),
        personas,
        chat_delete: ChatDelete {
            history: ChatHistory {
                repo: chats.clone(),
                config: config.clone(),
                hippocampus: hippocampus.clone(),
            },
            hippocampus: hippocampus.clone(),
        },
        telemetry_reads: TelemetryReads {
            store,
            timing: TimingStore::new(&config, cache.clone()),
            config: config.clone(),
        },
        memory_graph: MemoryGraph {
            hippocampus,
            config: config.clone(),
            records: None,
            cache,
        },
        chats,
        logs,
        ledger,
        remote_cli,
        agent_runs,
    });
    (app, runtime)
}

fn lazy_test_pool() -> sqlx::PgPool {
    PgPoolOptions::new()
        .max_connections(1)
        .connect_lazy("postgresql://cortex_api_app:test@127.0.0.1:5432/cortex_api_test")
        .unwrap()
}

async fn json_req(
    app: &axum::Router,
    method: &str,
    uri: &str,
    user: Option<&str>,
    body: Option<serde_json::Value>,
) -> (StatusCode, serde_json::Value) {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(u) = user {
        builder = builder.header("x-uuid", u);
    }
    let req = if let Some(b) = body {
        builder
            .header("content-type", "application/json")
            .body(Body::from(b.to_string()))
            .unwrap()
    } else {
        builder.body(Body::empty()).unwrap()
    };
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let v = serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null);
    (status, v)
}

async fn read_sse_events(url: &str, user: &str, expected_count: usize) -> Vec<RemoteCliEvent> {
    let client = reqwest::Client::new();
    let resp = client.get(url).header("x-uuid", user).send().await.unwrap();
    assert_eq!(resp.status(), reqwest::StatusCode::OK);
    let mut stream = resp.bytes_stream();
    let mut buf = String::new();
    let mut events = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
    while events.len() < expected_count {
        let next = tokio::time::timeout_at(deadline, stream.next()).await;
        match next {
            Ok(Some(Ok(chunk))) => {
                buf.push_str(&String::from_utf8_lossy(&chunk));
                while let Some(idx) = buf.find("\n\n") {
                    let frame = buf[..idx].to_string();
                    buf.drain(..idx + 2);
                    for line in frame.lines() {
                        if let Some(data) = line.strip_prefix("data:") {
                            let ev: RemoteCliEvent = serde_json::from_str(data.trim()).unwrap();
                            events.push(ev);
                        }
                    }
                }
            }
            _ => break,
        }
    }
    events
}

#[tokio::test]
async fn remote_cli_real_app_router_isolation_cursor_sse_approval_and_cancel() {
    let svc = RemoteCliService::in_memory(5_000, 1_000, 3_600_000);
    let app = build_real_app_router(
        lazy_test_pool(),
        RedisCache::new(None, Duration::from_millis(50)),
        svc.clone(),
    );

    // Gateway identity header x-uuid is required
    let (st_no_auth, err_no_auth) = json_req(&app, "GET", "/remote/sessions", None, None).await;
    assert_eq!(st_no_auth, StatusCode::BAD_REQUEST);
    assert_eq!(
        err_no_auth["data"]["error_number"],
        process_numbers::MISSING_USER.number
    );

    // Register session as user A through the real application router
    let (st, reg) = json_req(
        &app,
        "POST",
        "/remote/sessions",
        Some("user-a"),
        Some(json!({ "device_name": "laptop", "platform": "linux" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = reg["data"]["session_id"].as_str().unwrap().to_string();

    // User B cannot list, send prompts, post events, answer approvals, cancel, or unregister
    let (st_b_list, list_b) = json_req(&app, "GET", "/remote/sessions", Some("user-b"), None).await;
    assert_eq!(st_b_list, StatusCode::OK);
    assert!(list_b["data"]["sessions"].as_array().unwrap().is_empty());

    for (method, uri, body) in [
        (
            "POST",
            format!("/remote/sessions/{sid}/prompts"),
            Some(json!({ "prompt": "hijack" })),
        ),
        (
            "POST",
            format!("/remote/sessions/{sid}/events"),
            Some(json!({ "kind": "progress", "payload": {} })),
        ),
        (
            "POST",
            format!("/remote/sessions/{sid}/approvals/apr-1"),
            Some(json!({ "approved": true })),
        ),
        (
            "POST",
            format!("/remote/sessions/{sid}/cancel"),
            Some(json!({ "turn_id": "t-1" })),
        ),
        ("GET", format!("/remote/sessions/{sid}/commands"), None),
        ("GET", format!("/remote/sessions/{sid}/events"), None),
        ("DELETE", format!("/remote/sessions/{sid}"), None),
    ] {
        let (st_b, _) = json_req(&app, method, &uri, Some("user-b"), body).await;
        assert_eq!(
            st_b,
            StatusCode::FORBIDDEN,
            "{method} {uri} must reject user-b"
        );
    }

    // Send prompt + approval request
    let (st_p, p_ev) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{sid}/prompts"),
        Some("user-a"),
        Some(json!({ "prompt": "run tests", "file_ids": ["fsm-1"] })),
    )
    .await;
    assert_eq!(st_p, StatusCode::ACCEPTED);
    let first_cmd_id = p_ev["data"]["id"].as_u64().unwrap();

    let (st_ev, _) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{sid}/events"),
        Some("user-a"),
        Some(json!({
            "kind": "approval_request",
            "payload": { "approval_id": "apr-1", "action": "git push" },
            "ack_command_id": first_cmd_id
        })),
    )
    .await;
    assert_eq!(st_ev, StatusCode::CREATED);

    // Approval answered twice: first succeeds, second is rejected (409 Conflict)
    let (st_a1, _) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{sid}/approvals/apr-1"),
        Some("user-a"),
        Some(json!({ "approved": true })),
    )
    .await;
    assert_eq!(st_a1, StatusCode::OK);

    let (st_a2, err_a2) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{sid}/approvals/apr-1"),
        Some("user-a"),
        Some(json!({ "approved": false })),
    )
    .await;
    assert_eq!(st_a2, StatusCode::CONFLICT);
    assert_eq!(
        err_a2["data"]["error_number"],
        process_numbers::REMOTE_APPROVAL_ANSWERED.number
    );

    // CLI-first approval answer also blocks subsequent remote answer
    let (st_local, _) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{sid}/events"),
        Some("user-a"),
        Some(json!({
            "kind": "approval_answer",
            "payload": { "approval_id": "apr-local", "approved": false }
        })),
    )
    .await;
    assert_eq!(st_local, StatusCode::CREATED);
    let (st_remote_dup, _) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{sid}/approvals/apr-local"),
        Some("user-a"),
        Some(json!({ "approved": true })),
    )
    .await;
    assert_eq!(st_remote_dup, StatusCode::CONFLICT);

    // Cancel delivers Cancel command to CLI stream
    let (st_c, _) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{sid}/cancel"),
        Some("user-a"),
        Some(json!({ "turn_id": "t-1" })),
    )
    .await;
    assert_eq!(st_c, StatusCode::ACCEPTED);

    // Real HTTP SSE reconnect with cursor gives no lost or duplicate events
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv_handle = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    let base = format!("http://{addr}");

    let cmds = read_sse_events(
        &format!("{base}/remote/sessions/{sid}/commands?cursor={first_cmd_id}"),
        "user-a",
        2,
    )
    .await;
    assert_eq!(cmds.len(), 2);
    assert_eq!(cmds[0].id, 2);
    assert_eq!(cmds[0].kind, RemoteEventKind::ApprovalAnswer);
    assert_eq!(cmds[1].id, 3);
    assert_eq!(cmds[1].kind, RemoteEventKind::Cancel);

    let ctrl_evs = read_sse_events(
        &format!("{base}/remote/sessions/{sid}/events?cursor=1"),
        "user-a",
        3,
    )
    .await;
    assert_eq!(ctrl_evs.len(), 3);
    assert_eq!(ctrl_evs[0].id, 2);
    assert_eq!(ctrl_evs[1].id, 3);
    assert_eq!(ctrl_evs[2].id, 4);

    // Pending command expiry & offline detection
    svc.set_cli_connected(&sid, "user-a", false, 100)
        .await
        .unwrap();
    assert!(!svc.list_sessions("user-a", 10_000).await.unwrap()[0].online);
    assert!(svc.expire_pending_commands(u64::MAX).await.unwrap() >= 1);
    srv_handle.abort();
}

#[tokio::test]
#[ignore = "requires the explicitly approved cortex_api_test database"]
async fn output_action_route_enforces_one_executor_and_idempotent_registration() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_output_actions_{}", Uuid::now_v7().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();

    async {
        let cache = RedisCache::new(None, Duration::from_millis(25));
        let config = Config::default();
        let remote_cli = RemoteCliService::new(
            Arc::new(PostgresRemoteCliStore::new(pool.clone())),
            cache.clone(),
            &config,
        );
        let first_session = remote_cli
            .register("owner", "test cli", "linux", 1_000)
            .await
            .unwrap()
            .session_id;
        let second_session = remote_cli
            .register("owner", "second test cli", "linux", 1_001)
            .await
            .unwrap()
            .session_id;

        let agent_runs = cortex_api::domain::services::agent_runs::AgentRunService::new(
            Arc::new(PostgresAgentRunStore::new(pool.clone())),
            120_000,
            4,
            2,
            8192,
        );
        let (app, runtime) = build_real_app_router_with_agent_runs(
            pool.clone(),
            cache,
            remote_cli.clone(),
            agent_runs,
        );

        let chat = Uuid::now_v7();
        let input = Uuid::now_v7();
        let output = Uuid::now_v7();
        let call = Uuid::now_v7();
        let now = chrono::Utc::now();
        sqlx::query("INSERT INTO chats(session_id,app_id,user_id,title,created_at,updated_at) VALUES($1,'cortex','owner','output action route test',$2,$2)")
            .bind(chat).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO chat_inputs(input_id,session_id,text,needs_web,web_mode,use_memory,thinking,created_at) VALUES($1,$2,'typed tool fixture',false,'fast',false,false,$3)")
            .bind(input).bind(chat).bind(now).execute(&pool).await.unwrap();
        sqlx::query("INSERT INTO chat_outputs(output_id,input_id,session_id,status,text,created_at) VALUES($1,$2,$3,'running','',$4)")
            .bind(output).bind(input).bind(chat).bind(now).execute(&pool).await.unwrap();

        let (tool_result_tx, _tool_results) = tokio::sync::mpsc::unbounded_channel();
        let tools = cortex_api::domain::models::tools::ToolControl::new(tool_result_tx);
        assert!(tools.set_pending(cortex_api::domain::models::tools::PendingTool {
            call_id: call,
            name: cortex_api::domain::models::tools::ToolName::RunCommand,
            arguments: json!({"cwd":".","argv":["true"]}),
        }));
        let (cancel, _) = watch::channel(false);
        let (stop_thinking, _) = watch::channel(false);
        let (live, _) = tokio::sync::broadcast::channel(8);
        runtime.controls.open(
            output,
            cortex_api::domain::services::output_controls::Control {
                cancel: Arc::new(cancel),
                stop_thinking: Arc::new(stop_thinking),
                live,
                tools,
            },
        );

        let path = format!("/remote/sessions/{first_session}/outputs/{chat}/{output}/actions");
        let (status, first) = json_req(
            &app,
            "POST",
            &path,
            Some("owner"),
            Some(json!({"call_id":call})),
        )
        .await;
        assert_eq!(status, StatusCode::CREATED);
        let action_id = first["data"]["action_id"].as_str().unwrap();
        let (retry_status, retry) = json_req(
            &app,
            "POST",
            &path,
            Some("owner"),
            Some(json!({"call_id":call})),
        )
        .await;
        assert_eq!(retry_status, StatusCode::OK);
        assert_eq!(retry["data"]["action_id"], action_id);

        let (events, _) = remote_cli
            .ctrl_events_since(&first_session, "owner", 0)
            .await
            .unwrap();
        assert_eq!(events.len(), 1, "registration retry must not republish approval");

        let second_path = format!("/remote/sessions/{second_session}/outputs/{chat}/{output}/actions");
        let (second_status, _) = json_req(
            &app,
            "POST",
            &second_path,
            Some("owner"),
            Some(json!({"call_id":call})),
        )
        .await;
        assert_eq!(second_status, StatusCode::CONFLICT);

        let action_id = action_id.to_owned();
        let answer_path = format!("{path}/{action_id}/answer");
        let (answer_status, _) = json_req(
            &app,
            "POST",
            &answer_path,
            Some("owner"),
            Some(json!({"approved":true})),
        )
        .await;
        assert_eq!(answer_status, StatusCode::OK);
        let claim_path = format!("{path}/{action_id}/claim");
        let digest = first["data"]["action_digest"].as_str().unwrap();
        let claim = json!({"call_id":call,"action_digest":digest});
        let (claim_status, _) = json_req(
            &app,
            "POST",
            &claim_path,
            Some("owner"),
            Some(claim.clone()),
        )
        .await;
        assert_eq!(claim_status, StatusCode::OK);
        let (duplicate_claim, _) = json_req(
            &app,
            "POST",
            &claim_path,
            Some("owner"),
            Some(claim),
        )
        .await;
        assert_eq!(duplicate_claim, StatusCode::CONFLICT);
    }
    .await;
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires cortex_api_test and Valkey"]
async fn remote_cli_postgres_and_valkey_multi_instance_persistence_and_sse_fanout() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_remote_cli_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA {schema}"))
        .execute(&admin)
        .await
        .unwrap();
    let options = database::options(Target::Test)
        .unwrap()
        .options([("search_path", schema.as_str())]);
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect_with(options)
        .await
        .unwrap();
    sqlx::migrate!().run(&pool).await.unwrap();

    let Some((_, cache)) = RedisCache::from_test_environment() else {
        eprintln!("Skipping remote_cli test: CORTEX_TEST_REDIS_URL is not set");
        return;
    };
    let config = Config::default();

    // Instance 1
    let svc1 = RemoteCliService::new(
        Arc::new(PostgresRemoteCliStore::new(pool.clone())),
        cache.clone(),
        &config,
    );
    let app1 = build_real_app_router(pool.clone(), cache.clone(), svc1.clone());

    let (st, reg) = json_req(
        &app1,
        "POST",
        "/remote/sessions",
        Some("alice"),
        Some(json!({ "device_name": "thinkpad", "platform": "linux" })),
    )
    .await;
    assert_eq!(st, StatusCode::CREATED);
    let sid = reg["data"]["session_id"].as_str().unwrap().to_string();

    let (st_p, p_ev) = json_req(
        &app1,
        "POST",
        &format!("/remote/sessions/{sid}/prompts"),
        Some("alice"),
        Some(json!({ "prompt": "explain error", "file_ids": ["fsm-file-9"] })),
    )
    .await;
    assert_eq!(st_p, StatusCode::ACCEPTED);
    let first_cmd_id = p_ev["data"]["id"].as_u64().unwrap();
    assert_eq!(first_cmd_id, 1);

    // Instance 2: brand-new service instance with no shared in-memory state
    let svc2 = RemoteCliService::new(
        Arc::new(PostgresRemoteCliStore::new(pool.clone())),
        cache.clone(),
        &config,
    );
    let app2 = build_real_app_router(pool.clone(), cache.clone(), svc2.clone());

    // Instance 2 sees session persisted in Postgres + Valkey presence
    let (st_list, list_v) = json_req(&app2, "GET", "/remote/sessions", Some("alice"), None).await;
    assert_eq!(st_list, StatusCode::OK);
    let sessions = list_v["data"]["sessions"].as_array().unwrap();
    assert_eq!(sessions.len(), 1);
    assert_eq!(sessions[0]["session_id"], sid);
    assert_eq!(sessions[0]["online"], true);

    // User Bob cannot see or control Alice's session on Instance 2
    let (st_bob, _) = json_req(
        &app2,
        "POST",
        &format!("/remote/sessions/{sid}/cancel"),
        Some("bob"),
        Some(json!({ "turn_id": "t-9" })),
    )
    .await;
    assert_eq!(st_bob, StatusCode::FORBIDDEN);

    // Open SSE stream on Instance 2 while Instance 1 sends a live Cancel command (Valkey Pub/Sub fan-out)
    let listener2 = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr2 = listener2.local_addr().unwrap();
    let srv2 = tokio::spawn(async move {
        axum::serve(listener2, app2.clone()).await.unwrap();
    });
    let sid_clone = sid.clone();
    let app1_clone = app1.clone();
    let sse_task = tokio::spawn(async move {
        read_sse_events(
            &format!("http://{addr2}/remote/sessions/{sid_clone}/commands?cursor=0"),
            "alice",
            2,
        )
        .await
    });

    tokio::time::sleep(Duration::from_millis(150)).await;
    let (st_cancel, _) = json_req(
        &app1_clone,
        "POST",
        &format!("/remote/sessions/{sid}/cancel"),
        Some("alice"),
        Some(json!({ "turn_id": "turn-live" })),
    )
    .await;
    assert_eq!(st_cancel, StatusCode::ACCEPTED);

    let streamed_cmds = sse_task.await.unwrap();
    assert_eq!(streamed_cmds.len(), 2);
    assert_eq!(streamed_cmds[0].kind, RemoteEventKind::Prompt);
    assert_eq!(streamed_cmds[0].payload["file_ids"], json!(["fsm-file-9"]));
    assert_eq!(streamed_cmds[1].kind, RemoteEventKind::Cancel);
    assert_eq!(streamed_cmds[1].payload["turn_id"], "turn-live");

    // Approval first-answer-wins across instances
    let (st_ap1, _) = json_req(
        &app1,
        "POST",
        &format!("/remote/sessions/{sid}/approvals/apr-pg"),
        Some("alice"),
        Some(json!({ "approved": true })),
    )
    .await;
    assert_eq!(st_ap1, StatusCode::OK);
    let (st_ap2, _) = json_req(
        &app1,
        "POST",
        &format!("/remote/sessions/{sid}/approvals/apr-pg"),
        Some("alice"),
        Some(json!({ "approved": false })),
    )
    .await;
    assert_eq!(st_ap2, StatusCode::CONFLICT);

    // Command expiry + session retention cleanup
    assert!(svc2.expire_pending_commands(u64::MAX).await.unwrap() >= 1);
    let (_, ctrl_after) = svc2.ctrl_events_since(&sid, "alice", 0).await.unwrap();
    drop(ctrl_after);
    let (st_del, _) = json_req(
        &app1,
        "DELETE",
        &format!("/remote/sessions/{sid}"),
        Some("alice"),
        None,
    )
    .await;
    assert_eq!(st_del, StatusCode::OK);

    // Register another session and verify purge_expired_sessions honors retention TTL
    let exp_view = svc1
        .register("alice", "old-mac", "macos", 1_000)
        .await
        .unwrap();
    assert_eq!(svc2.purge_expired_sessions(u64::MAX).await.unwrap(), 1);
    assert!(
        svc2.list_sessions("alice", u64::MAX)
            .await
            .unwrap()
            .is_empty()
    );
    let _ = svc2.unregister(&exp_view.session_id, "alice").await;

    srv2.abort();
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}
