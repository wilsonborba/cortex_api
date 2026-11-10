use crate::core::process_numbers::ProcessNumber;
use std::sync::Arc;
#[derive(Clone)]
pub struct DocumentFile {
    pub name: String,
    pub content_type: String,
    pub bytes: Arc<Vec<u8>>,
}
pub struct TextPage {
    pub number: u32,
    pub text: String,
    pub ocr: bool,
}
pub struct ExtractedFile {
    pub name: String,
    pub pages: Vec<TextPage>,
    pub error: Option<ProcessNumber>,
    pub time_ms: u64,
}
impl ExtractedFile {
    pub fn text(&self) -> String {
        self.pages
            .iter()
            .map(|p| p.text.as_str())
            .collect::<Vec<_>>()
            .join("\n\n")
    }
}
