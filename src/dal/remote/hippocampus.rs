use crate::{
    core::config::Config,
    domain::models::{recall::RecalledMemory, storage::AppUser},
};
use serde::{Deserialize, Serialize};
#[derive(Clone)]
pub struct Hippocampus {
    client: reqwest::Client,
    base: reqwest::Url,
    workspace: String,
}
#[derive(Debug, Clone, Copy)]
pub struct HippoUnavailable;
#[derive(Serialize)]
struct ExchangeMemory<'a> {
    workspace_id: &'a str,
    app_id: &'a str,
    user_id: Option<&'a str>,
    title: &'a str,
    content: String,
}
#[derive(Serialize)]
struct Search<'a> {
    workspace_id: &'a str,
    app_id: &'a str,
    user_id: Option<&'a str>,
    text: &'a str,
    tags: Vec<&'a str>,
    expand: u8,
    limit: usize,
}
#[derive(Deserialize)]
struct SearchResult {
    memory: RecalledMemory,
}
#[derive(Deserialize)]
struct SearchResponse {
    data: Vec<SearchResult>,
}
impl Hippocampus {
    pub async fn search(
        &self,
        scope: &AppUser,
        text: &str,
        topic: Option<&str>,
        limit: usize,
    ) -> Result<Vec<RecalledMemory>, HippoUnavailable> {
        let body = Search {
            workspace_id: &self.workspace,
            app_id: &scope.app_id,
            user_id: if scope.user_id.is_empty() {
                None
            } else {
                Some(&scope.user_id)
            },
            text,
            tags: topic.into_iter().collect(),
            expand: 0,
            limit,
        };
        let url = self
            .base
            .join("api/v1/search")
            .map_err(|_| HippoUnavailable)?;
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|_| HippoUnavailable)?;
        if !response.status().is_success() {
            return Err(HippoUnavailable);
        }
        let response: SearchResponse = response.json().await.map_err(|_| HippoUnavailable)?;
        // Do not trust a malformed or wrong-scope response, even from a LAN peer.
        if response.data.iter().any(|r| {
            r.memory.workspace_id != self.workspace
                || r.memory.app_id != scope.app_id
                || r.memory.user_id.as_deref() != body.user_id
                || r.memory.id.is_empty()
        }) {
            return Err(HippoUnavailable);
        }
        Ok(response
            .data
            .into_iter()
            .take(limit)
            .map(|r| r.memory)
            .collect())
    }
    pub fn new(config: &Config) -> Result<Self, HippoUnavailable> {
        let base = reqwest::Url::parse(&format!(
            "{}/",
            config.hippocampus_url.trim_end_matches('/')
        ))
        .map_err(|_| HippoUnavailable)?;
        if !matches!(base.scheme(), "http" | "https") || base.host_str().is_none() {
            return Err(HippoUnavailable);
        }
        let client = reqwest::Client::builder()
            .timeout(config.hippocampus_timeout)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|_| HippoUnavailable)?;
        Ok(Self {
            client,
            base,
            workspace: config.memory_workspace.into(),
        })
    }
    pub async fn create_exchange(
        &self,
        scope: &AppUser,
        title: &str,
        input: &str,
        output: &str,
    ) -> Result<String, HippoUnavailable> {
        let body = ExchangeMemory {
            workspace_id: &self.workspace,
            app_id: &scope.app_id,
            user_id: if scope.user_id.is_empty() {
                None
            } else {
                Some(&scope.user_id)
            },
            title,
            content: format!("Input:\n{input}\n\nAnswer:\n{output}"),
        };
        let url = self
            .base
            .join("api/v1/memories")
            .map_err(|_| HippoUnavailable)?;
        let response = self
            .client
            .post(url)
            .json(&body)
            .send()
            .await
            .map_err(|_| HippoUnavailable)?;
        if !response.status().is_success() {
            return Err(HippoUnavailable);
        }
        let value: serde_json::Value = response.json().await.map_err(|_| HippoUnavailable)?;
        value["data"]["id"]
            .as_str()
            .filter(|id| !id.is_empty())
            .map(str::to_owned)
            .ok_or(HippoUnavailable)
    }
}
