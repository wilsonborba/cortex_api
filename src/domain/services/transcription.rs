use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::{local::whisper, remote::ollama::Ollama},
    domain::{
        models::{
            events::{NativeEvent, StepName, StepProgress},
            extraction::AudioRange,
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
    fn transcribe_selected<'a>(
        &'a self,
        caller: &'a Caller,
        name: &'a str,
        bytes: Arc<Vec<u8>>,
        intervals: Vec<AudioRange>,
        events: &'a EventSink,
    ) -> Pending<'a, Result<String, ProcessNumber>> {
        if intervals.is_empty() {
            self.transcribe(caller, name, bytes, events)
        } else {
            Box::pin(async { Err(process_numbers::INVALID_DATA) })
        }
    }
}
/// whisper-rs on the MSI GPU, inside the single model queue (7.5).
pub struct LocalTranscriber {
    pub queue: Scheduler,
    pub ollama: Arc<Ollama>,
    pub config: Config,
}
fn interval_pairs(intervals: &[AudioRange]) -> Result<(Vec<(u64, u64)>, u64), ProcessNumber> {
    if intervals.is_empty() || intervals.len() > 100 {
        return Err(process_numbers::INVALID_DATA);
    }
    let mut pairs = Vec::with_capacity(intervals.len());
    let mut prior_end = 0;
    let mut duration_ms = 0u64;
    for interval in intervals {
        if interval.start_ms >= interval.end_ms || interval.start_ms < prior_end {
            return Err(process_numbers::INVALID_DATA);
        }
        duration_ms = duration_ms
            .checked_add(interval.end_ms - interval.start_ms)
            .ok_or(process_numbers::INVALID_DATA)?;
        prior_end = interval.end_ms;
        pairs.push((interval.start_ms, interval.end_ms));
    }
    Ok((pairs, duration_ms))
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
impl LocalTranscriber {
    async fn transcribe_samples<'a>(
        &'a self,
        caller: &'a Caller,
        name: &'a str,
        samples: Vec<f32>,
        usage_seconds: f64,
        events: &'a EventSink,
    ) -> Result<String, ProcessNumber> {
        if usage_seconds > self.config.audio_max_seconds {
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
        events.send(step(name, 0.0, usage_seconds));
        let (tx, rx) = std::sync::mpsc::channel();
        let config = self.config.clone();
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
                    Ok(percent) => sink.send(step(
                        &owned,
                        usage_seconds * f64::from(percent) / 100.0,
                        usage_seconds,
                    )),
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
                    audio: usage_seconds.ceil() as u64,
                    ..Default::default()
                },
                chrono::Utc::now(),
            )
            .await;
        tracing::info!(
            audio_s = usage_seconds,
            ok = text.is_ok(),
            "Transcription finished"
        );
        text
    }
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
            self.transcribe_samples(caller, name, samples, total, events)
                .await
        })
    }
    fn transcribe_selected<'a>(
        &'a self,
        caller: &'a Caller,
        name: &'a str,
        bytes: Arc<Vec<u8>>,
        intervals: Vec<AudioRange>,
        events: &'a EventSink,
    ) -> Pending<'a, Result<String, ProcessNumber>> {
        Box::pin(async move {
            let (intervals, selected_ms) = interval_pairs(&intervals)?;
            if selected_ms as f64 / 1000.0 > self.config.audio_max_seconds {
                return Err(process_numbers::AUDIO_LIMIT);
            }
            let config = self.config.clone();
            let samples = tokio::task::spawn_blocking(move || {
                whisper::decode_intervals(&config, &bytes, &intervals)
            })
            .await
            .map_err(|_| process_numbers::AUDIO_FAILED)??;
            let total = whisper::seconds(&samples);
            if total > self.config.audio_max_seconds {
                return Err(process_numbers::AUDIO_LIMIT);
            }
            self.transcribe_samples(caller, name, samples, total, events)
                .await
        })
    }
}
/// Transcription on the file worker (7.10.3). It does not use the MSI GPU
/// queue, but it still counts as a usage item (7.10.6).
pub struct RemoteTranscriber {
    pub worker: crate::dal::remote::file_worker::FileWorker,
    pub queue: Scheduler,
    pub config: Config,
}
fn pcm_wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2).min(u32::MAX as usize - 36) as u32;
    let mut wav = Vec::with_capacity(data_len as usize + 44);
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&(36 + data_len).to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16u32.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&1u16.to_le_bytes());
    wav.extend_from_slice(&(whisper::RATE as u32).to_le_bytes());
    wav.extend_from_slice(&((whisper::RATE * 2) as u32).to_le_bytes());
    wav.extend_from_slice(&2u16.to_le_bytes());
    wav.extend_from_slice(&16u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&data_len.to_le_bytes());
    for sample in samples.iter().take(data_len as usize / 2) {
        let value = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
        wav.extend_from_slice(&value.to_le_bytes());
    }
    wav
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
    fn transcribe_selected<'a>(
        &'a self,
        caller: &'a Caller,
        name: &'a str,
        bytes: Arc<Vec<u8>>,
        intervals: Vec<AudioRange>,
        events: &'a EventSink,
    ) -> Pending<'a, Result<String, ProcessNumber>> {
        Box::pin(async move {
            let (intervals, selected_ms) = interval_pairs(&intervals)?;
            if selected_ms as f64 / 1000.0 > self.config.audio_max_seconds {
                return Err(process_numbers::AUDIO_LIMIT);
            }
            let config = self.config.clone();
            let samples = tokio::task::spawn_blocking(move || {
                whisper::decode_intervals(&config, &bytes, &intervals)
            })
            .await
            .map_err(|_| process_numbers::AUDIO_FAILED)??;
            let selected_seconds = whisper::seconds(&samples);
            if selected_seconds > self.config.audio_max_seconds {
                return Err(process_numbers::AUDIO_LIMIT);
            }
            let selected = pcm_wav(&samples);
            events.send(step(name, 0.0, selected_seconds));
            let answer = self
                .worker
                .run("transcribe", name, "audio/wav", selected)
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

#[cfg(test)]
mod selection_tests {
    use super::*;

    #[test]
    fn selected_audio_intervals_are_ordered_and_bounded() {
        let (pairs, duration) = interval_pairs(&[
            AudioRange {
                start_ms: 1,
                end_ms: 3,
            },
            AudioRange {
                start_ms: 6,
                end_ms: 8,
            },
        ])
        .unwrap();
        assert_eq!(pairs, [(1, 3), (6, 8)]);
        assert_eq!(duration, 4);
        assert!(
            interval_pairs(&[
                AudioRange {
                    start_ms: 5,
                    end_ms: 8
                },
                AudioRange {
                    start_ms: 6,
                    end_ms: 9
                },
            ])
            .is_err()
        );
    }

    #[test]
    fn selected_audio_worker_payload_is_pcm_wav_of_only_the_clipped_samples() {
        let wav = pcm_wav(&[0.0, 0.5, -0.5]);
        assert_eq!(&wav[..4], b"RIFF");
        assert_eq!(&wav[8..12], b"WAVE");
        assert_eq!(&wav[36..40], b"data");
        assert_eq!(u32::from_le_bytes(wav[40..44].try_into().unwrap()), 6);
        assert_eq!(wav.len(), 50);
    }
}
