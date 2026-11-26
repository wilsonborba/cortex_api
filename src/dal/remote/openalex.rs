use crate::dal::remote::web_page::page_url;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAlexWork {
    pub id: String,
    pub doi: Option<String>,
    pub title: String,
    pub publication_year: Option<i32>,
    pub abstract_text: Option<String>,
    pub open_access_url: Option<String>,
    pub is_oa: bool,
}

#[derive(Debug, Deserialize)]
struct OpenAlexSearchResponse {
    #[serde(default)]
    results: Vec<RawOpenAlexWork>,
}

#[derive(Debug, Deserialize)]
struct RawOpenAlexWork {
    id: String,
    doi: Option<String>,
    title: Option<String>,
    publication_year: Option<i32>,
    open_access: Option<RawOpenAccess>,
    abstract_inverted_index: Option<BTreeMap<String, Vec<usize>>>,
}

#[derive(Debug, Deserialize)]
struct RawOpenAccess {
    #[serde(default)]
    is_oa: bool,
    oa_url: Option<String>,
}

fn reconstruct_abstract(inv: Option<BTreeMap<String, Vec<usize>>>) -> Option<String> {
    let inv = inv?;
    let mut positioned: Vec<(usize, String)> = Vec::new();
    for (word, positions) in inv {
        for pos in positions {
            positioned.push((pos, word.clone()));
        }
    }
    if positioned.is_empty() {
        return None;
    }
    positioned.sort_by_key(|(pos, _)| *pos);
    Some(
        positioned
            .into_iter()
            .map(|(_, w)| w)
            .collect::<Vec<_>>()
            .join(" "),
    )
}

#[derive(Clone)]
pub struct OpenAlexClient {
    http: reqwest::Client,
    base_url: String,
    api_key: Option<String>,
    mailto: Option<String>,
}

impl OpenAlexClient {
    pub fn from_env() -> Result<Self, String> {
        let api_key = std::env::var("CORTEX_OPENALEX_API_KEY")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        let mailto = std::env::var("CORTEX_OPENALEX_MAILTO")
            .ok()
            .map(|v| v.trim().to_string())
            .filter(|v| !v.is_empty());
        Self::new("https://api.openalex.org", api_key, mailto)
    }

    pub fn new(
        base_url: impl Into<String>,
        api_key: Option<String>,
        mailto: Option<String>,
    ) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .use_rustls_tls()
            .timeout(Duration::from_secs(10))
            .user_agent("cortex-api-research/1.0")
            .build()
            .map_err(|e| format!("Failed to build OpenAlex HTTP client: {e}"))?;
        Ok(Self {
            http,
            base_url: base_url.into().trim_end_matches('/').to_string(),
            api_key,
            mailto,
        })
    }

    pub fn has_api_key(&self) -> bool {
        self.api_key.is_some()
    }

    pub async fn search_works(
        &self,
        query: &str,
        per_page: usize,
    ) -> Result<Vec<OpenAlexWork>, String> {
        let mut req = self.http.get(format!("{}/works", self.base_url)).query(&[
            ("search", query),
            ("per-page", &per_page.clamp(1, 25).to_string()),
        ]);
        if let Some(ref key) = self.api_key {
            req = req.query(&[("api_key", key.as_str())]);
        }
        if let Some(ref mail) = self.mailto {
            req = req.query(&[("mailto", mail.as_str())]);
        }

        let resp = req.send().await.map_err(|e| {
            // Never log or format the raw error because its URL contains api_key.
            if e.is_timeout() {
                "OpenAlex request timed out".to_string()
            } else {
                "OpenAlex request failed".to_string()
            }
        })?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("OpenAlex returned HTTP {}", status.as_u16()));
        }
        let body: OpenAlexSearchResponse = resp
            .json()
            .await
            .map_err(|_| "OpenAlex JSON decode failed".to_string())?;

        Ok(body
            .results
            .into_iter()
            .map(|w| {
                let oa = w.open_access.unwrap_or(RawOpenAccess {
                    is_oa: false,
                    oa_url: None,
                });
                OpenAlexWork {
                    id: w.id,
                    doi: w.doi,
                    title: w.title.unwrap_or_default(),
                    publication_year: w.publication_year,
                    abstract_text: reconstruct_abstract(w.abstract_inverted_index),
                    open_access_url: oa.oa_url,
                    is_oa: oa.is_oa,
                }
            })
            .collect())
    }

    pub async fn fetch_oa_content(&self, url: &str, max_bytes: usize) -> Result<String, String> {
        let valid_url =
            page_url(url).map_err(|_| "OpenAlex content URL is not allowed".to_string())?;
        let resp = self
            .http
            .get(valid_url)
            .send()
            .await
            .map_err(|_| "OpenAlex content fetch failed".to_string())?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!(
                "OpenAlex content returned HTTP {}",
                status.as_u16()
            ));
        }
        let bytes = resp
            .bytes()
            .await
            .map_err(|_| "OpenAlex content read failed".to_string())?;
        let slice = &bytes[..bytes.len().min(max_bytes)];
        Ok(String::from_utf8_lossy(slice).into_owned())
    }
}
