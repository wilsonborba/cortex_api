use crate::{
    core::{
        config::{Config, JobLocation},
        process_numbers::{self, ProcessNumber},
    },
    dal::local::{documents, whisper},
    domain::{
        models::extraction::{DocumentFile, TextPage},
        services::{events::channel, extraction::DocumentExtractor},
    },
    presentation::response::{self, Envelope},
};
use axum::{
    Router,
    body::Bytes,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::post,
};
use serde_json::json;
use std::sync::Arc;
/// The file worker (7.10.4): the same job code as Cortex, always Local here.
pub fn router(config: Config) -> Router {
    let config = Config {
        extract_location: JobLocation::Local,
        ocr_location: JobLocation::Local,
        transcribe_location: JobLocation::Local,
        ..config
    };
    let limit = config.file_bytes + config.file_form_overhead;
    Router::new()
        .route("/jobs/{job}", post(job).layer(DefaultBodyLimit::max(limit)))
        .with_state(config)
}
#[derive(serde::Deserialize)]
struct Name {
    name: String,
}
fn pages(pages: Vec<TextPage>, audio_s: f64) -> Response {
    let pages: Vec<_> = pages
        .into_iter()
        .map(|p| json!({"number": p.number, "text": p.text, "ocr": p.ocr}))
        .collect();
    Envelope::new("Job done", json!({"pages": pages, "audio_s": audio_s})).into_response()
}
fn failed(process: ProcessNumber) -> Response {
    response::error(StatusCode::UNPROCESSABLE_ENTITY, process)
}
async fn job(
    State(config): State<Config>,
    Path(job): Path<String>,
    Query(name): Query<Name>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let content_type = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_owned();
    match job.as_str() {
        "extract" => {
            let (events, _, _) = channel(&config, None);
            let file = DocumentFile {
                name: name.name,
                content_type,
                bytes: Arc::new(body.to_vec()),
            };
            let out = DocumentExtractor::new(&config).extract(file, &events).await;
            match out.error {
                Some(process) => failed(process),
                None => pages(out.pages, 0.0),
            }
        }
        "ocr" => {
            let result = tokio::task::spawn_blocking(move || {
                let image = image::load_from_memory(&body)
                    .map_err(|_| process_numbers::FILE_OCR)?
                    .to_luma8();
                documents::ocr_gray(&config, &image)
            })
            .await
            .unwrap_or(Err(process_numbers::FILE_OCR));
            match result {
                Ok(text) => pages(
                    vec![TextPage {
                        number: 1,
                        text,
                        ocr: true,
                    }],
                    0.0,
                ),
                Err(process) => failed(process),
            }
        }
        "transcribe" => {
            let result = tokio::task::spawn_blocking(move || {
                let samples = whisper::decode(&config, &body)?;
                let seconds = whisper::seconds(&samples);
                if seconds > config.audio_max_seconds {
                    return Err(process_numbers::AUDIO_LIMIT);
                }
                let (tx, _rx) = std::sync::mpsc::channel();
                whisper::transcribe(&config, &samples, tx).map(|text| (text, seconds))
            })
            .await
            .unwrap_or(Err(process_numbers::AUDIO_FAILED));
            match result {
                Ok((text, seconds)) => pages(
                    vec![TextPage {
                        number: 1,
                        text,
                        ocr: false,
                    }],
                    seconds,
                ),
                Err(process) => failed(process),
            }
        }
        _ => response::error(StatusCode::NOT_FOUND, process_numbers::NOT_FOUND),
    }
}
