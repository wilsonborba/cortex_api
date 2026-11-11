//! Issue 85: fixed retrieval fixtures, real Cortex chat router, CPU shield and MSI Ollama.
//! No production data or PostgreSQL connection is used. Run explicitly with --ignored.
use axum::{
    Json, Router,
    body::{Body, to_bytes},
    http::Request,
    routing::{get, post, put},
};
use cortex_api::{
    core::config::{Config, ContextFormat},
    dal::{
        local::{chats::Chats, injection_classifier::OnnxInjectionClassifier},
        remote::{
            cache::RedisCache, event_journal::EventJournal, hippocampus::Hippocampus,
            ollama::Ollama, runs::RunStore, timing::TimingStore, usage::RedisLedger,
            web_page::PageReader,
        },
    },
    domain::{
        models::{driver::*, search::*, usage::Pending},
        services::{
            balancer::Scheduler, chat_runtime::ChatRuntime, crawler::WebRetriever,
            exchange_memory::ExchangeMemories, extraction::DocumentExtractor, files::FileProxy,
            history::History, recall::MemoryRecall, run_evidence::RunEvidence, search::WebSearch,
            security::SecurityShield, telemetry_reads::TelemetryReads, temporary::TemporaryChats,
            thinking::Thinking, transcription::LocalTranscriber,
        },
    },
    presentation::{chat_inputs::router, with_response_contract},
};
use serde_json::{Value, json};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, watch};
use tower::ServiceExt;

