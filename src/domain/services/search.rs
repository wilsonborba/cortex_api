use crate::{
    core::{config::Config, process_numbers},
    dal::remote::search::{adapters, canonical_url},
    domain::models::search::*,
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
/// Each source list contributes once per canonical URL, at its first rank.
pub fn rrf(lists: &[Vec<SearchHit>], k: u32) -> Vec<RankedHit> {
    let mut values = HashMap::<String, (SearchHit, f64, usize)>::new();
    let mut order = 0;
    for list in lists {
        let mut seen = HashSet::new();
        for (index, hit) in list.iter().enumerate() {
            let Some(url) = canonical_url(&hit.url) else {
                continue;
            };
            if !seen.insert(url.clone()) {
                continue;
            }
            let value = values.entry(url.clone()).or_insert_with(|| {
                order += 1;
                (
                    SearchHit {
                        title: hit.title.clone(),
                        url,
                    },
                    0.0,
                    order,
                )
            });
            value.1 += 1.0 / (f64::from(k) + (index + 1) as f64);
        }
    }
    let mut values: Vec<_> = values.into_values().collect();
    values.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.2.cmp(&b.2)));
    values
        .into_iter()
        .map(|(hit, score, _)| RankedHit { hit, score })
        .collect()
}
#[derive(Clone)]
pub struct WebSearch {
    adapters: Vec<Arc<dyn SearchAdapter>>,
    k: u32,
    limit: usize,
}
impl WebSearch {
    pub fn new(config: &Config) -> Result<Self, SearchFailure> {
        Ok(Self::with_adapters(config, adapters(config)?))
    }
    pub fn with_adapters(config: &Config, adapters: Vec<Arc<dyn SearchAdapter>>) -> Self {
        Self {
            adapters,
            k: config.search_rrf_k,
            limit: config.search_results_per_adapter,
        }
    }
    pub async fn search(&self, query: &str) -> SearchResults {
        let results = futures_util::future::join_all(
            self.adapters.iter().map(|adapter| adapter.search(query)),
        )
        .await;
        let mut lists = vec![];
        for (adapter, result) in self.adapters.iter().zip(results) {
            match result {
                Ok(mut list) => {
                    list.truncate(self.limit);
                    lists.push(list)
                }
                Err(_) => tracing::warn!(adapter = adapter.name(), "Web search adapter failed"),
            }
        }
        let notice = if lists.is_empty() {
            Some(process_numbers::WEB_SEARCH_FAILED)
        } else {
            None
        };
        SearchResults {
            hits: rrf(&lists, self.k),
            notice,
        }
    }
}
