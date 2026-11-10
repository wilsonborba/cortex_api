use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    domain::models::extraction::TextPage,
};
use serde::Deserialize;
/// HTTP client of the file worker binary (7.10.3). The worker answers in the
/// Cortex `{message, data}` structure.
#[derive(Clone)]
pub struct FileWorker {
    client: reqwest::Client,
    base: String,
}
#[derive(Deserialize)]
struct Page {
    number: u32,
    text: String,
    ocr: bool,
}
#[derive(Deserialize)]
struct Data {
    #[serde(default)]
    pages: Vec<Page>,
    #[serde(default)]
    audio_s: f64,
    error_number: Option<u16>,
}
#[derive(Deserialize)]
struct Answer {
    data: Data,
}
pub struct WorkerText {
    pub pages: Vec<TextPage>,
    pub audio_s: f64,
}
impl FileWorker {
    pub fn new(config: &Config, base: &str) -> Self {
        Self {
            client: reqwest::Client::builder()
                .timeout(config.worker_timeout)
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("file worker client"),
            base: base.trim_end_matches('/').to_owned(),
        }
    }
    /// `job` is extract, ocr or transcribe. A known worker error number is kept;
    /// any other failure is FILE_WORKER_FAILED (7.10.7).
    pub async fn run(
        &self,
        job: &str,
        name: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<WorkerText, ProcessNumber> {
        let mut url = reqwest::Url::parse(&format!("{}/jobs/{job}", self.base))
            .map_err(|_| process_numbers::FILE_WORKER_FAILED)?;
        url.query_pairs_mut().append_pair("name", name);
        let answer: Answer = self
            .client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, content_type)
            .body(bytes)
            .send()
            .await
            .map_err(|_| process_numbers::FILE_WORKER_FAILED)?
            .json()
            .await
            .map_err(|_| process_numbers::FILE_WORKER_FAILED)?;
        if let Some(number) = answer.data.error_number {
            return Err(process_numbers::REGISTRY
                .iter()
                .copied()
                .find(|p| p.number == number)
                .unwrap_or(process_numbers::FILE_WORKER_FAILED));
        }
        Ok(WorkerText {
            pages: answer
                .data
                .pages
                .into_iter()
                .map(|p| TextPage {
                    number: p.number,
                    text: p.text,
                    ocr: p.ocr,
                })
                .collect(),
            audio_s: answer.data.audio_s,
        })
    }
}
