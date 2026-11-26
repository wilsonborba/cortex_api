use axum::{
    body::{Body, to_bytes},
    http::{Request, StatusCode},
};
use futures_util::StreamExt;
use serde_json::{Value, json};
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
        agent_runs::AgentRunStore,
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
                usage_measured: true,
                error: None,
                load_ms: 0,
                tokens_per_second: None,
                tool_request: None,
            })
        })
    }
}

struct PausedCancelDriver {
    started: tokio::sync::mpsc::UnboundedSender<Uuid>,
    cancel_seen: Arc<tokio::sync::Notify>,
    release: Arc<tokio::sync::Notify>,
}

struct LifecycleRunStore {
    inner: Arc<cortex_api::domain::services::agent_runs::InMemoryAgentRunStore>,
    created: tokio::sync::mpsc::UnboundedSender<Uuid>,
    pause_create: std::sync::atomic::AtomicBool,
    release_create: tokio::sync::Semaphore,
    bind_started: tokio::sync::mpsc::UnboundedSender<Uuid>,
    bind_completed: tokio::sync::mpsc::UnboundedSender<Uuid>,
    pause_bind: std::sync::atomic::AtomicBool,
    release_bind: tokio::sync::Semaphore,
}

macro_rules! forward_agent_store {
    ($name:ident($($arg:ident: $ty:ty),*) -> $result:ty) => {
        fn $name<'a>(
            &'a self,
            $($arg: $ty),*
        ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<$result, cortex_api::domain::services::agent_runs::ChildRunError>> + Send + 'a>> {
            Box::pin(async move { self.inner.$name($($arg),*).await })
        }
    };
}

