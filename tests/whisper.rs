use cortex_api::{
    core::config::Config,
    dal::{
        local::whisper,
        remote::{
            model_router::ModelRouter, ollama::Ollama, prism::Prism,
            prism_supervisor::PrismSupervisor,
        },
    },
    domain::{
        models::{
            driver::{CallInput, ModelDriver, OutputModel},
            events::{NativeEvent, StepName},
            identity::Caller,
            model_profile::ModelProfile,
            usage::{CallUsage, LedgerUnavailable, Pending, Share, Usage, WaitingCall},
        },
        services::{
            balancer::Scheduler,
            events::channel,
            transcription::{AudioTranscriber, LocalTranscriber},
        },
    },
};
use futures_util::StreamExt;
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};
#[derive(Default)]
struct Ledger {
    records: Mutex<Vec<CallUsage>>,
}
impl cortex_api::domain::models::usage::UsageLedger for Ledger {
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
    fn record<'a>(&'a self, call: CallUsage) -> Pending<'a, Result<(), LedgerUnavailable>> {
        self.records.lock().unwrap().push(call);
        Box::pin(async { Ok(()) })
    }
    fn waiting<'a>(&'a self, _: Vec<WaitingCall>) -> Pending<'a, Result<(), LedgerUnavailable>> {
        Box::pin(async { Ok(()) })
    }
}
#[test]
fn ffmpeg_decodes_opus_and_mp3_to_16khz_mono() {
    let config = Config::default();
    for (name, seconds) in [("audio.opus", 6.1), ("audio.mp3", 22.8)] {
        let bytes = std::fs::read(format!("tests/fixtures/{name}")).unwrap();
        let samples = whisper::decode(&config, &bytes).unwrap();
        assert!((whisper::seconds(&samples) - seconds).abs() < 0.2, "{name}");
    }
    assert!(whisper::decode(&config, b"not audio").is_err());
}
#[tokio::test]
#[ignore = "GPU: loads large-v3-turbo on the MSI and unloads the Ollama models first; owner GPU window"]
async fn transcribes_opus_and_mp3_on_the_gpu_in_the_model_queue() {
    let config = Config::default();
    let ledger = Arc::new(Ledger::default());
    let queue = Scheduler::new(ledger.clone());
    let transcriber = LocalTranscriber {
        queue: queue.clone(),
        ollama: Arc::new(Ollama::new(config.clone(), queue)),
        config: config.clone(),
    };
    let caller = Caller {
        app_id: "cortex".into(),
        user_id: Some("bench-user".into()),
        is_admin: false,
    };
    let mut rows = vec![];
    for (name, seconds) in [("audio.opus", 6.110542), ("audio.mp3", 22.848)] {
        let (events, mut rx, _task) = channel(&config, None);
        let bytes = Arc::new(std::fs::read(format!("tests/fixtures/{name}")).unwrap());
        let start = std::time::Instant::now();
        let text = transcriber
            .transcribe(&caller, name, bytes, &events)
            .await
            .unwrap();
        let wall = start.elapsed().as_secs_f64();
        drop(events);
        let mut steps = 0;
        while let Some(event) = rx.recv().await {
            if event.event.name() == "step" {
                steps += 1;
            }
        }
        assert!(!text.trim().is_empty(), "{name}");
        assert!(steps >= 1, "{name}: progress steps");
        println!(
            "RESULT {name} audio_s={seconds:.1} wall_s={wall:.2} realtime_factor={:.3} steps={steps} chars={}",
            wall / seconds,
            text.len()
        );
        rows.push(wall);
    }
    let recorded = ledger.records.lock().unwrap();
    assert_eq!(recorded.len(), 2);
    assert_eq!(recorded[0].usage.audio, 7);
    assert_eq!(recorded[1].usage.audio, 23);
}

async fn ollama_loaded(config: &Config) -> Result<Vec<String>, String> {
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .get(format!(
            "{}/api/ps",
            config.ollama_url.trim_end_matches('/')
        ))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("Ollama /api/ps status {}", response.status()));
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|error| error.to_string())?;
        if bytes.len().saturating_add(chunk.len()) > 64 * 1024 {
            return Err("Ollama /api/ps exceeded the 64 KiB test bound".into());
        }
        bytes.extend_from_slice(&chunk);
    }
    let value: serde_json::Value =
        serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
    Ok(value["models"]
        .as_array()
        .ok_or("Ollama /api/ps models missing")?
        .iter()
        .filter_map(|model| model["name"].as_str().map(str::to_owned))
        .collect())
}

