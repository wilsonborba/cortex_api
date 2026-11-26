use crate::core::{
    config::Config,
    process_numbers::{self, ProcessNumber},
};
use std::{io::Write, process::Stdio, sync::mpsc};
use whisper_rs::{FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters};

pub const RATE: usize = 16_000;

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

/// Decode only the selected, ordered audio intervals into the format consumed
/// by Whisper. FFmpeg trims the stream before writing samples to memory.
pub fn decode_intervals(
    config: &Config,
    bytes: &[u8],
    intervals: &[(u64, u64)],
) -> Result<Vec<f32>, ProcessNumber> {
    if intervals.is_empty() || intervals.len() > 100 {
        return Err(process_numbers::INVALID_DATA);
    }
    let duration = probe_duration_seconds(config, bytes)?;
    let mut filters = Vec::with_capacity(intervals.len() + 1);
    let mut previous_end = 0u64;
    for (index, (start, end)) in intervals.iter().copied().enumerate() {
        if start >= end || start < previous_end || end as f64 / 1000.0 > duration {
            return Err(process_numbers::INVALID_DATA);
        }
        previous_end = end;
        filters.push(format!(
            "[0:a]atrim=start={:.3}:end={:.3},asetpts=PTS-STARTPTS[a{index}]",
            start as f64 / 1000.0,
            end as f64 / 1000.0
        ));
    }
    let inputs = (0..intervals.len())
        .map(|index| format!("[a{index}]"))
        .collect::<String>();
    filters.push(format!("{inputs}concat=n={}:v=0:a=1[out]", intervals.len()));
    let filter = filters.join(";");
    let mut child = std::process::Command::new(&config.ffmpeg_path)
        .args([
            "-v",
            "error",
            "-i",
            "pipe:0",
            "-filter_complex",
            &filter,
            "-map",
            "[out]",
            "-f",
            "f32le",
            "-ac",
            "1",
            "-ar",
            "16000",
            "pipe:1",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let mut stdin = child.stdin.take().ok_or(process_numbers::AUDIO_FAILED)?;
    let input = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let _ = writer.join();
    if !output.status.success() || output.stdout.len() % 4 != 0 {
        return Err(process_numbers::AUDIO_FAILED);
    }
    let samples = output
        .stdout
        .chunks_exact(4)
        .map(|b| f32::from_le_bytes([b[0], b[1], b[2], b[3]]))
        .collect::<Vec<_>>();
    if samples.is_empty() {
        return Err(process_numbers::INVALID_DATA);
    }
    Ok(samples)
}

fn probe_duration_seconds(config: &Config, bytes: &[u8]) -> Result<f64, ProcessNumber> {
    let mut executable = config.ffmpeg_path.clone();
    executable.set_file_name("ffprobe");
    let mut child = std::process::Command::new(executable)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
            "pipe:0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let mut stdin = child.stdin.take().ok_or(process_numbers::AUDIO_FAILED)?;
    let input = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let output = child
        .wait_with_output()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let _ = writer.join();
    if !output.status.success() {
        return Err(process_numbers::AUDIO_FAILED);
    }
    let duration = std::str::from_utf8(&output.stdout)
        .ok()
        .and_then(|value| value.trim().parse::<f64>().ok())
        .filter(|value| value.is_finite() && *value > 0.0);
    match duration {
        Some(duration) => Ok(duration),
        None => probe_last_packet_end_seconds(config, bytes),
    }
}

fn probe_last_packet_end_seconds(config: &Config, bytes: &[u8]) -> Result<f64, ProcessNumber> {
    let mut executable = config.ffmpeg_path.clone();
    executable.set_file_name("ffprobe");
    let mut child = std::process::Command::new(executable)
        .args([
            "-v",
            "error",
            "-select_streams",
            "a:0",
            "-show_packets",
            "-show_entries",
            "packet=pts_time,duration_time",
            "-of",
            "csv=p=0",
            "pipe:0",
        ])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|_| process_numbers::AUDIO_FAILED)?;
    let mut stdin = child.stdin.take().ok_or(process_numbers::AUDIO_FAILED)?;
    let input = bytes.to_vec();
    let writer = std::thread::spawn(move || stdin.write_all(&input));
    let stdout = child.stdout.take().ok_or(process_numbers::AUDIO_FAILED)?;
    let reader = std::thread::spawn(move || {
        use std::io::{BufRead, BufReader};
        let mut end = None;
        for line in BufReader::new(stdout)
            .lines()
            .map_while(std::result::Result::ok)
        {
            let mut fields = line.split(',');
            let pts = fields.next().and_then(|value| value.parse::<f64>().ok());
            let duration = fields.next().and_then(|value| value.parse::<f64>().ok());
            if let (Some(pts), Some(duration)) = (pts, duration)
                && pts.is_finite()
                && duration.is_finite()
                && duration > 0.0
            {
                end = Some(pts + duration);
            }
        }
        end
    });
    let status = child.wait().map_err(|_| process_numbers::AUDIO_FAILED)?;
    let _ = writer.join();
    if !status.success() {
        return Err(process_numbers::AUDIO_FAILED);
    }
    reader
        .join()
        .map_err(|_| process_numbers::AUDIO_FAILED)?
        .filter(|end| end.is_finite() && *end > 0.0)
        .ok_or(process_numbers::AUDIO_FAILED)
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
    let parameters = WhisperContextParameters {
        use_gpu: true,
        ..Default::default()
    };
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