struct Child(std::process::Child);
impl Drop for Child {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
struct FixedSearch(String);
impl SearchAdapter for FixedSearch {
    fn name(&self) -> &'static str {
        "fixed experiment page"
    }
    fn search<'a>(&'a self, _: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>> {
        Box::pin(async {
            Ok(vec![SearchHit {
                title: "Festival facts".into(),
                url: self.0.clone(),
            }])
        })
    }
}
struct MeasuredDriver {
    inner: Arc<Ollama>,
    calls: Mutex<Vec<Value>>,
}
impl ModelDriver for MeasuredDriver {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        Box::pin(async move {
            assert_eq!(input.temperature, 0.0);
            let prompt = input.prompt.clone();
            let result = self.inner.call(output, input, events, cancel).await?;
            assert!(result.error.is_none(), "real model call failed");
            self.calls.lock().unwrap().push(json!({
                "prompt":prompt,"usage":result.usage,"tokens_per_second":result.tokens_per_second,
                "load_ms":result.load_ms,
            }));
            Ok(result)
        })
    }
}
async fn fixtures() -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/api/v1/search", post(|Json(body): Json<Value>| async move {
            let facts = [
                ("aster", "note", "Project Aster", "Project Aster release code is LILAC-47. Its review month is October."),
                ("boreal", "note", "Project Boreal", "Project Boreal release code is COPPER-82. Its review month is April."),
                ("warehouse", "file", "warehouse.txt", "The Lumen warehouse pickup bay is D6. The Nacre warehouse pickup bay is F2."),
                ("invoice", "file", "invoice.txt", "The Nacre invoice total is 273 credits. The Lumen invoice total is 148 credits."),
            ];
            Json(json!({"data":facts.iter().map(|(id,kind,title,text)| json!({"memory":{
                "id":id,"workspace_id":"cortex","app_id":body["app_id"],"user_id":body["user_id"],
                "memory_type":kind,"title":title,"content":text,
            }})).collect::<Vec<_>>()}))
        }))
        .route("/api/v1/entities", get(|| async { Json(json!({"data":[]})) }))
        .route("/api/v1/entities/match", post(|| async { Json(json!({"data":[]})) }))
        .route("/festival", get(|| async { axum::response::Html("<html><head><title>Festival facts</title></head><body><main><h1>Festival guide</h1><p>The Oriole festival meeting room is Juniper. The Heron festival meeting room is Cedar. These are separate events. This reference lists only the meeting rooms. Travel arrangements, transport costs, meals, accommodation, attendance counts and meeting dates are not included in this guide.</p></main></body></html>") }))
        .route("/runs/_find", post(|| async { Json(json!({"docs":[]})) }))
        .route("/runs/{id}", get(|| async { axum::http::StatusCode::NOT_FOUND })
            .merge(put(|| async { (axum::http::StatusCode::CREATED, Json(json!({"ok":true}))) })));
    let task = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    (address, task)
}
fn runtime(
    config: &Config,
    address: &str,
    cache: RedisCache,
    shield: Arc<SecurityShield>,
) -> (Arc<ChatRuntime>, Arc<MeasuredDriver>) {
    let hippo = Hippocampus::new(config).unwrap();
    // Temporary requests never use this lazy, unreachable pool.
    let chats = Chats {
        pool: sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://nobody@127.0.0.1:1/none")
            .unwrap(),
    };
    let store = Arc::new(
        RunStore::new(
            config,
            format!("{address}/runs").parse().unwrap(),
            "fixture".into(),
            "fixture".into(),
        )
        .unwrap(),
    );
    let ledger = Arc::new(RedisLedger::new(
        cache.clone(),
        store.clone(),
        "format-experiment".into(),
    ));
    let queue = Scheduler::new(ledger);
    let ollama = Arc::new(Ollama::new(config.clone(), queue.clone()));
    let driver = Arc::new(MeasuredDriver {
        inner: ollama.clone(),
        calls: Default::default(),
    });
    let temporary = TemporaryChats::new(config, cache.clone());
    let personas = cortex_api::domain::services::personas::Personas {
        hippocampus: hippo.clone(),
        shield: shield.clone(),
    };
    let rt = Arc::new(ChatRuntime {
        history: History::new(chats.clone(), cache.clone(), config).unwrap(),
        files: FileProxy {
            hippocampus: hippo.clone(),
            extractor: DocumentExtractor::new(config),
            transcriber: Arc::new(LocalTranscriber {
                queue,
                ollama,
                config: config.clone(),
            }),
            temporary: temporary.clone(),
            config: config.clone(),
        },
        temporary,
        recall: MemoryRecall::new(config, hippo.clone(), shield.clone()),
        web: WebRetriever::new(
            config,
            WebSearch::with_adapters(
                config,
                vec![Arc::new(FixedSearch(format!("{address}/festival")))],
            ),
            PageReader::new(config).unwrap(),
            shield.clone(),
            driver.clone(),
        ),
        thinking: Thinking::new(config, driver.clone()),
        memories: ExchangeMemories {
            repo: chats.clone(),
            hippocampus: hippo,
        },
        runs: RunEvidence::new(store.clone()),
        telemetry: TelemetryReads {
            store,
            timing: TimingStore::new(config, cache.clone()),
            config: config.clone(),
        },
        journal: EventJournal::new(cache, config).unwrap(),
        controls: Default::default(),
        personas,
        shield,
        chats,
        config: config.clone(),
    });
    (rt, driver)
}
fn normalized(text: &str) -> String {
    text.trim()
        .trim_matches(|c: char| c.is_ascii_punctuation() || c.is_whitespace())
        .to_uppercase()
}
#[tokio::test]
#[ignore = "GPU: real MSI Ollama, real CPU classifier, isolated local Valkey; owner-approved window"]
async fn compare_context_formats_through_real_chat_pipeline() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let child = std::process::Command::new(".scratch/valkey/usr/bin/valkey-server")
        .args([
            "--bind",
            "127.0.0.1",
            "--port",
            &port.to_string(),
            "--save",
            "",
            "--appendonly",
            "no",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let _child = Child(child);
    tokio::time::sleep(Duration::from_millis(150)).await;
    let redis_url = format!("redis://127.0.0.1:{port}/0");
    let cache = RedisCache::new(Some(&redis_url), Duration::from_secs(2));
    let (address, fixture_task) = fixtures().await;
    let classifier = Arc::new(OnnxInjectionClassifier::load(&Config::default()).unwrap());
    let shield = Arc::new(SecurityShield::new(&Config::default(), classifier).unwrap());
    let formats = [
        ("json", ContextFormat::Json),
        ("toon", ContextFormat::Toon),
        ("plain", ContextFormat::Plain),
    ];
    let runtimes: Vec<_> = formats
        .iter()
        .map(|(_, format)| {
            let config = Config {
                context_format: *format,
                hippocampus_url: address.clone(),
                query_temperature: 0.0,
                answer_token_reserve: 128,
                ..Default::default()
            };
            runtime(&config, &address, cache.clone(), shield.clone())
        })
        .collect();
    let cases = [
        (
            "memory_known",
            "What is the release code of Project Aster?",
            "LILAC-47",
        ),
        (
            "file_known",
            "What is the pickup bay for the Lumen warehouse?",
            "D6",
        ),
        (
            "web_known",
            "What is the meeting room for the Oriole festival?",
            "JUNIPER",
        ),
        (
            "memory_absent",
            "Who manages Project Aster?",
            "NOT IN CONTEXT",
        ),
        (
            "file_absent",
            "What is the opening hour of the Lumen warehouse?",
            "NOT IN CONTEXT",
        ),
        (
            "web_absent",
            "What is the ticket price for the Oriole festival?",
            "NOT IN CONTEXT",
        ),
    ];
    let mut rows = vec![];
    std::fs::create_dir_all("docs").unwrap();
    // Rotate format order across cases and repetitions. Each format has one excluded warmup.
    for repetition in 0..3 {
        for (case_index, (name, question, expected)) in cases.iter().enumerate() {
            for offset in 0..3 {
                let index = (case_index + repetition + offset) % 3;
                let (rt, driver) = &runtimes[index];
                let warmup = repetition == 0 && case_index == 0;
                for trial in 0..if warmup { 2 } else { 1 } {
                    let is_warmup = warmup && trial == 0;
                    let body = json!({"app_id":"cortex","text":format!("{question} Reply with only the requested value. If it is absent from the provided texts, reply exactly NOT IN CONTEXT."),"temporary":true,"use_memory":true,"needs_web":true,"stream":false});
                    let req = Request::post("/chats")
                        .header("content-type", "application/json")
                        .header("x-uuid", "format-experiment-user")
                        .extension(axum::extract::ConnectInfo(
                            "203.0.113.1:1234".parse::<std::net::SocketAddr>().unwrap(),
                        ))
                        .body(Body::from(body.to_string()))
                        .unwrap();
                    let start = Instant::now();
                    let response = with_response_contract(router(rt.clone()))
                        .oneshot(req)
                        .await
                        .unwrap();
                    let status = response.status();
                    let value: Value = serde_json::from_slice(
                        &to_bytes(response.into_body(), 1024 * 1024).await.unwrap(),
                    )
                    .unwrap();
                    let total_ms = start.elapsed().as_secs_f64() * 1000.0;
                    assert!(
                        status.is_success(),
                        "pipeline rejected synthetic request: {value}"
                    );
                    assert!(
                        value["data"]["error_number"].is_null(),
                        "pipeline error: {value}"
                    );
                    let text = value["data"]["text"].as_str().unwrap();
                    let call = driver
                        .calls
                        .lock()
                        .unwrap()
                        .pop()
                        .expect("real model must be called");
                    // Evidence that all fixed source classes survived Gather and Assemble.
                    let prompt = call["prompt"].as_str().unwrap();
                    for fact in [
                        "LILAC-47",
                        "COPPER-82",
                        "pickup bay",
                        "festival meeting room",
                    ] {
                        assert!(
                            prompt.contains(fact),
                            "source missing from assembled prompt: {fact}"
                        );
                    }
                    let answer = normalized(text);
                    let correct = answer == *expected;
                    let refused = answer == "NOT IN CONTEXT";
                    rows.push(
                        json!({"format":formats[index].0,"case":name,"repetition":repetition,
                        "warmup":is_warmup,"question":question,"expected":expected,"answer":text,
                        "correct":correct,"refused":refused,"unsupported_answer":!correct&&!refused,
                        "pipeline_total_ms":total_ms,"call":call}),
                    );
                    std::fs::write(
                        "docs/context-format-raw.json",
                        serde_json::to_vec_pretty(&rows).unwrap(),
                    )
                    .unwrap();
                    println!(
                        "format={} case={} repetition={} warmup={} correct={} total_ms={:.1}",
                        formats[index].0, name, repetition, is_warmup, correct, total_ms
                    );
                }
            }
        }
    }
    fixture_task.abort();
    assert_eq!(rows.iter().filter(|r| r["warmup"] == false).count(), 54);
}