async fn unload_test_t0(config: &Config) -> Result<(), String> {
    let loaded = ollama_loaded(config).await?;
    if loaded.is_empty() {
        return Ok(());
    }
    if loaded.len() != 1 || loaded[0] != config.text_model {
        return Err(format!(
            "preserving unexpected Ollama models during Whisper cleanup: {loaded:?}"
        ));
    }
    let client = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(std::time::Duration::from_secs(3))
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|error| error.to_string())?;
    let response = client
        .post(format!(
            "{}/api/generate",
            config.ollama_url.trim_end_matches('/')
        ))
        .json(&serde_json::json!({"model":config.text_model,"keep_alive":0,"stream":false,"prompt":""}))
        .send()
        .await
        .map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("test-owned T0 unload status {}", response.status()));
    }
    for _ in 0..30 {
        if ollama_loaded(config).await?.is_empty() {
            return Ok(());
        }
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    }
    Err("test-owned T0 did not unload within 30 seconds".into())
}

#[tokio::test]
#[ignore = "requires a separate explicit GPU authorization; real Bonsai to Whisper transition"]
async fn transitions_bonsai_to_whisper_and_recovers_t0_on_the_shared_queue() {
    use futures_util::FutureExt;

    let available = std::net::TcpListener::bind("127.0.0.1:0").expect("free loopback port");
    let port = available.local_addr().unwrap().port();
    drop(available);
    let config = Config {
        bonsai_enabled: true,
        prism_server_path: std::env::var("CORTEX_TEST_PRISM_SERVER")
            .expect("runner pins CORTEX_TEST_PRISM_SERVER")
            .into(),
        bonsai_model_directory: std::env::var("CORTEX_TEST_BONSAI_DIR")
            .expect("runner pins CORTEX_TEST_BONSAI_DIR")
            .into(),
        ollama_url: std::env::var("CORTEX_TEST_OLLAMA_URL")
            .expect("runner pins CORTEX_TEST_OLLAMA_URL"),
        prism_url: format!("http://127.0.0.1:{port}"),
        whisper_model_path: std::env::var("CORTEX_TEST_WHISPER_MODEL")
            .expect("runner pins CORTEX_TEST_WHISPER_MODEL")
            .into(),
        ..Config::default()
    };
    assert!(config.prism_url_is_loopback());
    assert!(
        config.whisper_model_path.is_file(),
        "reuse the installed Whisper artifact"
    );
    assert!(
        ollama_loaded(&config)
            .await
            .expect("Ollama preflight")
            .is_empty(),
        "do not disturb pre-existing Ollama models"
    );

    let ledger = Arc::new(Ledger::default());
    let queue = Scheduler::new(ledger.clone());
    let supervisor = Arc::new(PrismSupervisor::new());
    let ollama = Arc::new(
        Ollama::new(config.clone(), queue.clone()).with_prism_supervisor(supervisor.clone()),
    );
    let prism = Arc::new(Prism::managed(
        config.clone(),
        queue.clone(),
        supervisor.clone(),
    ));
    let router = Arc::new(ModelRouter::new(ollama.clone(), prism, queue.clone()));
    let transcriber = LocalTranscriber {
        queue: queue.clone(),
        ollama,
        config: config.clone(),
    };
    let caller = Caller {
        app_id: "bonsai-eval".into(),
        user_id: Some("local-eval".into()),
        is_admin: false,
    };
    let mut owned_t0 = false;
    let result = std::panic::AssertUnwindSafe(async {
        let profile = ModelProfile::Bonsai25Uncensored;
        let t1_output = OutputModel::for_profile(&caller, uuid::Uuid::now_v7(), profile, false);
        let (t1_events, _t1_rx) = mpsc::unbounded_channel();
        let (_cancel_tx, cancel) = watch::channel(false);
        let t1_input = CallInput {
            role: "answer".into(),
            system: "Answer accurately in one concise sentence.".into(),
            prompt: "Define Rust ownership in one sentence.".into(),
            images: vec![],
            temperature: 0.0,
            max_tokens: 128,
            context_tokens: 8192,
            prefix: None,
            allow_tools: false,
            tool_history: vec![],
        };
        let t1_result = tokio::time::timeout(
            std::time::Duration::from_secs(300),
            router.call(&t1_output, t1_input, t1_events, cancel),
        )
        .await
        .expect("bounded real T1 answer")
        .expect("T1 provider answer");
        assert!(t1_result.error.is_none(), "T1 error: {:?}", t1_result.error);
        assert!(!t1_result.text.trim().is_empty());
        assert!(
            supervisor
                .is_running(profile)
                .await
                .expect("owned Prism status"),
            "T1 child is resident before the Whisper transition"
        );
        let (events, mut event_rx, event_task) = channel(&config, None);
        let bytes =
            Arc::new(std::fs::read("tests/fixtures/audio.opus").expect("existing Opus fixture"));
        let transcription = transcriber.transcribe(&caller, "audio.opus", bytes, &events);
        let text = tokio::time::timeout(std::time::Duration::from_secs(300), transcription)
            .await
            .expect("bounded Whisper transcription")
            .expect("existing Opus fixture transcribes");
        drop(events);
        let mut saw_transcribe_step = false;
        tokio::time::timeout(std::time::Duration::from_secs(30), async {
            while let Some(event) = event_rx.recv().await {
                if let NativeEvent::Step(step) = event.event
                    && step.step == StepName::Transcribe
                {
                    assert!(
                        !supervisor
                            .is_running(profile)
                            .await
                            .expect("Prism status during Whisper"),
                        "Prism must be reaped before Whisper progress starts"
                    );
                    saw_transcribe_step = true;
                }
            }
        })
        .await
        .expect("bounded Whisper progress drain");
        tokio::time::timeout(std::time::Duration::from_secs(5), event_task)
            .await
            .expect("bounded Whisper event task completion")
            .expect("Whisper event task");
        assert!(
            saw_transcribe_step,
            "Whisper emits progress after Prism teardown"
        );
        assert!(!text.trim().is_empty());
        assert!(
            !supervisor
                .is_running(profile)
                .await
                .expect("Prism status after Whisper"),
            "Prism remains unloaded after local transcription"
        );

        owned_t0 = true;
        let t0_output =
            OutputModel::for_profile(&caller, uuid::Uuid::now_v7(), ModelProfile::CortexT0, false);
        let (t0_events, _t0_rx) = mpsc::unbounded_channel();
        let (_t0_cancel_tx, t0_cancel) = watch::channel(false);
        let t0_result = tokio::time::timeout(
            std::time::Duration::from_secs(240),
            router.call(
                &t0_output,
                CallInput {
                    role: "answer".into(),
                    system: "Answer concisely.".into(),
                    prompt: "Reply with the word RECOVERED.".into(),
                    images: vec![],
                    temperature: 0.0,
                    max_tokens: 32,
                    context_tokens: 8192,
                    prefix: None,
                    allow_tools: false,
                    tool_history: vec![],
                },
                t0_events,
                t0_cancel,
            ),
        )
        .await
        .expect("bounded T0 recovery")
        .expect("T0 recovery answer");
        assert!(t0_result.error.is_none(), "T0 error: {:?}", t0_result.error);
        assert!(!t0_result.text.trim().is_empty());
        assert!(t0_result.text.to_lowercase().contains("recovered"));

        let recorded = ledger.records.lock().unwrap();
        let audio: Vec<_> = recorded
            .iter()
            .filter(|record| record.usage.audio > 0)
            .collect();
        assert_eq!(audio.len(), 1);
        assert_eq!(audio[0].usage.audio, 7);
        assert_eq!(recorded.len(), 3, "T1, audio, and T0 usage are recorded");
    })
    .catch_unwind()
    .await;

    let stop_result = supervisor.stop().await;
    let cleanup = async {
        if owned_t0 {
            unload_test_t0(&config).await?;
        }
        let remaining = ollama_loaded(&config).await?;
        if !remaining.is_empty() {
            return Err(format!(
                "preserving unexpected Ollama models: {remaining:?}"
            ));
        }
        Ok::<(), String>(())
    }
    .await;
    stop_result.expect("stop only the owned Prism child");
    cleanup.expect("verify test-owned Ollama cleanup");
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}
