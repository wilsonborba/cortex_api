use crate::{core::process_numbers::ProcessNumber, domain::models::usage::Pending};
#[derive(Clone, PartialEq, Eq)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
}
pub struct RankedHit {
    pub hit: SearchHit,
    pub score: f64,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SearchFailure {
    Blocked,
    Unavailable,
    InvalidResponse,
}
pub trait SearchAdapter: Send + Sync {
    fn name(&self) -> &'static str;
    fn search<'a>(&'a self, query: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>>;
}
pub struct SearchResults {
    pub hits: Vec<RankedHit>,
    pub notice: Option<ProcessNumber>,
}
