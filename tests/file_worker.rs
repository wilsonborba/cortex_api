use axum::{Json, Router, extract::State, routing::post};
use cortex_api::{
    core::{
        config::{Config, JobLocation},
        process_numbers,
    },
    dal::remote::{file_worker::FileWorker, ollama::Ollama},
    domain::{
        models::{
            driver::{CallInput, ModelDriver, OutputModel},
            extraction::DocumentFile,
            identity::Caller,
            usage::{
                CallUsage, LedgerUnavailable, Pending, Share, Usage, UsageLedger, WaitingCall,
            },
        },
        services::{
            balancer::Scheduler,
            events::channel,
            extraction::DocumentExtractor,
            transcription::{AudioTranscriber, LocalTranscriber, RemoteTranscriber},
        },
    },
    presentation::{file_worker::router, with_response_contract},
};
use serde_json::{Value, json};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

/// Start the worker binary router in the test (7.10.4) and return its URL.
async fn worker() -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = with_response_contract(router(Config::default()));
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    url
}
fn file(name: &str, content_type: &str) -> DocumentFile {
    DocumentFile {
        name: name.into(),
        content_type: content_type.into(),
        bytes: Arc::new(std::fs::read(format!("tests/fixtures/worker/{name}")).unwrap()),
    }
}
async fn text(config: &Config, f: DocumentFile) -> (String, Option<u16>) {
    let (events, _, _) = channel(config, None);
    let out = DocumentExtractor::new(config).extract(f, &events).await;
    (out.text(), out.error.map(|e| e.number))
}
#[tokio::test]
async fn text_and_docx_give_the_same_text_local_and_remote() {
    let url = worker().await;
    let local = Config::default();
    let remote = Config {
        extract_location: JobLocation::Remote(url),
        ..Default::default()
    };
    let docx = "application/vnd.openxmlformats-officedocument.wordprocessingml.document";
    for (name, kind) in [("facts.txt", "text/plain"), ("tables.docx", docx)] {
        let a = text(&local, file(name, kind)).await;
        let b = text(&remote, file(name, kind)).await;
        assert!(a.0.contains("HERON-42"), "{name}");
        assert_eq!(a, b, "{name}");
    }
    // A worker error keeps its own number; the file fails, nothing else stops.
    let a = text(&local, file("facts.txt", "application/x-unknown")).await;
    let b = text(&remote, file("facts.txt", "application/x-unknown")).await;
    assert_eq!(a.1, Some(process_numbers::FILE_TYPE.number));
    assert_eq!(a, b);
}
#[tokio::test]
async fn worker_down_is_a_file_error_number() {
    let config = Config {
        extract_location: JobLocation::Remote("http://127.0.0.1:1".into()),
        ..Default::default()
    };
    let (out, error) = text(&config, file("facts.txt", "text/plain")).await;
    assert!(out.is_empty());
    assert_eq!(error, Some(process_numbers::FILE_WORKER_FAILED.number));
}
#[tokio::test]
#[ignore = "requires the pinned local PDFium build and Tesseract eng+por+tha"]
async fn pdf_and_ocr_give_the_same_text_local_and_remote() {
    let url = worker().await;
    let local = Config::default();
    for remote in [
        Config {
            extract_location: JobLocation::Remote(url.clone()),
            ..Default::default()
        },
        // Only the OCR pages go to the worker.
        Config {
            ocr_location: JobLocation::Remote(url.clone()),
            ..Default::default()
        },
    ] {
        for name in ["text.pdf", "scanned.pdf"] {
            let a = text(&local, file(name, "application/pdf")).await;
            let b = text(&remote, file(name, "application/pdf")).await;
            assert_eq!(a.1, None, "{name}");
            assert!(!a.0.trim().is_empty(), "{name}");
            assert_eq!(a, b, "{name}");
        }
    }
    let scanned = text(&local, file("scanned.pdf", "application/pdf")).await.0;
    assert!(scanned.contains("HERON"), "OCR text: {scanned}");
}
#[derive(Default)]
struct Ledger {
    records: Mutex<Vec<CallUsage>>,
}
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
            })
        })
    }
    fn record<'a>(&'a self, call: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>> {
        self.records.lock().unwrap().push(call);
        Box::pin(async { Ok(()) })
    }
    fn waiting<'a>(&'a self, _: Vec<WaitingCall>) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async { Ok(()) })
    }
}
fn caller() -> Caller {
    Caller {
        app_id: "cortex".into(),
        user_id: Some("bench-user".into()),
        is_admin: false,
    }
}
#[tokio::test]
#[ignore = "GPU: whisper large-v3-turbo on the MSI; unloads the Ollama models first; owner GPU window"]
async fn transcription_gives_the_same_text_local_and_remote() {
    let url = worker().await;
    let config = Config::default();
    let ledger = Arc::new(Ledger::default());
    let queue = Scheduler::new(ledger.clone());
    let local = LocalTranscriber {
        queue: queue.clone(),
        ollama: Arc::new(Ollama::new(config.clone(), queue.clone())),
        config: config.clone(),
    };
    let remote = RemoteTranscriber {
        worker: FileWorker::new(&config, &url),
        queue,
    };
    let bytes = Arc::new(std::fs::read("tests/fixtures/audio.opus").unwrap());
    let (events, _, _) = channel(&config, None);
    let a = local
        .transcribe(&caller(), "audio.opus", bytes.clone(), &events)
        .await
        .unwrap();
    let b = remote
        .transcribe(&caller(), "audio.opus", bytes, &events)
        .await
        .unwrap();
    assert!(!a.is_empty());
    assert_eq!(a, b);
    // Both count as usage: the local one through the queue, the remote one without it.
    let records = ledger.records.lock().unwrap();
    assert_eq!(records.len(), 2);
    assert!(records.iter().all(|r| r.usage.audio == 7));
}
#[tokio::test]
async fn remote_vision_uses_the_other_ollama_url_and_still_counts() {
    // Ollama double for the remote vision URL: one streamed answer frame.
    let seen: Arc<Mutex<Vec<Value>>> = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route(
            "/api/chat",
            post(|State(seen): State<Arc<Mutex<Vec<Value>>>>, Json(body): Json<Value>| async move {
                seen.lock().unwrap().push(body);
                format!("{}\n", json!({"message":{"content":"a red apple"},"done":true,"prompt_eval_count":9,"eval_count":3}))
            }),
        )
        .with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    // The local Ollama URL points to a closed port: a local call would fail.
    let config = Config {
        ollama_url: "http://127.0.0.1:1".into(),
        vision_location: JobLocation::Remote(url),
        ..Default::default()
    };
    let ledger = Arc::new(Ledger::default());
    let driver = Ollama::new(config.clone(), Scheduler::new(ledger.clone()));
    let model = OutputModel::new(&caller(), uuid::Uuid::now_v7(), true);
    let (tx, _rx) = mpsc::unbounded_channel();
    let (_c, cancel) = watch::channel(false);
    let input = CallInput {
        role: "answer".into(),
        system: String::new(),
        prompt: "What is in the image?".into(),
        images: vec!["aW1hZ2U=".into()],
        temperature: 0.0,
        max_tokens: 64,
    };
    let outcome = driver.call(&model, input, tx, cancel).await.unwrap();
    assert_eq!(outcome.text, "a red apple");
    assert_eq!(outcome.error, None);
    let body = seen.lock().unwrap()[0].clone();
    assert_eq!(body["model"], config.vision_model);
    assert_eq!(body["messages"][0]["images"][0], "aW1hZ2U=");
    let records = ledger.records.lock().unwrap();
    assert_eq!(records.len(), 1);
    assert_eq!(records[0].usage.images, 1);
}
