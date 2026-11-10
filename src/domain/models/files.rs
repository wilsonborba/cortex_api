/// Result of `POST /files`. A failed Hippocampus or FSM save is a notice (8.8).
#[derive(Debug, PartialEq, Eq)]
pub enum Upload {
    Saved(String),
    NotSaved,
}
/// File bytes read through Hippocampus for `GET /files/{file_id}/content`.
pub struct FileContent {
    pub content_type: String,
    pub disposition: Option<String>,
    pub bytes: Vec<u8>,
}
