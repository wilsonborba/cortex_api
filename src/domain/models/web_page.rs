use crate::{
    core::process_numbers::{self, ProcessNumber},
    domain::models::{context::ContextText, events::Source},
};
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct WebLink {
    pub url: String,
    pub text: String,
}
#[derive(Clone, serde::Serialize, serde::Deserialize)]
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

#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct PageConditionalMeta {
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

#[derive(Clone)]
pub enum PageFetchOutcome {
    Page {
        page: WebPage,
        meta: PageConditionalMeta,
        cacheable: bool,
        reset_cooldown_secs: Option<u64>,
    },
    NotModified,
    Blocked {
        status: u16,
        is_challenge: bool,
    },
    RateLimited {
        wait_secs: u64,
    },
}
