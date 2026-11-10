use cortex_api::{
    core::config::Config,
    dal::{local::whisper, remote::ollama::Ollama},
    domain::{
        models::{
            identity::Caller,
            usage::{CallUsage, LedgerUnavailable, Pending, Share, Usage, WaitingCall},
        },
        services::{
            balancer::Scheduler,
            events::channel,
            transcription::{AudioTranscriber, LocalTranscriber},
        },
    },
};
use std::sync::{Arc, Mutex};
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
