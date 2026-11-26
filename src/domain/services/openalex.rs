use serde::{Deserialize, Serialize};

use crate::dal::remote::openalex::{OpenAlexClient, OpenAlexWork};
use crate::domain::services::research_router::EvidenceStage;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OpenAlexEvidence {
    pub work: OpenAlexWork,
    pub stage: EvidenceStage,
    pub limitation_label: String,
    pub content_excerpt: Option<String>,
}

/// Domain service for OpenAlex scholarly research.
/// Enforces the rule that metadata is not proof a paper was read.
#[derive(Clone)]
pub struct OpenAlexResearchService {
    client: OpenAlexClient,
}

impl OpenAlexResearchService {
    pub fn new(client: OpenAlexClient) -> Self {
        Self { client }
    }

    pub async fn gather(
        &self,
        query: &str,
        per_page: usize,
        read_full_text_when_available: bool,
    ) -> Result<Vec<OpenAlexEvidence>, String> {
        let works = self.client.search_works(query, per_page).await?;
        let mut out = Vec::with_capacity(works.len());

        for work in works {
            if !read_full_text_when_available {
                out.push(OpenAlexEvidence {
                    work,
                    stage: EvidenceStage::MetadataOnly,
                    limitation_label: "metadata_only: full paper text not read".into(),
                    content_excerpt: None,
                });
                continue;
            }

            if let Some(ref oa_url) = work.open_access_url {
                match self.client.fetch_oa_content(oa_url, 64 * 1024).await {
                    Ok(text) if !text.trim().is_empty() => {
                        let excerpt: String = text.chars().take(1000).collect();
                        out.push(OpenAlexEvidence {
                            work,
                            stage: EvidenceStage::ReadContent,
                            limitation_label: "read_content: open-access text retrieved".into(),
                            content_excerpt: Some(excerpt),
                        });
                    }
                    Ok(_) | Err(_) => {
                        out.push(OpenAlexEvidence {
                            work,
                            stage: EvidenceStage::MetadataOnly,
                            limitation_label: "metadata_only: full-text fetch unavailable".into(),
                            content_excerpt: None,
                        });
                    }
                }
            } else {
                out.push(OpenAlexEvidence {
                    work,
                    stage: EvidenceStage::MetadataOnly,
                    limitation_label: "metadata_only: no open-access full text available".into(),
                    content_excerpt: None,
                });
            }
        }
        Ok(out)
    }
}
