use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{get, post},
};
use cortex_api::{
    core::config::Config,
    dal::remote::ollama::Ollama,
    domain::{
        models::{driver::*, identity::Caller, usage::*},
        services::balancer::Scheduler,
    },
};
use serde_json::{Value, json};
use std::{
    convert::Infallible,
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::sync::{mpsc, watch};
#[derive(Clone, Default)]
struct Double {
    loaded: Arc<Mutex<Option<String>>>,
    operations: Arc<Mutex<Vec<String>>>,
    bodies: Arc<Mutex<Vec<Value>>>,
    fail: Arc<AtomicBool>,
    trace: Arc<AtomicBool>,
    reject_probabilities: Arc<AtomicBool>,
    reject_after_text: Arc<AtomicBool>,
    reject_stream: Arc<AtomicBool>,
    stall: Arc<AtomicBool>,
    active: Arc<AtomicUsize>,
    maximum: Arc<AtomicUsize>,
}
struct Busy(Arc<AtomicUsize>);
impl Drop for Busy {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn ps(State(s): State<Double>) -> Json<Value> {
    Json(
        json!({"models":s.loaded.lock().unwrap().as_ref().map(|name|vec![json!({"name":name,"context_length":8192})]).unwrap_or_default()}),
    )
}
async fn generate(State(s): State<Double>, Json(body): Json<Value>) -> Json<Value> {
    let model = body["model"].as_str().unwrap();
    if body["keep_alive"] == 0 {
        s.operations.lock().unwrap().push(format!("unload:{model}"));
        *s.loaded.lock().unwrap() = None;
    } else {
        assert_eq!(body["options"]["num_ctx"], 8192);
        s.operations.lock().unwrap().push(format!("load:{model}"));
        *s.loaded.lock().unwrap() = Some(model.into());
    }
    Json(json!({"done":true}))
}
async fn chat(State(s): State<Double>, Json(body): Json<Value>) -> Response {
    s.bodies.lock().unwrap().push(body.clone());
    if s.reject_probabilities.load(Ordering::SeqCst) && body.get("logprobs").is_some() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({"error":"logprobs not supported"})),
        )
            .into_response();
    }
    if s.fail.load(Ordering::SeqCst) {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    if s.reject_stream.load(Ordering::SeqCst) && body.get("logprobs").is_some() {
        return Response::builder()
            .header("content-type", "application/x-ndjson")
            .body(Body::from("{\"error\":\"logprobs not supported\"}\n"))
            .unwrap();
    }
    assert_eq!(body["options"]["num_ctx"], 8192);
    assert_eq!(body["options"]["num_predict"], 32);
    assert_eq!(body["options"]["temperature"], 0.0);
    assert!(body["stream"].as_bool().unwrap());
    assert_eq!(
        Some(body["model"].as_str().unwrap().to_owned()),
        *s.loaded.lock().unwrap()
    );
    let active = s.active.fetch_add(1, Ordering::SeqCst) + 1;
    s.maximum.fetch_max(active, Ordering::SeqCst);
    let guard = Busy(s.active.clone());
    let mut first_value = json!({"message":{"content":"A💡"},"done":false});
    let mut second_value = json!({"message":{"content":" B"},"done":false});
    if s.trace.load(Ordering::SeqCst) {
        first_value["logprobs"] = json!([{"token":"A💡","bytes":"A💡".as_bytes(),"logprob":-0.2,"top_logprobs":[{"token":"X","logprob":-2.0}]}]);
        second_value["logprobs"] =
            json!([{"token":" B","logprob":-0.5,"top_logprobs":[{"token":" C","logprob":-1.5}]}]);
    }
    if s.reject_after_text.load(Ordering::SeqCst) {
        second_value = json!({"error":"logprobs not supported"});
    }
    let first = serde_json::to_vec(&first_value).unwrap();
    let mut second = serde_json::to_vec(&second_value).unwrap();
    second.push(b'\n');
    let split = first.iter().position(|b| *b == 0xf0).unwrap() + 1;
    let mut rest = first[split..].to_vec();
    rest.push(b'\n');
    let frames = Arc::new(vec![
        first[..split].to_vec(),
        rest,
        second,
        b"{\"done\":true,\"prompt_eval_count\":11,\"eval_count\":3,\"eval_duration\":100000000}\n"
            .to_vec(),
    ]);
    let stall = s.stall.load(Ordering::SeqCst);
    let stream = futures_util::stream::unfold((0, guard), move |(index, guard)| {
        let frames = frames.clone();
        async move {
            if index >= frames.len() {
                return None;
            }
            if index > 0 {
                tokio::time::sleep(Duration::from_millis(15)).await;
            }
            if stall && index == 2 {
                std::future::pending::<()>().await;
            }
            Some((
                Ok::<_, Infallible>(Bytes::from(frames[index].clone())),
                (index + 1, guard),
            ))
        }
    });
    Response::builder()
        .header("content-type", "application/x-ndjson")
        .body(Body::from_stream(stream))
        .unwrap()
}
#[derive(Default)]
struct Ledger(Mutex<Vec<CallUsage>>);
impl UsageLedger for Ledger {
    fn share<'a>(
        &'a self,
        _: &'a str,
        _: chrono::DateTime<chrono::Utc>,
    ) -> Pending<'a, Result<Share, LedgerUnavailable>> {
        Box::pin(async {
            Ok(Share {
                usage: Usage::default(),
                weight: 1.0,
                loyalty: None,
            })
        })
    }
    fn record<'a>(&'a self, c: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async move {
            self.0.lock().unwrap().push(c);
            Ok(())
        })
    }
    fn waiting<'a>(&'a self, _: Vec<WaitingCall>) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async { Ok(()) })
    }
}
fn input(images: bool, role: &str) -> CallInput {
    CallInput {
        role: role.into(),
        system: "Answer briefly".into(),
        prompt: "Fixture".into(),
        images: if images {
            vec!["aW1hZ2U=".into()]
        } else {
            vec![]
        },
        temperature: 0.0,
        max_tokens: 32,
        prefix: None,
    }
}
async fn setup() -> (
    Ollama,
    Scheduler,
    Arc<Ledger>,
    Double,
    tokio::task::JoinHandle<()>,
) {
    let state = Double::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = state.clone();
    let task = tokio::spawn(async move {
        axum::serve(
            listener,
            Router::new()
                .route("/api/ps", get(ps))
                .route("/api/generate", post(generate))
                .route("/api/chat", post(chat))
                .with_state(server),
        )
        .await
        .unwrap()
    });
    let config = Config {
        ollama_url: format!("http://{address}"),
        ..Config::default()
    };
    let ledger = Arc::new(Ledger::default());
    let scheduler = Scheduler::new(ledger.clone());
    (
        Ollama::new(config, scheduler.clone()),
        scheduler,
        ledger,
        state,
        task,
    )
}
fn output(images: bool) -> OutputModel {
    OutputModel::new(
        &Caller {
            app_id: "cortex".into(),
            user_id: Some("fixture-user".into()),
            is_admin: false,
        },
        uuid::Uuid::new_v4(),
        images,
    )
}
async fn run(driver: &Ollama, out: &OutputModel, images: bool, role: &str) -> CallOutcome {
    let (tx, _) = mpsc::unbounded_channel();
    let (_cancel, rx) = watch::channel(false);
    driver.call(out, input(images, role), tx, rx).await.unwrap()
}
#[tokio::test]
async fn text_vision_switch_and_one_model_for_all_output_roles() {
    let (driver, scheduler, ledger, state, task) = setup().await;
    let config = Config::default();
    let text = output(false);
    let result = run(&driver, &text, false, "answer").await;
    assert!(result.error.is_none());
    assert_eq!(result.text, "A💡 B");
    assert_eq!(result.usage.input_tokens, 11);
    assert_eq!(result.usage.output_tokens, 3);
    assert_eq!(result.tokens_per_second, Some(30.0));
    let vision = output(true);
    let result = run(&driver, &vision, true, "answer").await;
    assert!(result.error.is_none());
    assert_eq!(result.usage.model, config.vision_model);
    assert_eq!(result.usage.images, Some(1));
    let result = run(&driver, &vision, false, "plan").await;
    assert!(result.error.is_none());
    assert_eq!(result.usage.model, config.vision_model);
    run(&driver, &text, false, "answer").await;
    assert_eq!(
        *state.operations.lock().unwrap(),
        vec![
            format!("load:{}", config.text_model),
            format!("unload:{}", config.text_model),
            format!("load:{}", config.vision_model),
            format!("unload:{}", config.vision_model),
            format!("load:{}", config.text_model)
        ]
    );
    assert_eq!(state.maximum.load(Ordering::SeqCst), 1);
    assert_eq!(ledger.0.lock().unwrap().len(), 4);
    let permit = scheduler
        .acquire("fixture-user".into(), uuid::Uuid::new_v4())
        .await
        .unwrap();
    driver.release_models_for_local_job(&permit).await.unwrap();
    drop(permit);
    assert!(state.loaded.lock().unwrap().is_none());
    task.abort();
}
#[tokio::test]
async fn token_arrives_before_done_and_cancel_releases_the_queue() {
    let (driver, _, ledger, state, task) = setup().await;
    state.stall.store(true, Ordering::SeqCst);
    let out = output(false);
    let (tx, mut rx) = mpsc::unbounded_channel();
    let (cancel, watch) = watch::channel(false);
    let d = driver.clone();
    let call = tokio::spawn(async move { d.call(&out, input(false, "answer"), tx, watch).await });
    loop {
        if let DriverEvent::Token { text, .. } =
            tokio::time::timeout(Duration::from_secs(1), rx.recv())
                .await
                .unwrap()
                .unwrap()
        {
            assert_eq!(text, "A💡");
            break;
        }
    }
    assert!(!call.is_finished());
    cancel.send(true).unwrap();
    let result = call.await.unwrap().unwrap();
    assert_eq!(result.error, Some(DriverError::Cancelled));
    assert_eq!(result.text, "A💡");
    assert!(!ledger.0.lock().unwrap().is_empty());
    assert!(cortex_api::presentation::model_error::number(DriverError::Cancelled).is_none());
    state.stall.store(false, Ordering::SeqCst);
    let result = run(&driver, &output(false), false, "answer").await;
    assert!(result.error.is_none());
    task.abort();
}
#[tokio::test]
async fn model_fault_has_a_number_and_does_not_hold_the_slot() {
    let (driver, _, _, state, task) = setup().await;
    state.fail.store(true, Ordering::SeqCst);
    let result = run(&driver, &output(false), false, "answer").await;
    assert_eq!(result.error, Some(DriverError::Ollama));
    assert_eq!(
        cortex_api::presentation::model_error::number(DriverError::Ollama)
            .unwrap()
            .number,
        1012
    );
    state.fail.store(false, Ordering::SeqCst);
    assert!(
        run(&driver, &output(false), false, "answer")
            .await
            .error
            .is_none()
    );
    task.abort();
}

