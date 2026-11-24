use crate::{
    core::{
        config::{Config, JobLocation},
        process_numbers,
    },
    dal::{local::documents, remote::file_worker::FileWorker},
    domain::{
        models::{events::StepName, extraction::*},
        services::events::EventSink,
    },
};
fn finished(
    name: String,
    result: Result<Vec<TextPage>, crate::core::process_numbers::ProcessNumber>,
    start: std::time::Instant,
) -> ExtractedFile {
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
#[derive(Clone)]
pub struct DocumentExtractor {
    config: Config,
    extract_worker: Option<FileWorker>,
    ocr_worker: Option<FileWorker>,
}
fn worker(config: &Config, location: &JobLocation) -> Option<FileWorker> {
    match location {
        JobLocation::Local => None,
        JobLocation::Remote(url) => Some(FileWorker::new(config, url)),
    }
}
impl DocumentExtractor {
    pub fn new(config: &Config) -> Self {
        Self {
            config: config.clone(),
            extract_worker: worker(config, &config.extract_location),
            ocr_worker: worker(config, &config.ocr_location),
        }
    }
    pub async fn extract(&self, file: DocumentFile, events: &EventSink) -> ExtractedFile {
        self.extract_pdf_pages(file, events, None).await
    }
    pub async fn extract_pdf_pages(
        &self,
        file: DocumentFile,
        events: &EventSink,
        selected_pages: Option<Vec<std::ops::RangeInclusive<u32>>>,
    ) -> ExtractedFile {
        let config = self.config.clone();
        let events = events.clone();
        let name = file.name.clone();
        let start = std::time::Instant::now();
        if let Some(worker) = &self.extract_worker {
            if file.bytes.len() > config.file_bytes {
                return finished(name, Err(process_numbers::FILE_SIZE), start);
            }
            let result = worker
                .run(
                    "extract",
                    &file.name,
                    &file.content_type,
                    file.bytes.to_vec(),
                )
                .await
                .map(|t| t.pages)
                .and_then(|mut pages| {
                    if let Some(ranges) = &selected_pages {
                        let total = pages.iter().map(|page| page.number).max().unwrap_or(0);
                        if ranges.iter().any(|range| *range.end() > total) {
                            return Err(process_numbers::INVALID_DATA);
                        }
                        pages
                            .retain(|page| ranges.iter().any(|range| range.contains(&page.number)));
                    }
                    Ok(pages)
                });
            return finished(name, result, start);
        }
        let ocr_worker = self.ocr_worker.clone();
        let selected_pages_for_local = selected_pages.clone();
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
                "application/pdf" => documents::pdf_selected(
                    &config,
                    &file.name,
                    &file.bytes,
                    &events,
                    ocr_worker.as_ref(),
                    selected_pages_for_local.as_deref(),
                ),
                "application/vnd.openxmlformats-officedocument.wordprocessingml.document" => {
                    documents::progress(&events, StepName::FileReadPage, &file.name, 1, 1);
                    documents::docx(&file.bytes)
                }
                _ => Err(process_numbers::FILE_TYPE),
            }
        })
        .await
        .unwrap_or(Err(process_numbers::FILE_EXTRACTION));
        finished(name, result, start)
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
