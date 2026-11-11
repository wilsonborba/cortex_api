use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::{local::whisper, remote::ollama::Ollama},
    domain::{
        models::{
            events::{NativeEvent, StepName, StepProgress},
            identity::Caller,
            pipeline::StageName,
            usage::{Pending, QueueError, Usage},
        },
        services::{balancer::Scheduler, events::EventSink},
    },
};
use std::sync::Arc;
/// Audio file to text. Local now (7.10.8); #80 adds the Remote worker.
pub trait AudioTranscriber: Send + Sync {
    fn transcribe<'a>(
        &'a self,
        caller: &'a Caller,
        name: &'a str,
        bytes: Arc<Vec<u8>>,
        events: &'a EventSink,
    ) -> Pending<'a, Result<String, ProcessNumber>>;
}
/// whisper-rs on the MSI GPU, inside the single model queue (7.5).
pub struct LocalTranscriber {
    pub queue: Scheduler,
    pub ollama: Arc<Ollama>,
    pub config: Config,
}
fn step(name: &str, done: f64, total: f64) -> NativeEvent {
    let mut step = StepProgress::new(StageName::Gather, StepName::Transcribe);
    step.params.insert("file".into(), name.into());
    step.params
        .insert("position_s".into(), (done.round() as u64).into());
    step.params
        .insert("duration_s".into(), (total.round() as u64).into());
    step.progress = Some([done.round() as u64, total.round() as u64]);
    NativeEvent::Step(step)
}
impl AudioTranscriber for LocalTranscriber {
    fn transcribe<'a>(
        &'a self,
        caller: &'a Caller,
        name: &'a str,
        bytes: Arc<Vec<u8>>,
        events: &'a EventSink,
    ) -> Pending<'a, Result<String, ProcessNumber>> {
        Box::pin(async move {
            let config = self.config.clone();
            let samples = tokio::task::spawn_blocking(move || whisper::decode(&config, &bytes))
                .await
                .map_err(|_| process_numbers::AUDIO_FAILED)??;
            let total = whisper::seconds(&samples);
            if total > self.config.audio_max_seconds {
                return Err(process_numbers::AUDIO_LIMIT);
            }
            // Never at the same time as an LLM call: one queue, LLM models unloaded.
            let permit = self
                .queue
                .acquire(caller.balancer_identity(), uuid::Uuid::now_v7())
                .await
                .map_err(|e| match e {
                    QueueError::HeavyLoad => process_numbers::HEAVY_LOAD,
                    QueueError::ServerFault => process_numbers::QUEUE_FAULT,
                })?;
            self.ollama
                .release_models_for_local_job(&permit)
                .await
                .map_err(|_| process_numbers::OLLAMA_FAILED)?;
            events.send(step(name, 0.0, total));
            let (tx, rx) = std::sync::mpsc::channel();
            let config = self.config.clone();
            // whisper-rs keeps the progress callback (and its sender) after the job,
            // so the forwarder stops on this flag, not on a closed channel.
            let done = Arc::new(std::sync::atomic::AtomicBool::new(false));
            let finished = done.clone();
            let job = tokio::task::spawn_blocking(move || {
                let text = whisper::transcribe(&config, &samples, tx);
                finished.store(true, std::sync::atomic::Ordering::SeqCst);
                text
            });
            let sink = events.clone();
            let owned = name.to_owned();
            let forward = tokio::task::spawn_blocking(move || {
                use std::sync::mpsc::RecvTimeoutError;
                loop {
                    match rx.recv_timeout(std::time::Duration::from_millis(100)) {
                        Ok(percent) => {
                            sink.send(step(&owned, total * f64::from(percent) / 100.0, total))
                        }
                        Err(RecvTimeoutError::Timeout)
                            if !done.load(std::sync::atomic::Ordering::SeqCst) => {}
                        Err(_) => break,
                    }
                }
            });
            let text = job.await.map_err(|_| process_numbers::AUDIO_FAILED)?;
            let _ = forward.await;
            let _ = permit
                .finish(
                    Usage {
                        audio: total.ceil() as u64,
                        ..Default::default()
                    },
                    chrono::Utc::now(),
                )
                .await;
            tracing::info!(audio_s = total, ok = text.is_ok(), "Transcription finished");
            text
        })
    }
}
/// Transcription on the file worker (7.10.3). It does not use the MSI GPU
/// queue, but it still counts as a usage item (7.10.6).
pub struct RemoteTranscriber {
    pub worker: crate::dal::remote::file_worker::FileWorker,
    pub queue: Scheduler,
}
impl AudioTranscriber for RemoteTranscriber {
    fn transcribe<'a>(
        &'a self,
        caller: &'a Caller,
        name: &'a str,
        bytes: Arc<Vec<u8>>,
        events: &'a EventSink,
    ) -> Pending<'a, Result<String, ProcessNumber>> {
        Box::pin(async move {
            events.send(step(name, 0.0, 0.0));
            let answer = self
                .worker
                .run(
                    "transcribe",
                    name,
                    "application/octet-stream",
                    bytes.to_vec(),
                )
                .await?;
            events.send(step(name, answer.audio_s, answer.audio_s));
            let _ = self
                .queue
                .record_remote(
                    caller.balancer_identity(),
                    uuid::Uuid::now_v7(),
                    Usage {
                        audio: answer.audio_s.ceil() as u64,
                        ..Default::default()
                    },
                    chrono::Utc::now(),
                )
                .await;
            Ok(answer
                .pages
                .into_iter()
                .map(|p| p.text)
                .collect::<Vec<_>>()
                .join("\n"))
        })
    }
}
