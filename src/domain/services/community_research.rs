use serde::{Deserialize, Serialize};

use crate::dal::remote::reddit::{RedditClient, RedditPost};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CommunityAccessOutcome {
    Allowed { attribution: String },
    Unavailable { reason: String },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct CommunityEvidence {
    pub post: RedditPost,
    pub evidence_label: String,
    pub canonical_url: String,
}

/// Evaluates Thai and general community-source access rules:
/// - Pantip / Bloggang: allowed public path only, otherwise unavailable (never scraped).
/// - Blognone: public feeds with explicit attribution.
/// - Reddit: authenticated OAuth API labeled as community evidence.
pub fn evaluate_community_source(
    host: &str,
    has_allowed_public_path: bool,
) -> CommunityAccessOutcome {
    let h = host.trim().to_ascii_lowercase();
    if h == "pantip.com"
        || h.ends_with(".pantip.com")
        || h == "bloggang.com"
        || h.ends_with(".bloggang.com")
    {
        if has_allowed_public_path {
            return CommunityAccessOutcome::Allowed {
                attribution: format!("{h} (allowed public path, community discussion)"),
            };
        }
        return CommunityAccessOutcome::Unavailable {
            reason: format!("{h}: no allowed public access path; unofficial scraping disabled"),
        };
    }
    if h == "blognone.com" || h.ends_with(".blognone.com") {
        return CommunityAccessOutcome::Allowed {
            attribution: "Blognone public feed (community news)".into(),
        };
    }
    if h.contains("reddit.com") {
        return CommunityAccessOutcome::Allowed {
            attribution: "Reddit OAuth API (community discussion)".into(),
        };
    }
    CommunityAccessOutcome::Unavailable {
        reason: format!("{h}: community source policy not enabled"),
    }
}

#[derive(Clone)]
pub struct CommunityResearchService {
    reddit: RedditClient,
}

impl CommunityResearchService {
    pub fn new(reddit: RedditClient) -> Self {
        Self { reddit }
    }

    pub async fn search_reddit(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<CommunityEvidence>, String> {
        let posts = self.reddit.search_posts(query, limit).await?;
        Ok(posts
            .into_iter()
            .map(|post| {
                let canonical_url = format!("https://www.reddit.com{}", post.permalink);
                CommunityEvidence {
                    post,
                    evidence_label:
                        "community_evidence: reddit discussion (not authoritative fact)".into(),
                    canonical_url,
                }
            })
            .collect())
    }
}
