use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RedditPost {
    pub id: String,
    pub subreddit: String,
    pub title: String,
    pub permalink: String,
    pub score: i64,
    pub num_comments: i64,
    pub selftext_excerpt: String,
}

#[derive(Debug, Deserialize)]
struct TokenResponse {
    access_token: String,
}

#[derive(Debug, Deserialize)]
struct ListingResponse {
    data: ListingData,
}

#[derive(Debug, Deserialize)]
struct ListingData {
    #[serde(default)]
    children: Vec<ListingChild>,
}

#[derive(Debug, Deserialize)]
struct ListingChild {
    data: RawPost,
}

#[derive(Debug, Deserialize)]
struct RawPost {
    id: String,
    subreddit: Option<String>,
    title: Option<String>,
    permalink: Option<String>,
    #[serde(default)]
    score: i64,
    #[serde(default)]
    num_comments: i64,
    selftext: Option<String>,
}

#[derive(Clone)]
pub struct RedditClient {
    http: reqwest::Client,
    auth_base: String,
    api_base: String,
    client_id: String,
    client_secret: String,
    user_agent: String,
}

impl RedditClient {
    pub fn from_env() -> Result<Self, String> {
        let client_id = std::env::var("CORTEX_REDDIT_CLIENT_ID")
            .map_err(|_| "Missing CORTEX_REDDIT_CLIENT_ID".to_string())?;
        let client_secret = std::env::var("CORTEX_REDDIT_CLIENT_SECRET")
            .map_err(|_| "Missing CORTEX_REDDIT_CLIENT_SECRET".to_string())?;
        let user_agent = std::env::var("CORTEX_REDDIT_USER_AGENT")
            .unwrap_or_else(|_| "asodya/1.0".to_string());
        Self::new(
            "https://www.reddit.com",
            "https://oauth.reddit.com",
            client_id,
            client_secret,
            user_agent,
        )
    }

    pub fn new(
        auth_base: impl Into<String>,
        api_base: impl Into<String>,
        client_id: impl Into<String>,
        client_secret: impl Into<String>,
        user_agent: impl Into<String>,
    ) -> Result<Self, String> {
        let ua = user_agent.into();
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(ua.clone())
            .build()
            .map_err(|e| format!("Failed to build Reddit HTTP client: {e}"))?;
        Ok(Self {
            http,
            auth_base: auth_base.into().trim_end_matches('/').to_string(),
            api_base: api_base.into().trim_end_matches('/').to_string(),
            client_id: client_id.into(),
            client_secret: client_secret.into(),
            user_agent: ua,
        })
    }

    pub fn user_agent(&self) -> &str {
        &self.user_agent
    }

    async fn fetch_app_token(&self) -> Result<String, String> {
        let resp = self
            .http
            .post(format!("{}/api/v1/access_token", self.auth_base))
            .basic_auth(&self.client_id, Some(&self.client_secret))
            .form(&[("grant_type", "client_credentials")])
            .send()
            .await
            .map_err(|e| format!("Reddit OAuth request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("Reddit OAuth returned HTTP {}", status.as_u16()));
        }
        let tok: TokenResponse = resp
            .json()
            .await
            .map_err(|e| format!("Reddit OAuth JSON decode failed: {e}"))?;
        Ok(tok.access_token)
    }

    pub async fn search_posts(&self, query: &str, limit: usize) -> Result<Vec<RedditPost>, String> {
        let token = self.fetch_app_token().await?;
        let resp = self
            .http
            .get(format!("{}/search", self.api_base))
            .bearer_auth(token)
            .query(&[
                ("q", query),
                ("limit", &limit.clamp(1, 25).to_string()),
                ("sort", "relevance"),
            ])
            .send()
            .await
            .map_err(|e| format!("Reddit search request failed: {e}"))?;
        let status = resp.status();
        if !status.is_success() {
            return Err(format!("Reddit API returned HTTP {}", status.as_u16()));
        }
        let listing: ListingResponse = resp
            .json()
            .await
            .map_err(|e| format!("Reddit JSON decode failed: {e}"))?;
        Ok(listing
            .data
            .children
            .into_iter()
            .map(|c| {
                let raw = c.data;
                let excerpt: String = raw.selftext.unwrap_or_default().chars().take(500).collect();
                RedditPost {
                    id: raw.id,
                    subreddit: raw.subreddit.unwrap_or_default(),
                    title: raw.title.unwrap_or_default(),
                    permalink: raw.permalink.unwrap_or_default(),
                    score: raw.score,
                    num_comments: raw.num_comments,
                    selftext_excerpt: excerpt,
                }
            })
            .collect())
    }
}
