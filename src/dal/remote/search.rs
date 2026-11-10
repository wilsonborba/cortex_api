use crate::{
    core::config::{Config, SearchEngine},
    domain::models::{search::*, usage::Pending},
};
use futures_util::StreamExt;
use scraper::{Html, Selector};
use std::sync::Arc;
use url::Url;
pub fn canonical_url(value: &str) -> Option<String> {
    let mut url = Url::parse(value).ok()?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return None;
    }
    url.set_fragment(None);
    Some(url.into())
}
pub fn duckduckgo_html(
    status: u16,
    body: &str,
    limit: usize,
) -> Result<Vec<SearchHit>, SearchFailure> {
    if status == 202 {
        return Err(SearchFailure::Blocked);
    }
    if status != 200 {
        return Err(SearchFailure::Unavailable);
    }
    let html = Html::parse_document(body);
    let selector = Selector::parse("a.result__a").expect("fixed selector");
    let mut hits = vec![];
    for link in html.select(&selector).take(limit) {
        let Some(href) = link.attr("href") else {
            continue;
        };
        let resolved = Url::parse(href).or_else(|_| {
            Url::parse("https://duckduckgo.com/")
                .expect("fixed URL")
                .join(href)
        });
        let Ok(url) = resolved else { continue };
        let target = url
            .query_pairs()
            .find_map(|(key, value)| {
                if key == "uddg" {
                    Some(value.into_owned())
                } else {
                    None
                }
            })
            .unwrap_or_else(|| url.into());
        if let Some(url) = canonical_url(&target) {
            hits.push(SearchHit {
                title: link.text().collect::<String>().trim().to_owned(),
                url,
            });
        }
    }
    if hits.is_empty()
        && html
            .select(&Selector::parse(".no-results,.result--no-result").expect("fixed selector"))
            .next()
            .is_none()
    {
        return Err(SearchFailure::Blocked);
    }
    Ok(hits)
}
pub fn wikipedia_json(body: &[u8], limit: usize) -> Result<Vec<SearchHit>, SearchFailure> {
    let value: serde_json::Value =
        serde_json::from_slice(body).map_err(|_| SearchFailure::InvalidResponse)?;
    let titles = value
        .get(1)
        .and_then(|v| v.as_array())
        .ok_or(SearchFailure::InvalidResponse)?;
    let urls = value
        .get(3)
        .and_then(|v| v.as_array())
        .ok_or(SearchFailure::InvalidResponse)?;
    if titles.len() != urls.len() {
        return Err(SearchFailure::InvalidResponse);
    }
    titles
        .iter()
        .zip(urls)
        .take(limit)
        .map(|(title, url)| {
            Ok(SearchHit {
                title: title.as_str().ok_or(SearchFailure::InvalidResponse)?.into(),
                url: canonical_url(url.as_str().ok_or(SearchFailure::InvalidResponse)?)
                    .ok_or(SearchFailure::InvalidResponse)?,
            })
        })
        .collect()
}
async fn body(response: reqwest::Response, limit: usize) -> Result<Vec<u8>, SearchFailure> {
    let mut stream = response.bytes_stream();
    let mut bytes = vec![];
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|_| SearchFailure::Unavailable)?;
        if chunk.len() > limit.saturating_sub(bytes.len()) {
            return Err(SearchFailure::InvalidResponse);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
pub struct DuckDuckGo {
    client: reqwest::Client,
    endpoint: String,
    limit: usize,
    bytes: usize,
}
pub struct Wikipedia {
    client: reqwest::Client,
    endpoint: String,
    limit: usize,
    bytes: usize,
}
impl SearchAdapter for DuckDuckGo {
    fn name(&self) -> &'static str {
        "duckduckgo"
    }
    fn search<'a>(&'a self, query: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>> {
        Box::pin(async move {
            let response = self
                .client
                .get(&self.endpoint)
                .query(&[("q", query)])
                .send()
                .await
                .map_err(|_| SearchFailure::Unavailable)?;
            let status = response.status().as_u16();
            if status == 202 {
                return Err(SearchFailure::Blocked);
            }
            if status != 200 {
                return Err(SearchFailure::Unavailable);
            }
            let bytes = body(response, self.bytes).await?;
            let text = std::str::from_utf8(&bytes).map_err(|_| SearchFailure::InvalidResponse)?;
            duckduckgo_html(status, text, self.limit)
        })
    }
}
impl SearchAdapter for Wikipedia {
    fn name(&self) -> &'static str {
        "wikipedia"
    }
    fn search<'a>(&'a self, query: &'a str) -> Pending<'a, Result<Vec<SearchHit>, SearchFailure>> {
        Box::pin(async move {
            let limit = self.limit.to_string();
            let response = self
                .client
                .get(&self.endpoint)
                .query(&[
                    ("action", "opensearch"),
                    ("search", query),
                    ("limit", &limit),
                    ("namespace", "0"),
                    ("format", "json"),
                ])
                .send()
                .await
                .map_err(|_| SearchFailure::Unavailable)?;
            if !response.status().is_success() {
                return Err(SearchFailure::Unavailable);
            }
            let bytes = body(response, self.bytes).await?;
            wikipedia_json(&bytes, self.limit)
        })
    }
}
pub fn adapters(config: &Config) -> Result<Vec<Arc<dyn SearchAdapter>>, SearchFailure> {
    let client = reqwest::Client::builder()
        .user_agent(&config.web_user_agent)
        .timeout(config.web_search_timeout)
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|_| SearchFailure::Unavailable)?;
    Ok(config
        .search_adapters
        .iter()
        .map(|adapter| match adapter {
            SearchEngine::DuckDuckGo => Arc::new(DuckDuckGo {
                client: client.clone(),
                endpoint: config.duckduckgo_html_url.clone(),
                limit: config.search_results_per_adapter,
                bytes: config.web_html_bytes,
            }) as Arc<dyn SearchAdapter>,
            SearchEngine::Wikipedia => Arc::new(Wikipedia {
                client: client.clone(),
                endpoint: config.wikipedia_api_url.clone(),
                limit: config.search_results_per_adapter,
                bytes: config.web_html_bytes,
            }) as Arc<dyn SearchAdapter>,
        })
        .collect())
}