#[tokio::test]
async fn captures_only_answer_probabilities_without_an_extra_call() {
    let (driver, _, _, state, task) = setup().await;
    state.trace.store(true, Ordering::SeqCst);
    let outcome = run(&driver, &output(false), false, "answer").await;
    let trace = outcome.inspection.unwrap();
    assert!(trace.reason.is_none());
    assert_eq!(trace.tokens.len(), 2);
    assert_eq!(trace.tokens[1].start_byte, "A💡".len());
    assert_eq!(state.bodies.lock().unwrap().len(), 1);
    assert_eq!(state.bodies.lock().unwrap()[0]["top_logprobs"], 5);
    let plan = run(&driver, &output(false), false, "plan").await;
    assert!(plan.inspection.is_none());
    assert!(state.bodies.lock().unwrap()[1].get("logprobs").is_none());
    task.abort();
}
#[tokio::test]
async fn unsupported_options_retry_once_before_text_only() {
    let (driver, _, _, state, task) = setup().await;
    state.reject_probabilities.store(true, Ordering::SeqCst);
    let outcome = run(&driver, &output(false), false, "answer").await;
    assert!(outcome.error.is_none());
    assert_eq!(outcome.text, "A💡 B");
    assert_eq!(
        outcome.inspection.unwrap().reason.as_deref(),
        Some("not_supported")
    );
    assert_eq!(state.bodies.lock().unwrap().len(), 2);
    assert!(state.bodies.lock().unwrap()[1].get("logprobs").is_none());
    task.abort();
}
#[tokio::test]
async fn never_retries_a_partial_answer() {
    let (driver, _, _, state, task) = setup().await;
    state.reject_after_text.store(true, Ordering::SeqCst);
    let outcome = run(&driver, &output(false), false, "answer").await;
    assert!(outcome.error.is_some());
    assert_eq!(outcome.text, "A💡");
    assert_eq!(state.bodies.lock().unwrap().len(), 1);
    task.abort();
}

#[tokio::test]
async fn stream_option_rejection_retries_before_text() {
    let (driver, _, _, state, task) = setup().await;
    state.reject_stream.store(true, Ordering::SeqCst);
    let outcome = run(&driver, &output(false), false, "answer").await;
    assert!(outcome.error.is_none());
    assert_eq!(outcome.text, "A💡 B");
    assert_eq!(state.bodies.lock().unwrap().len(), 2);
    assert_eq!(
        outcome.inspection.unwrap().reason.as_deref(),
        Some("not_supported")
    );
    task.abort();
}

#[tokio::test]
#[ignore = "Explicit operator model capability check; no database or user data"]
async fn model_probability_capability() {
    assert_eq!(std::env::var("CORTEX_INSPECTION_PROBE").as_deref(), Ok("1"));
    let config = Config::default();
    let scheduler = Scheduler::new(Arc::new(Ledger::default()));
    let driver = Ollama::new(config, scheduler);
    let result = run(&driver, &output(false), false, "answer").await;
    assert!(result.error.is_none());
    let trace = result.inspection.unwrap();
    println!(
        "recorded_probability_tokens={} availability={}",
        trace.tokens.len(),
        trace.reason.as_deref().unwrap_or("available")
    );
}
