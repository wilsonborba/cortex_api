use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::remote::hippocampus::Hippocampus,
    domain::{
        models::{
            context::ContextText,
            extraction::{AudioRange, DocumentFile, FileSelection, TextRange},
            files::{FileContent, FileMemory, Upload},
            identity::Caller,
            memory_graph::GraphError,
            temporary::TemporaryFile,
        },
        services::{events::channel, extraction::DocumentExtractor, temporary::TemporaryChats},
    },
};
#[derive(Clone)]
pub struct FileProxy {
    pub hippocampus: Hippocampus,
    pub extractor: DocumentExtractor,
    pub transcriber: std::sync::Arc<dyn crate::domain::services::transcription::AudioTranscriber>,
    pub temporary: TemporaryChats,
    pub config: Config,
}
const AUDIO_EXTENSIONS: [&str; 7] = ["opus", "ogg", "oga", "mp3", "wav", "m4a", "flac"];
const MAX_SELECTION_SPEC_BYTES: usize = 2000;
const MAX_SELECTION_RANGES: usize = 100;
const MAX_SELECTED_PDF_PAGES: u64 = 100;
fn is_audio(file: &DocumentFile) -> bool {
    file.content_type.to_ascii_lowercase().starts_with("audio/")
        || file
            .name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}
fn lookup_error(error: GraphError) -> ProcessNumber {
    match error {
        GraphError::Unavailable => process_numbers::MEMORY_LOAD_FAILED,
        GraphError::Invalid | GraphError::NotFound => process_numbers::FILE_NOT_FOUND,
    }
}
fn invalid_selection() -> ProcessNumber {
    process_numbers::INVALID_DATA
}
fn parse_pair(spec: &str) -> Result<(usize, usize), ProcessNumber> {
    let (start, end) = spec.split_once('-').ok_or_else(invalid_selection)?;
    if end.contains('-') {
        return Err(invalid_selection());
    }
    let start = start.parse::<usize>().map_err(|_| invalid_selection())?;
    let end = end.parse::<usize>().map_err(|_| invalid_selection())?;
    if start >= end {
        return Err(invalid_selection());
    }
    Ok((start, end))
}
fn parse_selection(
    file: &DocumentFile,
    kind: Option<&str>,
    spec: Option<&str>,
) -> Result<Option<FileSelection>, ProcessNumber> {
    let (Some(kind), Some(spec)) = (kind, spec) else {
        if kind.is_some() || spec.is_some() {
            return Err(invalid_selection());
        }
        return Ok(None);
    };
    if spec.len() > MAX_SELECTION_SPEC_BYTES {
        return Err(invalid_selection());
    }
    let content_type = file
        .content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    match kind {
        "all" if spec.is_empty() => {
            if content_type.starts_with("text/")
                || content_type == "application/pdf"
                || content_type
                    == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
                || content_type.starts_with("image/")
                || is_audio(file)
            {
                Ok(Some(FileSelection::All))
            } else {
                Err(process_numbers::FILE_TYPE)
            }
        }
        "pages" if content_type == "application/pdf" => {
            let items: Vec<_> = spec.split(',').collect();
            if items.is_empty() || items.len() > MAX_SELECTION_RANGES {
                return Err(invalid_selection());
            }
            let mut ranges = Vec::with_capacity(items.len());
            let mut selected_page_count = 0u64;
            let mut previous_page_end = None;
            for item in items {
                let (start, end) = if item.contains('-') {
                    let (start, end) = item.split_once('-').ok_or_else(invalid_selection)?;
                    if end.contains('-') {
                        return Err(invalid_selection());
                    }
                    (
                        start.parse::<u32>().map_err(|_| invalid_selection())?,
                        end.parse::<u32>().map_err(|_| invalid_selection())?,
                    )
                } else {
                    let page = item.parse::<u32>().map_err(|_| invalid_selection())?;
                    (page, page)
                };
                if start == 0 || end < start {
                    return Err(invalid_selection());
                }
                if previous_page_end.is_some_and(|previous| start <= previous) {
                    return Err(invalid_selection());
                }
                previous_page_end = Some(end);
                selected_page_count = selected_page_count
                    .checked_add(u64::from(end) - u64::from(start) + 1)
                    .ok_or_else(invalid_selection)?;
                if selected_page_count > MAX_SELECTED_PDF_PAGES {
                    return Err(invalid_selection());
                }
                ranges.push(start..=end);
            }
            Ok(Some(FileSelection::PdfPages(ranges)))
        }
        "passages" if content_type.starts_with("text/") => {
            let items: Vec<_> = spec.split(',').collect();
            if items.is_empty() || items.len() > MAX_SELECTION_RANGES {
                return Err(invalid_selection());
            }
            let mut ranges = Vec::with_capacity(items.len());
            let mut previous_end = None;
            for item in items {
                let (page, offsets) = item.split_once(':').ok_or_else(invalid_selection)?;
                if page != "p1" {
                    return Err(invalid_selection());
                }
                let (start, end) = parse_pair(offsets)?;
                if previous_end.is_some_and(|previous| start < previous) {
                    return Err(invalid_selection());
                }
                previous_end = Some(end);
                ranges.push(TextRange {
                    start_utf16: start,
                    end_utf16: end,
                });
            }
            Ok(Some(FileSelection::TextPassages(ranges)))
        }
        "intervals" if is_audio(file) => {
            let items: Vec<_> = spec
                .strip_prefix("ms:")
                .ok_or_else(invalid_selection)?
                .split(',')
                .collect();
            if items.is_empty() || items.len() > MAX_SELECTION_RANGES {
                return Err(invalid_selection());
            }
            let mut ranges = Vec::with_capacity(items.len());
            let mut previous_end = None;
            for item in items {
                let (start_ms, end_ms) = parse_pair(item)?;
                let start_ms = u64::try_from(start_ms).map_err(|_| invalid_selection())?;
                let end_ms = u64::try_from(end_ms).map_err(|_| invalid_selection())?;
                if previous_end.is_some_and(|previous| start_ms < previous) {
                    return Err(invalid_selection());
                }
                previous_end = Some(end_ms);
                ranges.push(AudioRange { start_ms, end_ms });
            }
            Ok(Some(FileSelection::AudioIntervals(ranges)))
        }
        _ => Err(invalid_selection()),
    }
}
fn utf16_offset_to_byte(text: &str, target: usize) -> Option<usize> {
    let mut units = 0usize;
    for (byte, ch) in text.char_indices() {
        if units == target {
            return Some(byte);
        }
        let next = units.checked_add(ch.len_utf16())?;
        if target > units && target < next {
            return None;
        }
        units = next;
    }
    (units == target).then_some(text.len())
}
fn select_text_passages(bytes: &[u8], ranges: &[TextRange]) -> Result<Vec<String>, ProcessNumber> {
    let decoded = std::str::from_utf8(bytes).map_err(|_| invalid_selection())?;
    let decoded = decoded.strip_prefix('\u{feff}').unwrap_or(decoded);
    let text = decoded.replace("\r\n", "\n").replace('\r', "\n");
    let mut previous_end = None;
    ranges
        .iter()
        .map(|range| {
            if range.end_utf16 <= range.start_utf16 {
                return Err(invalid_selection());
            }
            if previous_end.is_some_and(|previous| range.start_utf16 < previous) {
                return Err(invalid_selection());
            }
            previous_end = Some(range.end_utf16);
            let start =
                utf16_offset_to_byte(&text, range.start_utf16).ok_or_else(invalid_selection)?;
            let end = utf16_offset_to_byte(&text, range.end_utf16).ok_or_else(invalid_selection)?;
            if end <= start {
                return Err(invalid_selection());
            }
            Ok(text[start..end].to_owned())
        })
        .collect()
}
impl FileProxy {
    /// Extract the text once, then save the file and text in one Hippocampus
    /// call. A temporary chat file stays in Redis only (17.4).
    pub async fn upload(
        &self,
        caller: &Caller,
        file: DocumentFile,
        temporary: bool,
        selection_kind: Option<&str>,
        selection_spec: Option<&str>,
    ) -> Result<Upload, ProcessNumber> {
        if file.bytes.len() > self.config.file_bytes {
            return Err(process_numbers::FILE_SIZE);
        }
        let selection = parse_selection(&file, selection_kind, selection_spec)?;
        if selection.is_some() && !temporary {
            return Err(invalid_selection());
        }
        // Upload has no output stream, so the progress events are dropped.
        let (events, _, _) = channel(&self.config, None);
        let is_image = file.content_type.to_ascii_lowercase().starts_with("image/");
        let mut pages = Vec::new();
        let mut text = String::new();
        match selection {
            None => {
                // Preserve existing upload behavior for callers with no selection fields.
                if !is_image {
                    if is_audio(&file) {
                        text = self
                            .transcriber
                            .transcribe(caller, &file.name, file.bytes.clone(), &events)
                            .await
                            .unwrap_or_default();
                    } else {
                        text = self.extractor.extract(file.clone(), &events).await.text();
                    }
                }
                if !text.is_empty() {
                    pages.push(ContextText {
                        label: file.name.clone(),
                        source: None,
                        text: text.clone(),
                    });
                }
            }
            Some(FileSelection::All) => {
                if !is_image {
                    if is_audio(&file) {
                        text = self
                            .transcriber
                            .transcribe(caller, &file.name, file.bytes.clone(), &events)
                            .await?;
                    } else {
                        let extracted = self.extractor.extract(file.clone(), &events).await;
                        if let Some(error) = extracted.error {
                            return Err(error);
                        }
                        text = extracted.text();
                    }
                }
                if !text.is_empty() {
                    pages.push(ContextText {
                        label: file.name.clone(),
                        source: None,
                        text: text.clone(),
                    });
                }
            }
            Some(FileSelection::PdfPages(ranges)) => {
                let extracted = self
                    .extractor
                    .extract_pdf_pages(file.clone(), &events, Some(ranges.clone()))
                    .await;
                if let Some(error) = extracted.error {
                    return Err(error);
                }
                pages.extend(extracted.pages.into_iter().map(|page| ContextText {
                    label: format!("{}#page={}", file.name, page.number),
                    source: None,
                    text: page.text,
                }));
                text = pages
                    .iter()
                    .map(|page| page.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n\n");
            }
            Some(FileSelection::TextPassages(ranges)) => {
                let selected = select_text_passages(&file.bytes, &ranges)?;
                for (range, content) in ranges.iter().zip(selected) {
                    pages.push(ContextText {
                        label: format!(
                            "{}#p1:{}-{}",
                            file.name, range.start_utf16, range.end_utf16
                        ),
                        source: None,
                        text: content,
                    });
                }
                text = pages
                    .iter()
                    .map(|page| page.text.as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
            }
            Some(FileSelection::AudioIntervals(ranges)) => {
                text = self
                    .transcriber
                    .transcribe_selected(caller, &file.name, file.bytes.clone(), ranges, &events)
                    .await?;
                pages.push(ContextText {
                    label: file.name.clone(),
                    source: None,
                    text: text.clone(),
                });
            }
        }
        if temporary {
            use base64::Engine;
            let image = is_image
                .then(|| base64::engine::general_purpose::STANDARD.encode(file.bytes.as_slice()));
            let id = self
                .temporary
                .save_file(
                    caller,
                    TemporaryFile {
                        file_id: uuid::Uuid::nil(),
                        name: file.name,
                        pages,
                        image,
                    },
                )
                .await?;
            return Ok(Upload::Saved(id.to_string()));
        }
        Ok(
            match self
                .hippocampus
                .save_file(&caller.storage_scope(), &file, &text)
                .await
            {
                Ok(id) => Upload::Saved(id),
                Err(_) => Upload::NotSaved,
            },
        )
    }
    /// The Hippocampus file memory of a file inside the caller's scope.
    pub async fn file(&self, caller: &Caller, file_id: &str) -> Result<FileMemory, ProcessNumber> {
        self.hippocampus
            .file_memory(&caller.storage_scope(), file_id)
            .await
            .map_err(lookup_error)
    }
    /// Check the scope first (8.6), then read the bytes through Hippocampus.
    pub async fn download(
        &self,
        caller: &Caller,
        file_id: &str,
    ) -> Result<FileContent, ProcessNumber> {
        self.file(caller, file_id).await?;
        self.hippocampus
            .file_content(&caller.storage_scope(), file_id)
            .await
            .map_err(lookup_error)
    }
}

#[cfg(test)]
mod selection_tests {
    use super::*;
    use std::sync::Arc;

    fn file(name: &str, content_type: &str, bytes: &[u8]) -> DocumentFile {
        DocumentFile {
            name: name.into(),
            content_type: content_type.into(),
            bytes: Arc::new(bytes.to_vec()),
        }
    }

    #[test]
    fn selection_units_pairs_and_bounds_are_strict() {
        let text = file("a.txt", "text/plain", b"abc");
        assert_eq!(parse_selection(&text, None, None).unwrap(), None);
        assert_eq!(
            parse_selection(&text, Some("all"), Some("")).unwrap(),
            Some(FileSelection::All)
        );
        assert!(parse_selection(&text, Some("passages"), None).is_err());
        assert!(parse_selection(&text, Some("pages"), Some("1")).is_err());
        assert!(parse_selection(&text, Some("passages"), Some("p1:0-3,")).is_err());
        assert!(parse_selection(&text, Some("passages"), Some("p1:0-3")).is_ok());
        assert!(parse_selection(&text, Some("passages"), Some("p1:4-5,p1:0-1")).is_err());
        assert!(parse_selection(&text, Some("passages"), Some("p1:0-3,p1:2-4")).is_err());
        assert!(parse_selection(&text, Some("all"), Some("anything")).is_err());
        assert!(
            parse_selection(
                &text,
                Some("passages"),
                Some(&"x".repeat(MAX_SELECTION_SPEC_BYTES + 1))
            )
            .is_err()
        );
        let over_count = std::iter::repeat_n("p1:0-1", MAX_SELECTION_RANGES + 1)
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse_selection(&text, Some("passages"), Some(&over_count)).is_err());

        let audio = file("a.wav", "audio/wav", b"audio");
        assert!(parse_selection(&audio, Some("intervals"), Some("ms:0-100,200-300")).is_ok());
        assert!(parse_selection(&audio, Some("intervals"), Some("ms:0-100,ms:200-300")).is_err());
        assert!(parse_selection(&audio, Some("intervals"), Some("ms:0-100,50-150")).is_err());
        assert!(parse_selection(&audio, Some("intervals"), Some("ms:100-100")).is_err());
        assert!(parse_selection(&audio, Some("intervals"), Some("ms:200-300,0-100")).is_err());
        assert!(parse_selection(&audio, Some("passages"), Some("p1:0-1")).is_err());

        let pdf = file("a.pdf", "application/pdf", b"pdf");
        assert!(parse_selection(&pdf, Some("pages"), Some("1-3,5")).is_ok());
        assert!(parse_selection(&pdf, Some("pages"), Some("0,2")).is_err());
        assert!(parse_selection(&pdf, Some("pages"), Some("1-101")).is_err());
        assert!(parse_selection(&pdf, Some("pages"), Some("3,1-2")).is_err());
        assert!(parse_selection(&pdf, Some("pages"), Some("1-3,3-4")).is_err());
        assert!(parse_selection(&pdf, Some("pages"), Some("2,2")).is_err());
        assert!(parse_selection(&pdf, Some("intervals"), Some("ms:0-10")).is_err());

        let docx = file(
            "a.docx",
            "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            b"docx",
        );
        let image = file("a.png", "image/png", b"png");
        assert_eq!(
            parse_selection(&docx, Some("all"), Some("")).unwrap(),
            Some(FileSelection::All)
        );
        assert_eq!(
            parse_selection(&image, Some("all"), Some("")).unwrap(),
            Some(FileSelection::All)
        );
        assert!(parse_selection(&image, Some("pages"), Some("1")).is_err());
        assert!(parse_selection(&image, Some("passages"), Some("p1:0-1")).is_err());
        assert!(parse_selection(&image, Some("intervals"), Some("ms:0-1")).is_err());
        let unknown = file("a.bin", "application/octet-stream", b"bin");
        assert!(parse_selection(&unknown, Some("all"), Some("")).is_err());
    }

    #[test]
    fn text_selection_normalizes_before_utf16_offsets_and_rejects_invalid_boundaries() {
        let bytes = "\u{feff}A😀\r\nB".as_bytes();
        let selected = select_text_passages(
            bytes,
            &[TextRange {
                start_utf16: 1,
                end_utf16: 3,
            }],
        )
        .unwrap();
        assert_eq!(selected, ["😀"]);
        assert!(
            select_text_passages(
                bytes,
                &[TextRange {
                    start_utf16: 2,
                    end_utf16: 3
                }]
            )
            .is_err()
        );
        assert!(
            select_text_passages(
                bytes,
                &[TextRange {
                    start_utf16: 0,
                    end_utf16: 6
                }]
            )
            .is_err()
        );
        assert!(
            select_text_passages(
                &[0xff, 0xfe],
                &[TextRange {
                    start_utf16: 0,
                    end_utf16: 1
                }]
            )
            .is_err()
        );
        assert!(
            select_text_passages(
                b"abcdef",
                &[
                    TextRange {
                        start_utf16: 0,
                        end_utf16: 4,
                    },
                    TextRange {
                        start_utf16: 3,
                        end_utf16: 5,
                    },
                ],
            )
            .is_err()
        );
        assert!(
            select_text_passages(
                b"abcdef",
                &[
                    TextRange {
                        start_utf16: 4,
                        end_utf16: 5,
                    },
                    TextRange {
                        start_utf16: 0,
                        end_utf16: 2,
                    },
                ],
            )
            .is_err()
        );
    }
}
