use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::dal::local::research_sources::ResearchSource;
use crate::domain::models::run_record::WebMode;
use crate::domain::services::research_catalog::ResearchCatalogService;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResearchPurpose {
    General,
    Academic,
    News,
    Technical,
    Reference,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchBudget {
    pub remaining_requests: usize,
    pub remaining_pages: usize,
    pub remaining_time_ms: u64,
    pub max_sources: usize,
}

impl ResearchBudget {
    pub fn for_mode(mode: WebMode) -> Self {
        match mode {
            WebMode::Fast => Self {
                remaining_requests: 6,
                remaining_pages: 4,
                remaining_time_ms: 8_000,
                max_sources: 3,
            },
            WebMode::Medium => Self {
                remaining_requests: 16,
                remaining_pages: 10,
                remaining_time_ms: 25_000,
                max_sources: 6,
            },
            WebMode::Deep => Self {
                remaining_requests: 36,
                remaining_pages: 24,
                remaining_time_ms: 60_000,
                max_sources: 12,
            },
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchRouteRequest {
    pub subjects: Vec<String>,
    pub purpose: ResearchPurpose,
    pub language: Option<String>,
    pub region: Option<String>,
    pub mode: WebMode,
    pub budget: ResearchBudget,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct RoutedSource {
    pub source: ResearchSource,
    pub score: i32,
    pub allocated_requests: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ResearchPlan {
    pub effective_mode: WebMode,
    pub selected: Vec<RoutedSource>,
    pub catalog_total: usize,
    pub page_budget: usize,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceStage {
    FoundLink,
    MetadataOnly,
    ReadContent,
    UsedInAnswer,
}

impl EvidenceStage {
    pub fn label(self) -> &'static str {
        match self {
            Self::FoundLink => "found_link",
            Self::MetadataOnly => "metadata_only",
            Self::ReadContent => "read_content",
            Self::UsedInAnswer => "used_in_answer",
        }
    }

    pub fn is_citable(self) -> bool {
        matches!(self, Self::ReadContent | Self::UsedInAnswer)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct EvidenceItem {
    pub canonical_url: String,
    pub publication_id: Option<String>,
    pub title: String,
    pub source_host: String,
    pub stage: EvidenceStage,
}

/// Canonicalizes a URL for deduplication by lowercasing scheme/host, stripping
/// fragments, tracking query parameters (`utm_*`, `fbclid`, `gclid`), and trailing `/`.
pub fn canonicalize_url(raw: &str) -> String {
    let trimmed = raw.trim();
    let without_frag = trimmed.split('#').next().unwrap_or(trimmed);
    let (base, query_opt) = match without_frag.split_once('?') {
        Some((b, q)) => (b, Some(q)),
        None => (without_frag, None),
    };
    let mut normalized_base = base.to_string();
    if let Some((scheme, rest)) = base.split_once("://") {
        let (host, path) = match rest.split_once('/') {
            Some((h, p)) => (h.to_ascii_lowercase(), format!("/{}", p.trim_end_matches('/'))),
            None => (rest.to_ascii_lowercase(), String::new()),
        };
        let clean_path = if path == "/" { String::new() } else { path };
        normalized_base = format!("{}://{}{}", scheme.to_ascii_lowercase(), host, clean_path);
    }
    if let Some(q) = query_opt {
        let kept: Vec<&str> = q
            .split('&')
            .filter(|kv| {
                let k = kv.split('=').next().unwrap_or("").to_ascii_lowercase();
                !k.is_empty()
                    && !k.starts_with("utm_")
                    && k != "fbclid"
                    && k != "gclid"
                    && k != "ref"
            })
            .collect();
        if !kept.is_empty() {
            normalized_base.push('?');
            normalized_base.push_str(&kept.join("&"));
        }
    }
    normalized_base
}

/// Deduplicates research evidence across sources by canonical URL and publication ID
/// (e.g. DOI / OpenAlex ID), promoting to the highest observed [EvidenceStage].
#[derive(Debug, Default, Clone)]
pub struct EvidenceLedger {
    items: Vec<EvidenceItem>,
    by_url: BTreeMap<String, usize>,
    by_pub_id: BTreeMap<String, usize>,
}

impl EvidenceLedger {
    pub fn record(&mut self, mut item: EvidenceItem) {
        item.canonical_url = canonicalize_url(&item.canonical_url);
        let pub_key = item
            .publication_id
            .as_ref()
            .map(|p| p.trim().to_ascii_lowercase())
            .filter(|p| !p.is_empty());

        let existing_idx = pub_key
            .as_ref()
            .and_then(|k| self.by_pub_id.get(k).copied())
            .or_else(|| self.by_url.get(&item.canonical_url).copied());

        if let Some(idx) = existing_idx {
            let slot = &mut self.items[idx];
            if item.stage > slot.stage {
                slot.stage = item.stage;
            }
            if slot.title.is_empty() && !item.title.is_empty() {
                slot.title = item.title;
            }
            if slot.publication_id.is_none() && item.publication_id.is_some() {
                slot.publication_id = item.publication_id.clone();
            }
            self.by_url.entry(item.canonical_url).or_insert(idx);
            if let Some(pk) = pub_key {
                self.by_pub_id.entry(pk).or_insert(idx);
            }
        } else {
            let idx = self.items.len();
            self.by_url.insert(item.canonical_url.clone(), idx);
            if let Some(pk) = pub_key {
                self.by_pub_id.insert(pk, idx);
            }
            self.items.push(item);
        }
    }

    pub fn all_items(&self) -> &[EvidenceItem] {
        &self.items
    }

    /// Returns only read or answer-used items that may appear in answer citations.
    pub fn citable_items(&self) -> Vec<&EvidenceItem> {
        self.items.iter().filter(|i| i.stage.is_citable()).collect()
    }
}

/// Selects a small, relevant subset of sources based on subjects, purpose, language,
/// region coverage, availability, and remaining budget. Never sweeps all 200 sites.
pub fn select_sources(sources: &[ResearchSource], req: &ResearchRouteRequest) -> ResearchPlan {
    let effective_mode = if req.budget.remaining_time_ms < 10_000
        || req.budget.remaining_requests <= 6
    {
        WebMode::Fast
    } else if req.budget.remaining_time_ms < 25_000 || req.budget.remaining_requests <= 16 {
        match req.mode {
            WebMode::Deep => WebMode::Medium,
            other => other,
        }
    } else {
        req.mode
    };

    let mode_cap = match effective_mode {
        WebMode::Fast => 3,
        WebMode::Medium => 6,
        WebMode::Deep => 12,
    };
    let cap = mode_cap
        .min(req.budget.max_sources)
        .min(req.budget.remaining_requests);

    if cap == 0 {
        return ResearchPlan {
            effective_mode,
            selected: Vec::new(),
            catalog_total: sources.len(),
            page_budget: 0,
        };
    }

    let wanted_subjects: BTreeSet<String> = req
        .subjects
        .iter()
        .map(|s| s.trim().to_ascii_lowercase())
        .filter(|s| !s.is_empty())
        .collect();
    let wanted_lang = req.language.as_deref().map(|l| l.trim().to_ascii_lowercase());
    let wanted_region = req.region.as_deref().map(|r| r.trim().to_ascii_lowercase());

    let mut scored: Vec<(i32, &ResearchSource)> = sources
        .iter()
        .filter(|s| s.is_enabled && s.access_mode != "unavailable")
        .filter_map(|s| {
            let mut score = 0i32;
            let subject_hits = s
                .subjects
                .iter()
                .filter(|sub| wanted_subjects.contains(&sub.to_ascii_lowercase()))
                .count() as i32;
            if !wanted_subjects.is_empty() && subject_hits == 0 {
                return None;
            }
            score += subject_hits * 10;

            let mode_lc = s.access_mode.to_ascii_lowercase();
            let desc_lc = s.description.to_ascii_lowercase();
            score += match req.purpose {
                ResearchPurpose::Academic => {
                    if mode_lc.contains("api") || desc_lc.contains("journal") || desc_lc.contains("paper") {
                        6
                    } else {
                        1
                    }
                }
                ResearchPurpose::News => {
                    if desc_lc.contains("news") || desc_lc.contains("press") {
                        6
                    } else {
                        1
                    }
                }
                ResearchPurpose::Technical => {
                    if desc_lc.contains("code") || desc_lc.contains("spec") || desc_lc.contains("doc") {
                        6
                    } else {
                        1
                    }
                }
                ResearchPurpose::Reference => {
                    if desc_lc.contains("encyclopedia") || desc_lc.contains("reference") || mode_lc.contains("api") {
                        5
                    } else {
                        2
                    }
                }
                ResearchPurpose::General => 2,
            };

            if let Some(ref lang) = wanted_lang {
                let langs = s.languages.to_ascii_lowercase();
                if langs.contains(lang) {
                    score += 4;
                } else if langs.contains("en") || langs.contains("all") {
                    score += 1;
                }
            }
            if let Some(ref reg) = wanted_region {
                let regs = s.regions.to_ascii_lowercase();
                if regs.contains(reg) {
                    score += 3;
                } else if regs.contains("global") {
                    score += 1;
                }
            }

            Some((score, s))
        })
        .collect();

    scored.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.host.cmp(&b.1.host)));

    let mut remaining_req = req.budget.remaining_requests;
    let mut selected = Vec::new();
    for (score, src) in scored.into_iter().take(cap) {
        if remaining_req == 0 {
            break;
        }
        let max_per_src = (src.max_requests_per_run.max(1) as usize).min(remaining_req);
        let alloc = match effective_mode {
            WebMode::Fast => max_per_src.min(2),
            WebMode::Medium => max_per_src.min(3),
            WebMode::Deep => max_per_src.min(4),
        };
        remaining_req -= alloc;
        selected.push(RoutedSource {
            source: src.clone(),
            score,
            allocated_requests: alloc,
        });
    }

    let page_budget = match effective_mode {
        WebMode::Fast => req.budget.remaining_pages.min(4),
        WebMode::Medium => req.budget.remaining_pages.min(10),
        WebMode::Deep => req.budget.remaining_pages.min(24),
    };

    ResearchPlan {
        effective_mode,
        selected,
        catalog_total: sources.len(),
        page_budget,
    }
}

/// Convenience helper that routes from the cached [ResearchCatalogService].
pub async fn route_from_catalog(
    catalog: &ResearchCatalogService,
    req: &ResearchRouteRequest,
) -> Result<ResearchPlan, String> {
    let enabled = catalog.list_enabled_sources().await?;
    Ok(select_sources(&enabled, req))
}
