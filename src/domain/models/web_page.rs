use crate::{
    core::process_numbers::{self, ProcessNumber},
    domain::models::{context::ContextText, events::Source},
};
#[derive(Clone)]
pub struct WebLink {
    pub url: String,
    pub text: String,
}
#[derive(Clone)]
pub struct WebPage {
    pub source: Source,
    pub chunks: Vec<ContextText>,
    pub links: Vec<WebLink>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PageFailure {
    Timeout,
    TooLarge,
    WrongType,
    Unavailable,
    NoMainText,
}
impl PageFailure {
    pub fn process(self) -> ProcessNumber {
        match self {
            Self::Timeout => process_numbers::WEB_PAGE_TIMEOUT,
            Self::TooLarge => process_numbers::WEB_PAGE_TOO_LARGE,
            Self::WrongType => process_numbers::WEB_PAGE_TYPE,
            Self::Unavailable => process_numbers::WEB_PAGE_FAILED,
            Self::NoMainText => process_numbers::WEB_PAGE_TEXT,
        }
    }
}
pub struct RankedChunk {
    pub chunk: ContextText,
    pub score: f32,
}
