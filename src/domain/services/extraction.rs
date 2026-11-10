use crate::{
    core::{config::Config, process_numbers},
    dal::local::documents,
    domain::{
        models::{events::StepName, extraction::*},
        services::events::EventSink,
    },
};
#[derive(Clone)]
pub struct DocumentExtractor {
    config: Config,
}
impl DocumentExtractor {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
        }
    }
    pub async fn extract(&self, file: DocumentFile, events: &EventSink) -> ExtractedFile {
        let config = self.config.clone();
        let events = events.clone();
        let name = file.name.clone();
        let start = std::time::Instant::now();
        let result = tokio::task::spawn_blocking(move || {
            if file.bytes.len() > config.file_bytes {
                return Err(process_numbers::FILE_SIZE);
            }
            let kind = file
                .content_type
                .split(';')
                .next()
                .unwrap_or_default()
                .trim()
                .to_ascii_lowercase();
            match kind.as_str() {
                kind if kind.starts_with("text/") => {
                    documents::progress(&events, StepName::FileReadPage, &file.name, 1, 1);
                    let text = std::str::from_utf8(&file.bytes)
                        .map_err(|_| process_numbers::FILE_EXTRACTION)?;
                    Ok(vec![TextPage {
                        number: 1,
                        text: text.strip_prefix('\u{feff}').unwrap_or(text).into(),
                        ocr: false,
                    }])
                }
                "application/pdf" => documents::pdf(&config, &file.name, &file.bytes, &events),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                    documents::progress(&events, StepName::FileReadPage, &file.name, 1, 1);
                    documents::docx(&file.bytes)
                }
                _ => Err(process_numbers::FILE_TYPE),
            }
        })
        .await
        .unwrap_or(Err(process_numbers::FILE_EXTRACTION));
        let (pages, error) = match result {
            Ok(pages) => (pages, None),
            Err(error) => (vec![], Some(error)),
        };
        let time_ms = start.elapsed().as_millis() as u64;
        tracing::info!(
            time_ms,
            error_number = error.map(|e| e.number),
            "File extraction finished"
        );
        ExtractedFile {
            name,
            pages,
            error,
            time_ms,
        }
    }
    pub async fn extract_all(
        &self,
        files: Vec<DocumentFile>,
        events: &EventSink,
    ) -> Vec<ExtractedFile> {
        let mut results = vec![];
        for file in files {
            results.push(self.extract(file, events).await);
        }
        results
    }
}