#[allow(clippy::too_many_arguments)]
impl cortex_api::domain::services::agent_runs::AgentRunStore for LifecycleRunStore {
    forward_agent_store!(child_for_output(owner: &'a str, session_id: Uuid, chat_session_id: Uuid, output_id: Uuid) -> Option<cortex_api::domain::services::agent_runs::ChildRun>);

    fn create_bound_child<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        request: cortex_api::domain::services::agent_runs::BoundChildRequest,
        max_active: usize,
        max_depth: u8,
        max_budget_tokens: u32,
        now: chrono::DateTime<chrono::Utc>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        cortex_api::domain::services::agent_runs::ChildRun,
                        cortex_api::domain::services::agent_runs::ChildRunError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let child = self
                .inner
                .create_bound_child(
                    owner,
                    session_id,
                    request,
                    max_active,
                    max_depth,
                    max_budget_tokens,
                    now,
                )
                .await?;
            if self
                .pause_create
                .swap(false, std::sync::atomic::Ordering::AcqRel)
            {
                let _ = self.created.send(child.child_id);
                self.release_create
                    .acquire()
                    .await
                    .map_err(|_| {
                        cortex_api::domain::services::agent_runs::ChildRunError::Unavailable
                    })?
                    .forget();
            }
            Ok(child)
        })
    }

    fn bind_child_output<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        child_chat_session_id: Uuid,
        child_output_id: Uuid,
        now: chrono::DateTime<chrono::Utc>,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        cortex_api::domain::services::agent_runs::ChildRun,
                        cortex_api::domain::services::agent_runs::ChildRunError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        Box::pin(async move {
            let paused = self
                .pause_bind
                .swap(false, std::sync::atomic::Ordering::AcqRel);
            if paused {
                let _ = self.bind_started.send(child_output_id);
                self.release_bind
                    .acquire()
                    .await
                    .map_err(|_| {
                        cortex_api::domain::services::agent_runs::ChildRunError::Unavailable
                    })?
                    .forget();
            }
            let child = self
                .inner
                .bind_child_output(
                    owner,
                    session_id,
                    child_id,
                    child_chat_session_id,
                    child_output_id,
                    now,
                )
                .await?;
            if paused {
                let _ = self.bind_completed.send(child_output_id);
            }
            Ok(child)
        })
    }

    forward_agent_store!(report_child_usage(owner: &'a str, session_id: Uuid, child_id: Uuid, used_tokens: u64, now: chrono::DateTime<chrono::Utc>) -> ());
    forward_agent_store!(settle_child_budget(owner: &'a str, session_id: Uuid, child_id: Uuid, used_tokens: u64, now: chrono::DateTime<chrono::Utc>) -> ());
    forward_agent_store!(create_child(owner: &'a str, session_id: Uuid, parent_run_id: &'a str, role: &'a str, task: &'a str, budget_tokens: u32, max_active: usize, max_depth: u8, max_budget_tokens: u32, now: chrono::DateTime<chrono::Utc>) -> cortex_api::domain::services::agent_runs::ChildRun);
    forward_agent_store!(append_event(owner: &'a str, session_id: Uuid, child_id: Uuid, event_id: Uuid, kind: cortex_api::domain::services::agent_runs::ChildEventKind, message: Option<&'a str>, result: Option<&'a str>, error_code: Option<&'a str>, now: chrono::DateTime<chrono::Utc>) -> cortex_api::domain::services::agent_runs::ChildRunEvent);
    forward_agent_store!(request_cancel(owner: &'a str, session_id: Uuid, child_id: Uuid, event_id: Uuid, now: chrono::DateTime<chrono::Utc>) -> cortex_api::domain::services::agent_runs::ChildRunEvent);
    forward_agent_store!(create_action(owner: &'a str, session_id: Uuid, child_id: Uuid, action: cortex_api::domain::services::agent_runs::PendingChildAction, now: chrono::DateTime<chrono::Utc>, event_id: Uuid) -> cortex_api::domain::services::agent_runs::ChildRunEvent);
    forward_agent_store!(answer_action(owner: &'a str, session_id: Uuid, child_id: Uuid, action_id: Uuid, approved: bool, digest: &'a str, event_id: Uuid, now: chrono::DateTime<chrono::Utc>) -> cortex_api::domain::services::agent_runs::ChildRunEvent);
    forward_agent_store!(create_output_action(owner: &'a str, session_id: Uuid, action: cortex_api::domain::services::agent_runs::OutputAction, now: chrono::DateTime<chrono::Utc>) -> cortex_api::domain::services::agent_runs::OutputAction);
    forward_agent_store!(output_action(owner: &'a str, session_id: Uuid, chat_session_id: Uuid, output_id: Uuid, action_id: Uuid) -> cortex_api::domain::services::agent_runs::OutputAction);
    forward_agent_store!(answer_output_action(owner: &'a str, session_id: Uuid, chat_session_id: Uuid, output_id: Uuid, action_id: Uuid, approved: bool, now: chrono::DateTime<chrono::Utc>) -> cortex_api::domain::services::agent_runs::OutputAction);
    forward_agent_store!(claim_output_action(owner: &'a str, session_id: Uuid, chat_session_id: Uuid, output_id: Uuid, action_id: Uuid, call_id: Uuid, digest: &'a str, now: chrono::DateTime<chrono::Utc>) -> cortex_api::domain::services::agent_runs::OutputAction);
    forward_agent_store!(cancel_output_actions(owner: &'a str, chat_session_id: Uuid, output_id: Uuid, now: chrono::DateTime<chrono::Utc>) -> ());

    fn events_after<'a>(
        &'a self,
        owner: &'a str,
        session_id: Uuid,
        child_id: Uuid,
        after: u64,
        limit: usize,
    ) -> std::pin::Pin<
        Box<
            dyn std::future::Future<
                    Output = Result<
                        (
                            cortex_api::domain::services::agent_runs::ChildRun,
                            Vec<cortex_api::domain::services::agent_runs::ChildRunEvent>,
                        ),
                        cortex_api::domain::services::agent_runs::ChildRunError,
                    >,
                > + Send
                + 'a,
        >,
    > {
        self.inner
            .events_after(owner, session_id, child_id, after, limit)
    }
}
impl ModelDriver for PausedCancelDriver {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        _: CallInput,
        _: mpsc::UnboundedSender<DriverEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            let _ = self.started.send(output.output_id);
            while !*cancel.borrow() {
                if cancel.changed().await.is_err() {
                    return Err(DriverError::Cancelled);
                }
            }
            self.cancel_seen.notify_one();
            self.release.notified().await;
            Err(DriverError::Cancelled)
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
    build_real_app_router_with_agent_runs_and_driver(
        pool,
        cache,
        remote_cli,
        agent_runs,
        Arc::new(StubDriver),
    )
}

fn build_real_app_router_with_agent_runs_and_driver(
    pool: sqlx::PgPool,
    cache: RedisCache,
    remote_cli: RemoteCliService,
    agent_runs: cortex_api::domain::services::agent_runs::AgentRunService,
    driver: Arc<dyn ModelDriver>,
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
                runs: None,
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

struct ParentFixture {
    chat: Uuid,
    output: Uuid,
    call: Uuid,
    _cancel_receiver: watch::Receiver<bool>,
    results: mpsc::UnboundedReceiver<cortex_api::domain::models::tools::ToolResult>,
}

async fn parent_fixture(pool: &sqlx::PgPool, runtime: &Arc<ChatRuntime>) -> ParentFixture {
    let chat = Uuid::now_v7();
    let input = Uuid::now_v7();
    let output = Uuid::now_v7();
    let call = Uuid::now_v7();
    let now = chrono::Utc::now();
    sqlx::query("INSERT INTO chats(session_id,app_id,user_id,title,created_at,updated_at) VALUES($1,'cortex','owner','child cancellation fixture',$2,$2)")
        .bind(chat).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO chat_inputs(input_id,session_id,text,needs_web,web_mode,use_memory,thinking,created_at) VALUES($1,$2,'fixture prompt',false,'fast',false,false,$3)")
        .bind(input).bind(chat).bind(now).execute(pool).await.unwrap();
    sqlx::query("INSERT INTO chat_outputs(output_id,input_id,session_id,status,text,created_at) VALUES($1,$2,$3,'running','',$4)")
        .bind(output).bind(input).bind(chat).bind(now).execute(pool).await.unwrap();

    let (result_tx, results) = mpsc::unbounded_channel();
    let tools = cortex_api::domain::models::tools::ToolControl::new(result_tx);
    assert!(
        tools.set_pending(cortex_api::domain::models::tools::PendingTool {
            call_id: call,
            name: cortex_api::domain::models::tools::ToolName::InvokeSubagent,
            arguments: json!({"role":"researcher","task":"wait for cancellation"}),
        })
    );
    let (cancel, cancel_receiver) = watch::channel(false);
    let (stop_thinking, _) = watch::channel(false);
    let (live, _) = tokio::sync::broadcast::channel(8);
    runtime.controls.open(
        output,
        cortex_api::domain::services::output_controls::Control::new(
            Arc::new(cancel),
            Arc::new(stop_thinking),
            live,
            tools,
        ),
    );
    ParentFixture {
        chat,
        output,
        call,
        _cancel_receiver: cancel_receiver,
        results,
    }
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
        let golden: Value =
            serde_json::from_str(include_str!("fixtures/output_action_digest.json")).unwrap();
        let call = Uuid::parse_str(golden["call_id"].as_str().unwrap()).unwrap();
        let command_arguments: Value = serde_json::from_str(
            golden["valid_command"]["arguments_raw"].as_str().unwrap(),
        )
        .unwrap();
        let cli_digest = golden["valid_command"]["digest"].as_str().unwrap();
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
            arguments: command_arguments.clone(),
        }));
        let (cancel, _) = watch::channel(false);
        let (stop_thinking, _) = watch::channel(false);
        let (live, _) = tokio::sync::broadcast::channel(8);
        let test_tools = tools.clone();
        runtime.controls.open(
            output,
            cortex_api::domain::services::output_controls::Control::new(
                Arc::new(cancel),
                Arc::new(stop_thinking),
                live,
                tools,
            ),
        );

        let child_path = format!("/remote/sessions/{first_session}/outputs/{chat}/{output}/children");
        let (wrong_call_status, _) = json_req(
            &app,
            "POST",
            &child_path,
            Some("owner"),
            Some(json!({"call_id":Uuid::now_v7()})),
        )
        .await;
        assert_eq!(wrong_call_status, StatusCode::CONFLICT);
        let (client_authority_status, _) = json_req(
            &app,
            "POST",
            &child_path,
            Some("owner"),
            Some(json!({"call_id":call,"task":"client supplied task"})),
        )
        .await;
        assert_eq!(client_authority_status, StatusCode::UNPROCESSABLE_ENTITY);
        let child_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_cli_children WHERE session_id = $1",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        let child_action_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_cli_child_actions WHERE session_id = $1",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        let (events_after_rejection, _) = remote_cli
            .ctrl_events_since(&first_session, "owner", 0)
            .await
            .unwrap();
        assert_eq!(child_count, 0, "rejected client task must not create a child");
        assert_eq!(child_action_count, 0, "rejected client task must not create actions");
        assert!(
            events_after_rejection.is_empty(),
            "rejected client task must not publish a provider-side effect"
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
        assert_eq!(first["data"]["action_digest"], cli_digest);
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

        let wrong_session_approval_path =
            format!("/remote/sessions/{second_session}/approvals/{action_id}");
        let (wrong_session_approval_status, _) = json_req(
            &app,
            "POST",
            &wrong_session_approval_path,
            Some("owner"),
            Some(json!({"approved":false})),
        )
        .await;
        assert_eq!(wrong_session_approval_status, StatusCode::NOT_FOUND);
        let wrong_session_legacy_decisions: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_cli_approvals
             WHERE session_id = $1 AND approval_id = $2",
        )
        .bind(Uuid::parse_str(&second_session).unwrap())
        .bind(action_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let wrong_session_commands: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_cli_commands
             WHERE session_id = $1 AND approval_id = $2",
        )
        .bind(Uuid::parse_str(&second_session).unwrap())
        .bind(action_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        let (wrong_session_events, _) = remote_cli
            .ctrl_events_since(&second_session, "owner", 0)
            .await
            .unwrap();
        assert_eq!(wrong_session_legacy_decisions, 0);
        assert_eq!(wrong_session_commands, 0);
        assert!(wrong_session_events.is_empty());

        let (events, _) = remote_cli
            .ctrl_events_since(&first_session, "owner", 0)
            .await
            .unwrap();
        assert_eq!(events.len(), 1, "registration retry must not republish approval");
        assert_eq!(
            events[0].payload["action"],
            first["data"]["display"],
            "persisted approval event must retain the action description"
        );

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
        let phone_path = format!("/remote/sessions/{first_session}/approvals/{action_id}");
        let (phone_loser_status, _) = json_req(
            &app,
            "POST",
            &phone_path,
            Some("owner"),
            Some(json!({"approved":false})),
        )
        .await;
        assert_eq!(phone_loser_status, StatusCode::CONFLICT);
        let local_winner_command: (i64, bool) = sqlx::query_as(
            "SELECT COUNT(*), bool_and(approved) FROM remote_cli_commands
             WHERE session_id = $1 AND kind = 'approval_answer' AND approval_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(&action_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(local_winner_command, (1, true));
        let claim_path = format!("{path}/{action_id}/claim");
        let claim = json!({"call_id":call,"action_digest":cli_digest});
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

        let denied_call = Uuid::now_v7();
        test_tools.clear_pending(call);
        assert!(test_tools.set_pending(cortex_api::domain::models::tools::PendingTool {
            call_id: denied_call,
            name: cortex_api::domain::models::tools::ToolName::RunCommand,
            arguments: command_arguments.clone(),
        }));
        let (denied_create_status, denied_action) = json_req(
            &app,
            "POST",
            &path,
            Some("owner"),
            Some(json!({"call_id":denied_call})),
        )
        .await;
        assert_eq!(denied_create_status, StatusCode::CREATED);
        let denied_action_id = denied_action["data"]["action_id"].as_str().unwrap();
        let denied_phone_path =
            format!("/remote/sessions/{first_session}/approvals/{denied_action_id}");
        let (denied_phone_status, _) = json_req(
            &app,
            "POST",
            &denied_phone_path,
            Some("owner"),
            Some(json!({"approved":false})),
        )
        .await;
        assert_eq!(denied_phone_status, StatusCode::OK);
        let denied_local_path = format!("{path}/{denied_action_id}/answer");
        let (denied_local_status, _) = json_req(
            &app,
            "POST",
            &denied_local_path,
            Some("owner"),
            Some(json!({"approved":true})),
        )
        .await;
        assert_eq!(denied_local_status, StatusCode::CONFLICT);
        let (denied_retry_status, _) = json_req(
            &app,
            "POST",
            &denied_phone_path,
            Some("owner"),
            Some(json!({"approved":false})),
        )
        .await;
        assert_eq!(denied_retry_status, StatusCode::CONFLICT);
        let denied_command_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_cli_commands
             WHERE session_id = $1 AND kind = 'approval_answer' AND approval_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(denied_action_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(denied_command_count, 1, "only the winning phone denial is queued");
        let denied_decision: Option<bool> = sqlx::query_scalar(
            "SELECT approved FROM remote_cli_output_actions
             WHERE session_id = $1 AND action_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(Uuid::parse_str(denied_action_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(denied_decision, Some(false));
        let denied_claim_path = format!("{path}/{denied_action_id}/claim");
        let (denied_claim_status, _) = json_req(
            &app,
            "POST",
            &denied_claim_path,
            Some("owner"),
            Some(json!({
                "call_id":denied_call,
                "action_digest":denied_action["data"]["action_digest"]
            })),
        )
        .await;
        assert_eq!(denied_claim_status, StatusCode::CONFLICT);

        let parallel_call = Uuid::now_v7();
        test_tools.clear_pending(denied_call);
        assert!(test_tools.set_pending(cortex_api::domain::models::tools::PendingTool {
            call_id: parallel_call,
            name: cortex_api::domain::models::tools::ToolName::RunCommand,
            arguments: command_arguments.clone(),
        }));
        let (parallel_create_status, parallel_action) = json_req(
            &app,
            "POST",
            &path,
            Some("owner"),
            Some(json!({"call_id":parallel_call})),
        )
        .await;
        assert_eq!(parallel_create_status, StatusCode::CREATED);
        let parallel_action_id = parallel_action["data"]["action_id"].as_str().unwrap();
        let parallel_phone_path =
            format!("/remote/sessions/{first_session}/approvals/{parallel_action_id}");
        let parallel_local_path = format!("{path}/{parallel_action_id}/answer");
        let phone_answer = json_req(
            &app,
            "POST",
            &parallel_phone_path,
            Some("owner"),
            Some(json!({"approved":false})),
        );
        let local_answer = json_req(
            &app,
            "POST",
            &parallel_local_path,
            Some("owner"),
            Some(json!({"approved":true})),
        );
        let ((phone_status, _), (local_status, _)) = tokio::join!(phone_answer, local_answer);
        assert_eq!(
            [phone_status, local_status]
                .iter()
                .filter(|status| **status == StatusCode::OK)
                .count(),
            1,
            "exactly one concurrent authority answer wins"
        );
        assert_eq!(
            [phone_status, local_status]
                .iter()
                .filter(|status| **status == StatusCode::CONFLICT)
                .count(),
            1,
            "the losing concurrent answer is rejected"
        );
        let parallel_decision: Option<bool> = sqlx::query_scalar(
            "SELECT approved FROM remote_cli_output_actions
             WHERE session_id = $1 AND action_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(Uuid::parse_str(parallel_action_id).unwrap())
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(parallel_decision, Some(local_status == StatusCode::OK));
        let parallel_command_count: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_cli_commands
             WHERE session_id = $1 AND kind = 'approval_answer' AND approval_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(parallel_action_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(parallel_command_count, 1);
        let parallel_command_decision: bool = sqlx::query_scalar(
            "SELECT approved FROM remote_cli_commands
             WHERE session_id = $1 AND kind = 'approval_answer' AND approval_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(parallel_action_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(parallel_command_decision, parallel_decision.unwrap());

        let expired_call = Uuid::now_v7();
        test_tools.clear_pending(parallel_call);
        assert!(test_tools.set_pending(cortex_api::domain::models::tools::PendingTool {
            call_id: expired_call,
            name: cortex_api::domain::models::tools::ToolName::RunCommand,
            arguments: command_arguments.clone(),
        }));
        let (expired_create_status, expired_action) = json_req(
            &app,
            "POST",
            &path,
            Some("owner"),
            Some(json!({"call_id":expired_call})),
        )
        .await;
        assert_eq!(expired_create_status, StatusCode::CREATED);
        let expired_action_id = expired_action["data"]["action_id"].as_str().unwrap();
        sqlx::query(
            "UPDATE remote_cli_output_actions SET expires_at = now() - interval '1 second'
             WHERE session_id = $1 AND action_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(Uuid::parse_str(expired_action_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
        let expired_phone_path =
            format!("/remote/sessions/{first_session}/approvals/{expired_action_id}");
        let (expired_status, _) = json_req(
            &app,
            "POST",
            &expired_phone_path,
            Some("owner"),
            Some(json!({"approved":true})),
        )
        .await;
        assert_eq!(expired_status, StatusCode::CONFLICT);
        let expired_local_path = format!("{path}/{expired_action_id}/answer");
        let (expired_local_status, _) = json_req(
            &app,
            "POST",
            &expired_local_path,
            Some("owner"),
            Some(json!({"approved":true})),
        )
        .await;
        assert_eq!(expired_local_status, StatusCode::CONFLICT);
        let expired_commands: i64 = sqlx::query_scalar(
            "SELECT COUNT(*) FROM remote_cli_commands
             WHERE session_id = $1 AND kind = 'approval_answer' AND approval_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(expired_action_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(expired_commands, 0, "expired decisions publish nothing");

        let cancelled_call = Uuid::now_v7();
        test_tools.clear_pending(expired_call);
        assert!(test_tools.set_pending(cortex_api::domain::models::tools::PendingTool {
            call_id: cancelled_call,
            name: cortex_api::domain::models::tools::ToolName::RunCommand,
            arguments: command_arguments,
        }));
        let (cancelled_create_status, cancelled_action) = json_req(
            &app,
            "POST",
            &path,
            Some("owner"),
            Some(json!({"call_id":cancelled_call})),
        )
        .await;
        assert_eq!(cancelled_create_status, StatusCode::CREATED);
        let cancelled_action_id = cancelled_action["data"]["action_id"].as_str().unwrap();
        sqlx::query(
            "UPDATE remote_cli_output_actions SET cancelled_at = now()
             WHERE session_id = $1 AND action_id = $2",
        )
        .bind(Uuid::parse_str(&first_session).unwrap())
        .bind(Uuid::parse_str(cancelled_action_id).unwrap())
        .execute(&pool)
        .await
        .unwrap();
        let cancelled_phone_path =
            format!("/remote/sessions/{first_session}/approvals/{cancelled_action_id}");
        let (cancelled_status, _) = json_req(
            &app,
            "POST",
            &cancelled_phone_path,
            Some("owner"),
            Some(json!({"approved":true})),
        )
        .await;
        assert_eq!(cancelled_status, StatusCode::CONFLICT);
        let wrong_owner_path =
            format!("/remote/sessions/{first_session}/approvals/{cancelled_action_id}");
        let (wrong_owner_status, _) = json_req(
            &app,
            "POST",
            &wrong_owner_path,
            Some("other-owner"),
            Some(json!({"approved":true})),
        )
        .await;
        assert_eq!(wrong_owner_status, StatusCode::FORBIDDEN);
    }
    .await;
    pool.close().await;
    sqlx::query(&format!("DROP SCHEMA {schema} CASCADE"))
        .execute(&admin)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "requires the explicitly approved cortex_api_test database"]
async fn bound_child_parent_cancel_waits_for_real_chat_runtime_provider_stop() {
    dotenvy::dotenv().ok();
    let admin = database::connect(Target::Test).await.unwrap();
    let schema = format!("test_bound_child_cancel_{}", Uuid::now_v7().simple());
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

    let cache = RedisCache::new(None, Duration::from_millis(25));
    let config = Config::default();
    let remote_cli = RemoteCliService::new(
        Arc::new(PostgresRemoteCliStore::new(pool.clone())),
        cache.clone(),
        &config,
    );
    let remote_session: Uuid = remote_cli
        .register("owner", "test cli", "linux", 1_000)
        .await
        .unwrap()
        .session_id
        .parse()
        .unwrap();
    let (_base_service, store) =
        cortex_api::domain::services::agent_runs::AgentRunService::in_memory_with_store(60_000);
    store.register_session(remote_session, "owner").await;
    let (created_tx, mut created_rx) = mpsc::unbounded_channel();
    let (bind_started_tx, mut bind_started_rx) = mpsc::unbounded_channel();
    let (bind_completed_tx, mut bind_completed_rx) = mpsc::unbounded_channel();
    let run_store = Arc::new(LifecycleRunStore {
        inner: store.clone(),
        created: created_tx,
        pause_create: std::sync::atomic::AtomicBool::new(false),
        release_create: tokio::sync::Semaphore::new(0),
        bind_started: bind_started_tx,
        bind_completed: bind_completed_tx,
        pause_bind: std::sync::atomic::AtomicBool::new(false),
        release_bind: tokio::sync::Semaphore::new(0),
    });
    let agent_runs = cortex_api::domain::services::agent_runs::AgentRunService::new(
        run_store.clone(),
        60_000,
        4,
        2,
        8192,
    );
    let (started_tx, mut started_rx) = mpsc::unbounded_channel();
    let cancel_seen = Arc::new(tokio::sync::Notify::new());
    let release = Arc::new(tokio::sync::Notify::new());
    let driver: Arc<dyn ModelDriver> = Arc::new(PausedCancelDriver {
        started: started_tx,
        cancel_seen: cancel_seen.clone(),
        release: release.clone(),
    });
    let (app, runtime) = build_real_app_router_with_agent_runs_and_driver(
        pool.clone(),
        cache,
        remote_cli,
        agent_runs,
        driver,
    );

    let parent_before_registration = parent_fixture(&pool, &runtime).await;
    run_store
        .pause_create
        .store(true, std::sync::atomic::Ordering::Release);
    let create_path = format!(
        "/remote/sessions/{remote_session}/outputs/{}/{}/children",
        parent_before_registration.chat, parent_before_registration.output
    );
    let app_clone = app.clone();
    let call = parent_before_registration.call;
    let create_before_registration = tokio::spawn(async move {
        json_req(
            &app_clone,
            "POST",
            &create_path,
            Some("owner"),
            Some(json!({"call_id":call})),
        )
        .await
    });
    let child_before_registration =
        tokio::time::timeout(Duration::from_secs(10), created_rx.recv())
            .await
            .unwrap()
            .unwrap();
    let (cancel_status, _) = json_req(
        &app,
        "POST",
        &format!("/remote/sessions/{remote_session}/children/{child_before_registration}/cancel"),
        Some("owner"),
        Some(json!({})),
    )
    .await;
    assert_eq!(cancel_status, StatusCode::ACCEPTED);
    run_store.release_create.add_permits(1);
    assert_eq!(
        create_before_registration.await.unwrap().0,
        StatusCode::CREATED
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (child, _) = store
                .events_after("owner", remote_session, child_before_registration, 0, 10)
                .await
                .unwrap();
            if child.status.terminal() {
                assert_eq!(
                    child.status,
                    cortex_api::domain::services::agent_runs::ChildRunStatus::Cancelled
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), started_rx.recv())
            .await
            .is_err()
    );

    let parent_before_task = parent_fixture(&pool, &runtime).await;
    run_store
        .pause_create
        .store(true, std::sync::atomic::Ordering::Release);
    let create_path = format!(
        "/remote/sessions/{remote_session}/outputs/{}/{}/children",
        parent_before_task.chat, parent_before_task.output
    );
    let app_clone = app.clone();
    let call = parent_before_task.call;
    let create_before_task = tokio::spawn(async move {
        json_req(
            &app_clone,
            "POST",
            &create_path,
            Some("owner"),
            Some(json!({"call_id":call})),
        )
        .await
    });
    let child_before_task = tokio::time::timeout(Duration::from_secs(10), created_rx.recv())
        .await
        .unwrap()
        .unwrap();
    runtime
        .controls
        .get(parent_before_task.output)
        .expect("parent output control is registered")
        .request_cancel();
    run_store.release_create.add_permits(1);
    assert_eq!(create_before_task.await.unwrap().0, StatusCode::CREATED);
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (child, _) = store
                .events_after("owner", remote_session, child_before_task, 0, 10)
                .await
                .unwrap();
            if child.status.terminal() {
                assert_eq!(
                    child.status,
                    cortex_api::domain::services::agent_runs::ChildRunStatus::Cancelled
                );
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), started_rx.recv())
            .await
            .is_err()
    );

    let mut parent_during_binding = parent_fixture(&pool, &runtime).await;
    run_store
        .pause_bind
        .store(true, std::sync::atomic::Ordering::Release);
    let create_path = format!(
        "/remote/sessions/{remote_session}/outputs/{}/{}/children",
        parent_during_binding.chat, parent_during_binding.output
    );
    let app_clone = app.clone();
    let call = parent_during_binding.call;
    let create_child = tokio::spawn(async move {
        json_req(
            &app_clone,
            "POST",
            &create_path,
            Some("owner"),
            Some(json!({"call_id":call})),
        )
        .await
    });
    let provider_output = tokio::time::timeout(Duration::from_secs(10), bind_started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    let child_output = tokio::time::timeout(Duration::from_secs(10), started_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(provider_output, child_output);
    runtime
        .controls
        .get(parent_during_binding.output)
        .expect("parent output control is registered")
        .request_cancel();
    run_store.release_bind.add_permits(1);
    let (status, body) = create_child.await.unwrap();
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let child_id: Uuid = body["data"]["child_id"].as_str().unwrap().parse().unwrap();
    let bound_output = tokio::time::timeout(Duration::from_secs(10), bind_completed_rx.recv())
        .await
        .expect("child output binding should finish")
        .expect("binding completion signal should be sent");
    assert_eq!(bound_output, provider_output);

    assert!(runtime.controls.get(provider_output).is_some());
    let (stored, _) = store
        .events_after("owner", remote_session, child_id, 0, 10)
        .await
        .unwrap();
    assert_eq!(stored.child_output_id, Some(provider_output));

    tokio::time::timeout(Duration::from_secs(10), cancel_seen.notified())
        .await
        .unwrap();
    let (still_running, _) = store
        .events_after("owner", remote_session, child_id, 0, 10)
        .await
        .unwrap();
    assert!(still_running.cancel_requested);
    assert!(!still_running.status.terminal());
    assert!(parent_during_binding.results.try_recv().is_err());

    release.notify_one();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let (child, _) = store
                .events_after("owner", remote_session, child_id, 0, 10)
                .await
                .unwrap();
            if child.status.terminal() {
                break child;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let (cancelled, _) = store
        .events_after("owner", remote_session, child_id, 0, 10)
        .await
        .unwrap();
    assert_eq!(
        cancelled.status,
        cortex_api::domain::services::agent_runs::ChildRunStatus::Cancelled
    );
    assert!(cancelled.budget_settled);
    assert!(parent_during_binding.results.try_recv().is_err());

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
