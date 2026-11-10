use crate::{
    core::config::{Config, TokenEstimateRule},
    domain::models::{context::ContextText, web_page::RankedChunk},
};
use text_splitter::{ChunkConfig, ChunkSizer, TextSplitter};
#[derive(Clone, Copy)]
struct TokenSizer(TokenEstimateRule);
impl ChunkSizer for TokenSizer {
    fn size(&self, text: &str) -> usize {
        usize::try_from(self.0.estimate(text)).unwrap_or(usize::MAX)
    }
}
pub fn split(config: &Config, text: &str) -> Vec<String> {
    let splitter = TextSplitter::new(
        ChunkConfig::new(config.web_chunk_tokens.max(1))
            .with_sizer(TokenSizer(config.token_estimate_rule())),
    );
    splitter.chunks(text).map(str::to_owned).collect()
}
pub fn rank(question: &str, chunks: Vec<ContextText>) -> Vec<RankedChunk> {
    if chunks.is_empty() {
        return vec![];
    }
    let engine = bm25::SearchEngineBuilder::<u32>::with_corpus(
        bm25::LanguageMode::Detect,
        chunks.iter().map(|c| c.text.as_str()),
    )
    .build();
    let mut scores = vec![0.0f32; chunks.len()];
    for result in engine.search(question, chunks.len()) {
        scores[result.document.id as usize] = result.score;
    }
    let mut ranked: Vec<_> = chunks
        .into_iter()
        .enumerate()
        .map(|(i, chunk)| {
            (
                i,
                RankedChunk {
                    chunk,
                    score: scores[i],
                },
            )
        })
        .collect();
    ranked.sort_by(|a, b| b.1.score.total_cmp(&a.1.score).then(a.0.cmp(&b.0)));
    ranked.into_iter().map(|(_, chunk)| chunk).collect()
}

/// Find HTTP(S) URLs in input text without reading them or fetching assets.
pub fn pasted_urls(text: &str) -> Vec<String> {
    let matcher = regex::Regex::new(r#"(?i)https?://[^\s<>\"']+"#).expect("fixed URL pattern");
    let mut seen = std::collections::HashSet::new();
    let mut urls = vec![];
    for value in matcher.find_iter(text) {
        let mut value = value
            .as_str()
            .trim_end_matches(['.', ',', ';', '!', '?'])
            .to_owned();
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            while value.ends_with(close)
                && value.matches(close).count() > value.matches(open).count()
            {
                value.pop();
            }
        }
        if let Ok(mut url) = url::Url::parse(&value) {
            url.set_fragment(None);
            let url = String::from(url);
            if seen.insert(url.clone()) {
                urls.push(url);
            }
        }
    }
    urls
}
