use crate::core::{
    config::Config,
    process_numbers::{self, ProcessNumber},
};
use std::{io::Write, process::Stdio, sync::mpsc};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

const RATE: usize = 16_000;

/// Any audio format to 16 kHz mono f32 samples with ffmpeg (7.6.4; reads Opus too).
pub fn decode(config: &Config, bytes: &[u8]) -> Result<Vec<f32>, ProcessNumber> {
    let mut child = std::process::Command::new(&config.ffmpeg_path)
        .args([
            "-v", "error", "-i", "pipe:0", "-f", "f32le", "-ac", "1", "-ar", "16000", "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let mut stdin = child.stdin.take().ok_or(process_numbers::AUDIO_FAILED)?;
    let input = bytes.to_vec();
    // Write in another thread so a full stdout pipe cannot block the write.
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let _ = writer.join();
    if !output.status.success() || output.stdout.len() % 4 != 0 {
        return Err(process_numbers::AUDIO_FAILED);
    }
    Ok(output
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect())
}

pub fn seconds(samples: &[f32]) -> f64 {
    samples.len() as f64 / RATE as f64
}

/// Load the model for this job, transcribe, and drop it again (7.5).
/// `progress` receives whisper.cpp progress in percent.
pub fn transcribe(
    config: &Config,
    samples: &[f32],
    progress: mpsc::Sender<i32>,
) -> Result<String, ProcessNumber> {
    let mut parameters = WhisperContextParameters::default();
    parameters.use_gpu = true;
    let path = config
        .whisper_model_path
        .to_str()
        .ok_or(process_numbers::AUDIO_FAILED)?;
    let context = WhisperContext::new_with_params(path, parameters)
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let mut state = context
        .create_state()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
    params.set_n_threads(config.whisper_threads);
    params.set_language(Some("auto"));
    params.set_print_progress(false);
    params.set_print_realtime(false);
    params.set_print_special(false);
    params.set_print_timestamps(false);
    params.set_progress_callback_safe(move |percent: i32| {
        let _ = progress.send(percent);
    });
    state
        .full(params, samples)
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let mut text = String::new();
    for segment in state.as_iter() {
        if let Ok(part) = segment.to_str_lossy() {
            text.push_str(part.trim());
            text.push(' ');
        }
    }
    Ok(text.trim().to_owned())
}
