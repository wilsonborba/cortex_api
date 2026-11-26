use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::remote::web_page::{PageReader, page_url},
    domain::{
        models::{
            context::ContextText,
            events::{NativeEvent, Source, StepName, StepProgress},
            pipeline::{SourceNotice, StageName},
            run_record::WebMode,
            web::GatheredWeb,
            web_page::{PageFailure, PageFetchOutcome, WebPage},
        },
        services::{
            chunks::{pasted_urls, rank},
            community_research::{
                CommunityAccessOutcome, CommunityResearchService, evaluate_community_source,
            },
            events::EventSink,
            openalex::OpenAlexResearchService,
            research_catalog::ResearchCatalogService,
            research_limits::{
                CachedWebPage, FetchDecision, InMemoryLimitStore, ResearchPageCache,
                ResearchPolitenessGuard, evaluate_publisher_policy,
            },
            research_router::{
                EvidenceItem, EvidenceLedger, EvidenceStage, canonicalize_url,
                classify_research_request, route_from_catalog,
            },
            robots::{CrawlPermission, Robots},
            search::WebSearch,
            security::SecurityShield,
        },
    },
};
use futures_util::{StreamExt, future::BoxFuture, stream::FuturesUnordered};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};
use tokio::time::Instant;

pub(crate) enum PolicedPageError {
    Failure(PageFailure),
    Decision {
        host: String,
        decision: FetchDecision,
    },
}

type PageResult = (
    usize,
    String,
    Result<(WebPage, Vec<ProcessNumber>), PolicedPageError>,
);

#[derive(Clone, Default)]
pub struct ResearchProviders {
    pub catalog: Option<ResearchCatalogService>,
    pub openalex: Option<Arc<OpenAlexResearchService>>,
    pub community: Option<Arc<CommunityResearchService>>,
}

#[derive(Clone)]
pub struct FastWeb {
    search: WebSearch,
    reader: PageReader,
    shield: Arc<SecurityShield>,
    politeness: Arc<ResearchPolitenessGuard>,
    page_cache: Arc<ResearchPageCache>,
    providers: ResearchProviders,
    enforce_robots: bool,
    config: Config,
}

pub(crate) fn notice(output: &mut GatheredWeb, process: ProcessNumber, source: Option<String>) {
    // The stage reporter emits these once with the final Gather result.
    output.notices.push(SourceNotice {
        process,
        label: "web".into(),
        source,
    });
}

pub(crate) fn record_policed_error(output: &mut GatheredWeb, url: String, err: PolicedPageError) {
    match err {
        PolicedPageError::Failure(error) => {
            if let Ok(parsed) = page_url(&url)
                && let Some(host) = parsed.host_str()
            {
                output
                    .coverage
                    .record_skipped(host, format!("{host}: fetch failed"));
            }
            notice(output, error.process(), Some(url));
        }
        PolicedPageError::Decision { host, decision } => match decision {
            FetchDecision::BlockedStop { reason } => {
                output.coverage.record_skipped(&host, reason);
                notice(output, process_numbers::WEB_SOURCE_BLOCKED, Some(url));
            }
            FetchDecision::RateLimitedCooldown { wait_secs } => {
                output.coverage.record_skipped(
                    &host,
                    format!("{host}: rate limited (cooldown {wait_secs}s)"),
                );
                notice(output, process_numbers::WEB_RATE_LIMITED, Some(url));
            }
            FetchDecision::Proceed => {}
        },
    }
}

pub(crate) async fn read_policed_page(
    reader: &PageReader,
    politeness: &ResearchPolitenessGuard,
    page_cache: &ResearchPageCache,
    shield: &SecurityShield,
    url: &str,
    scope_key: &str,
) -> Result<(WebPage, Vec<ProcessNumber>), PolicedPageError> {
    let parsed = page_url(url).map_err(PolicedPageError::Failure)?;
    let host = parsed
        .host_str()
        .ok_or(PolicedPageError::Failure(PageFailure::Unavailable))?
        .to_ascii_lowercase();
    let cfg = politeness.config();
    let permit = match politeness
        .acquire_host(
            &host,
            cfg.default_host_spacing_ms,
            scope_key,
            cfg.provider_quota_per_minute,
            60,
        )
        .await
    {
        Ok(Ok(p)) => p,
        Ok(Err(decision)) => return Err(PolicedPageError::Decision { host, decision }),
        Err(_) => return Err(PolicedPageError::Failure(PageFailure::Unavailable)),
    };

    let cached = page_cache.get(url).await;
    let outcome = reader
        .read_policed(url, cached.as_ref().map(|c| &c.meta), cfg.max_backoff_secs)
        .await;
    drop(permit);

    match outcome {
        Ok(PageFetchOutcome::NotModified) => {
            if let Some(mut hit) = cached {
                let (safe, notices) = shield.filter_chunks(hit.page.chunks).await;
                hit.page.chunks = safe;
                Ok((hit.page, notices))
            } else {
                Err(PolicedPageError::Failure(PageFailure::Unavailable))
            }
        }
        Ok(PageFetchOutcome::Page {
            mut page,
            meta,
            cacheable,
            reset_cooldown_secs,
        }) => {
            if let Some(wait_secs) = reset_cooldown_secs {
                let _ = politeness.set_host_cooldown(&host, wait_secs).await;
            }
            if cacheable {
                page_cache
                    .put(
                        url,
                        CachedWebPage {
                            page: page.clone(),
                            meta,
                        },
                    )
                    .await;
            }
            let (safe, notices) = shield.filter_chunks(page.chunks).await;
            page.chunks = safe;
            Ok((page, notices))
        }
        Ok(PageFetchOutcome::Blocked {
            status,
            is_challenge,
        }) => {
            let decision = politeness
                .handle_status(&host, status, None, is_challenge)
                .await
                .unwrap_or(FetchDecision::BlockedStop {
                    reason: format!("{host}: blocked"),
                });
            Err(PolicedPageError::Decision { host, decision })
        }
        Ok(PageFetchOutcome::RateLimited { wait_secs }) => {
            let wait_str = wait_secs.to_string();
            let decision = politeness
                .handle_status(&host, 429, Some(&wait_str), false)
                .await
                .unwrap_or(FetchDecision::RateLimitedCooldown { wait_secs });
            Err(PolicedPageError::Decision { host, decision })
        }
        Err(e) => Err(PolicedPageError::Failure(e)),
    }
}

/// Executes catalog-guided research across routed sources (OpenAlex, Reddit/community,
/// and policy checks), respecting budgets, politeness limits, and evidence stages.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn gather_routed_providers(
    providers: &ResearchProviders,
    politeness: &ResearchPolitenessGuard,
    shield: &SecurityShield,
    question: &str,
    mode: WebMode,
    remaining_time_ms: u64,
    output: &mut GatheredWeb,
    ledger: &mut EvidenceLedger,
    events: &EventSink,
) -> Vec<WebPage> {
    let Some(ref catalog) = providers.catalog else {
        return Vec::new();
    };
    let req = classify_research_request(question, mode, remaining_time_ms);
    let Ok(plan) = route_from_catalog(catalog, &req).await else {
        return Vec::new();
    };
    output.selected_hosts = plan
        .selected
        .iter()
        .map(|r| r.source.host.clone())
        .collect();

    let mut pages = Vec::new();
    let mut remaining_pages = plan.page_budget;

    for routed in plan.selected {
        if remaining_pages == 0 {
            break;
        }
        let host = routed.source.host.to_ascii_lowercase();
        let policy = evaluate_publisher_policy(&host, Some(&routed.source.access_mode), true);
        if let FetchDecision::BlockedStop { reason } = policy {
            output.coverage.record_skipped(&host, reason);
            notice(
                output,
                process_numbers::WEB_SOURCE_BLOCKED,
                Some(routed.source.url),
            );
            continue;
        }

        if host == "pantip.com"
            || host.ends_with(".pantip.com")
            || host == "bloggang.com"
            || host.ends_with(".bloggang.com")
        {
            if let CommunityAccessOutcome::Unavailable { reason } =
                evaluate_community_source(&host, false)
            {
                output.coverage.record_skipped(&host, reason);
                notice(
                    output,
                    process_numbers::WEB_SOURCE_BLOCKED,
                    Some(routed.source.url),
                );
            }
            continue;
        }

        if (host == "openalex.org" || host.ends_with(".openalex.org"))
            && let Some(ref openalex) = providers.openalex
        {
            let spacing = routed.source.min_spacing_ms.max(0) as u64;
            let quota = politeness.config().provider_quota_per_minute;
            match politeness
                .acquire_host("api.openalex.org", spacing, "provider:openalex", quota, 60)
                .await
            {
                Ok(Ok(_permit)) => {
                    let mut step = StepProgress::new(StageName::Gather, StepName::WebSearch);
                    step.params.insert("source".into(), "openalex".into());
                    events.send(NativeEvent::Step(step));
                    let per_page = routed.allocated_requests.min(remaining_pages).clamp(1, 5);
                    let read_oa = plan.effective_mode != WebMode::Fast || remaining_pages >= 2;
                    match openalex.gather(question, per_page, read_oa).await {
                        Ok(items) => {
                            for ev in items {
                                let pub_id =
                                    ev.work.doi.clone().or_else(|| Some(ev.work.id.clone()));
                                let raw_url = ev
                                    .work
                                    .open_access_url
                                    .clone()
                                    .or_else(|| ev.work.doi.clone())
                                    .unwrap_or_else(|| ev.work.id.clone());
                                let canon = canonicalize_url(&raw_url);
                                ledger.record(EvidenceItem {
                                    canonical_url: canon.clone(),
                                    publication_id: pub_id,
                                    title: ev.work.title.clone(),
                                    source_host: "openalex.org".into(),
                                    stage: ev.stage,
                                });
                                if ev.stage == EvidenceStage::ReadContent
                                    && let Some(excerpt) = ev.content_excerpt
                                    && remaining_pages > 0
                                {
                                    let (safe, notices) = shield
                                        .filter_chunks(vec![ContextText {
                                            label: format!("[read_content] {}", ev.work.title),
                                            source: Some(canon.clone()),
                                            text: excerpt,
                                        }])
                                        .await;
                                    for p in notices {
                                        if p.number != process_numbers::SECURITY_BLOCK.number {
                                            notice(output, p, Some(canon.clone()));
                                        }
                                    }
                                    if !safe.is_empty() {
                                        remaining_pages = remaining_pages.saturating_sub(1);
                                        output.pages_read += 1;
                                        pages.push(WebPage {
                                            source: Source {
                                                title: format!("[read_content] {}", ev.work.title),
                                                url: canon,
                                            },
                                            chunks: safe,
                                            links: vec![],
                                        });
                                    }
                                }
                            }
                        }
                        Err(_) => {
                            output.coverage.record_skipped(
                                "openalex.org",
                                "openalex.org: provider unavailable",
                            );
                        }
                    }
                }
                Ok(Err(decision)) => {
                    record_policed_error(
                        output,
                        routed.source.url,
                        PolicedPageError::Decision { host, decision },
                    );
                }
                Err(_) => {
                    output
                        .coverage
                        .record_skipped(&host, format!("{host}: limit store unavailable"));
                }
            }
            continue;
        }

        if (host == "reddit.com" || host.ends_with(".reddit.com"))
            && let Some(ref community) = providers.community
        {
            let spacing = routed.source.min_spacing_ms.max(0) as u64;
            let quota = politeness.config().provider_quota_per_minute;
            match politeness
                .acquire_host("oauth.reddit.com", spacing, "provider:reddit", quota, 60)
                .await
            {
                Ok(Ok(_permit)) => {
                    let mut step = StepProgress::new(StageName::Gather, StepName::WebSearch);
                    step.params.insert("source".into(), "reddit".into());
                    events.send(NativeEvent::Step(step));
                    let limit = routed.allocated_requests.min(remaining_pages).clamp(1, 5);
                    match community.search_reddit(question, limit).await {
                        Ok(posts) => {
                            for ce in posts {
                                let canon = canonicalize_url(&ce.canonical_url);
                                let body_text = if ce.post.selftext_excerpt.trim().is_empty() {
                                    ce.post.title.clone()
                                } else {
                                    format!("{}: {}", ce.post.title, ce.post.selftext_excerpt)
                                };
                                let label = format!(
                                    "[community_evidence: reddit r/{}] {}",
                                    ce.post.subreddit, ce.post.title
                                );
                                let (safe, notices) = shield
                                    .filter_chunks(vec![ContextText {
                                        label: label.clone(),
                                        source: Some(canon.clone()),
                                        text: body_text,
                                    }])
                                    .await;
                                for p in notices {
                                    if p.number != process_numbers::SECURITY_BLOCK.number {
                                        notice(output, p, Some(canon.clone()));
                                    }
                                }
                                ledger.record(EvidenceItem {
                                    canonical_url: canon.clone(),
                                    publication_id: Some(format!("reddit:{}", ce.post.id)),
                                    title: label.clone(),
                                    source_host: "reddit.com".into(),
                                    stage: if safe.is_empty() {
                                        EvidenceStage::MetadataOnly
                                    } else {
                                        EvidenceStage::ReadContent
                                    },
                                });
                                if !safe.is_empty() && remaining_pages > 0 {
                                    remaining_pages = remaining_pages.saturating_sub(1);
                                    output.pages_read += 1;
                                    pages.push(WebPage {
                                        source: Source {
                                            title: label,
                                            url: canon,
                                        },
                                        chunks: safe,
                                        links: vec![],
                                    });
                                }
                            }
                        }
                        Err(err) => {
                            output
                                .coverage
                                .record_skipped("reddit.com", format!("reddit.com: {err}"));
                        }
                    }
                }
                Ok(Err(decision)) => {
                    record_policed_error(
                        output,
                        routed.source.url,
                        PolicedPageError::Decision { host, decision },
                    );
                }
                Err(_) => {
                    output
                        .coverage
                        .record_skipped(&host, format!("{host}: limit store unavailable"));
                }
            }
        }
    }

    pages
}

impl FastWeb {
    pub fn new(
        config: &Config,
        search: WebSearch,
        reader: PageReader,
        shield: Arc<SecurityShield>,
    ) -> Self {
        Self {
            search,
            reader,
            shield,
            politeness: Arc::new(ResearchPolitenessGuard::from_config(
                config,
                Arc::new(InMemoryLimitStore::default()),
            )),
            page_cache: Arc::new(ResearchPageCache::in_memory(config.research_cache_ttl)),
            providers: ResearchProviders::default(),
            enforce_robots: false,
            config: config.clone(),
        }
    }

    pub fn with_politeness(
        mut self,
        politeness: Arc<ResearchPolitenessGuard>,
        page_cache: Arc<ResearchPageCache>,
    ) -> Self {
        self.politeness = politeness;
        self.page_cache = page_cache;
        self
    }

    pub fn with_robots(mut self, enforce_robots: bool) -> Self {
        self.enforce_robots = enforce_robots;
        self
    }

    pub fn with_providers(mut self, providers: ResearchProviders) -> Self {
        self.providers = providers;
        self
    }

    fn schedule<'a>(
        &'a self,
        index: usize,
        url: String,
        total: Option<u64>,
        events: &EventSink,
        pages: &mut FuturesUnordered<BoxFuture<'a, PageResult>>,
    ) {
        let mut step = StepProgress::new(StageName::Gather, StepName::WebPageRead);
        step.params.insert("url".into(), url.clone().into());
        step.progress = total.map(|total| [index as u64 + 1, total]);
        events.send(NativeEvent::Step(step));
        pages.push(Box::pin(async move {
            let host = page_url(&url)
                .ok()
                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                .unwrap_or_else(|| "unknown".into());
            let scope = format!("host:{host}");
            let result = read_policed_page(
                &self.reader,
                &self.politeness,
                &self.page_cache,
                &self.shield,
                &url,
                &scope,
            )
            .await;
            (index, url, result)
        }));
    }

    async fn allow_robots(
        &self,
        robots: &mut Robots,
        url: &str,
        deadline: Instant,
        output: &mut GatheredWeb,
    ) -> bool {
        if !self.enforce_robots {
            return true;
        }
        let host = page_url(url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
            .unwrap_or_else(|| "unknown".into());
        match robots
            .check(&self.reader, &self.config, url, deadline)
            .await
        {
            CrawlPermission::Allowed(_) => true,
            CrawlPermission::Denied => {
                let decision = evaluate_publisher_policy(&host, None, false);
                record_policed_error(
                    output,
                    url.to_string(),
                    PolicedPageError::Decision { host, decision },
                );
                false
            }
            CrawlPermission::Unavailable => {
                output
                    .coverage
                    .record_skipped(&host, format!("{host}: robots.txt unavailable"));
                notice(
                    output,
                    process_numbers::WEB_ROBOTS_FAILED,
                    Some(url.to_string()),
                );
                false
            }
            CrawlPermission::TooLate => {
                notice(output, process_numbers::WEB_TIME_LIMIT, None);
                false
            }
        }
    }

    pub async fn gather(&self, question: &str, needs_web: bool, events: &EventSink) -> GatheredWeb {
        let deadline = Instant::now() + self.config.fast_web_timeout;
        let mut output = GatheredWeb::default();
        let mut ledger = EvidenceLedger::default();
        let mut pages = FuturesUnordered::new();
        let mut seen = HashSet::new();
        let mut scheduled = 0;
        let mut collected = vec![];
        let mut robots = Robots::default();

        if needs_web {
            let provider_pages = gather_routed_providers(
                &self.providers,
                &self.politeness,
                &self.shield,
                question,
                WebMode::Fast,
                self.config.fast_web_timeout.as_millis() as u64,
                &mut output,
                &mut ledger,
                events,
            )
            .await;
            for page in provider_pages {
                let idx = scheduled;
                scheduled += 1;
                seen.insert(page.source.url.clone());
                collected.push((idx, page));
            }
        }

        let pasted = pasted_urls(question);
        let mut page_limit_notice = false;
        let mut initial_urls = vec![];
        for value in &pasted {
            // Reject credentials before emitting URL params. Neither credentials
            // nor the forbidden host can reach the client or the progress stream.
            let Ok(url) = page_url(value) else {
                notice(&mut output, process_numbers::WEB_PAGE_FAILED, None);
                continue;
            };
            let url = String::from(url);
            if !seen.insert(url.clone()) {
                continue;
            }
            if scheduled == self.config.fast_web_pages {
                page_limit_notice = true;
                continue;
            }
            if self
                .allow_robots(&mut robots, &url, deadline, &mut output)
                .await
            {
                initial_urls.push((scheduled, url));
                scheduled += 1;
            }
        }
        let initial_total = if needs_web {
            None
        } else {
            Some(initial_urls.len().min(self.config.fast_web_pages) as u64)
        };
        for (idx, url) in initial_urls.into_iter() {
            self.schedule(idx, url, initial_total, events, &mut pages);
        }
        if page_limit_notice {
            notice(&mut output, process_numbers::WEB_PAGE_LIMIT, None);
        }
        let mut search = if needs_web {
            let mut step = StepProgress::new(StageName::Gather, StepName::WebSearch);
            step.params.insert("query".into(), question.into());
            events.send(NativeEvent::Step(step));
            Some(Box::pin(self.search.search(question)))
        } else {
            None
        };
        while search.is_some() || !pages.is_empty() {
            tokio::select! {
                _=tokio::time::sleep_until(deadline)=>{notice(&mut output,process_numbers::WEB_TIME_LIMIT,None);break;}
                results=async{search.as_mut().expect("pending search").await},if search.is_some()=>{
                    search=None;if let Some(process)=results.notice{notice(&mut output,process,None);}
                    let mut candidates = vec![];
                    for hit in results.hits {
                        if let Ok(url) = page_url(&hit.hit.url) {
                            let url_str = String::from(url);
                            let host = page_url(&url_str)
                                .ok()
                                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                                .unwrap_or_default();
                            ledger.record(EvidenceItem {
                                canonical_url: url_str.clone(),
                                publication_id: None,
                                title: hit.hit.title,
                                source_host: host,
                                stage: EvidenceStage::FoundLink,
                            });
                            if seen.insert(url_str.clone()) {
                                candidates.push(url_str);
                            }
                        }
                    }
                    let mut allowed = vec![];
                    for url in candidates.into_iter().take(self.config.fast_web_pages.saturating_sub(scheduled)) {
                        if self.allow_robots(&mut robots, &url, deadline, &mut output).await {
                            allowed.push((scheduled, url));
                            scheduled += 1;
                        }
                    }
                    let total = scheduled;
                    for (idx, url) in allowed.into_iter() {
                        self.schedule(idx, url, Some(total as u64), events, &mut pages);
                    }
                }
                Some((index,url,result))=pages.next(),if !pages.is_empty()=>match result{
                    Ok((page,notices))=>{
                        output.pages_read+=1;
                        for process in notices{if process.number!=process_numbers::SECURITY_BLOCK.number{notice(&mut output,process,Some(url.clone()));}}
                        if !page.chunks.is_empty(){
                            let host = page_url(&page.source.url)
                                .ok()
                                .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                                .unwrap_or_default();
                            ledger.record(EvidenceItem {
                                canonical_url: page.source.url.clone(),
                                publication_id: None,
                                title: page.source.title.clone(),
                                source_host: host,
                                stage: EvidenceStage::ReadContent,
                            });
                            collected.push((index,page));
                            collected.sort_by_key(|(index,_)|*index);
                            events.send(NativeEvent::Sources(collected.iter().map(|(_,p)|p.source.clone()).collect()));
                        }
                    }
                    Err(error)=>record_policed_error(&mut output, url, error),
                }
            }
        }
        // Dropping pending page futures stops HTTP work at the global deadline.
        drop(pages);
        drop(search);
        select(
            &self.config,
            question,
            deadline,
            collected.into_iter().map(|(_, page)| page).collect(),
            &mut output,
            &mut ledger,
            events,
        )
        .await;
        output.evidence = ledger.all_items().to_vec();
        output
    }
}

fn label_source_title(title: &str, stage: EvidenceStage) -> String {
    if title.starts_with('[') {
        title.to_string()
    } else {
        format!("[{}] {}", stage.label(), title)
    }
}

pub(crate) async fn select(
    config: &Config,
    question: &str,
    deadline: Instant,
    mut collected: Vec<WebPage>,
    output: &mut GatheredWeb,
    ledger: &mut EvidenceLedger,
    events: &EventSink,
) {
    let chunks: Vec<_> = collected
        .iter_mut()
        .flat_map(|page| std::mem::take(&mut page.chunks))
        .collect();
    let candidates = chunks.len();
    let ranked = if Instant::now() < deadline && !chunks.is_empty() {
        let question = question.to_owned();
        let input = chunks.clone();
        let task = tokio::task::spawn_blocking(move || rank(&question, input));
        match tokio::time::timeout_at(deadline, task).await {
            Ok(Ok(ranked)) => ranked.into_iter().map(|r| r.chunk).collect(),
            _ => {
                if !output
                    .notices
                    .iter()
                    .any(|n| n.process.number == process_numbers::WEB_TIME_LIMIT.number)
                {
                    notice(output, process_numbers::WEB_TIME_LIMIT, None);
                }
                chunks
            }
        }
    } else {
        chunks
    };
    let mut per_page = HashMap::<Option<String>, usize>::new();
    for chunk in ranked {
        if output.texts.len() == config.context_chunk_count {
            break;
        }
        let count = per_page.entry(chunk.source.clone()).or_default();
        if *count < config.chunks_per_page {
            *count += 1;
            if let Some(ref src) = chunk.source {
                ledger.promote_used(src);
            }
            output.texts.push(chunk);
        }
    }
    let mut cited_urls = HashSet::new();
    output.sources = collected
        .into_iter()
        .map(|p| p.source)
        .filter(|source| {
            output
                .texts
                .iter()
                .any(|t| t.source.as_deref() == Some(source.url.as_str()))
        })
        .filter_map(|mut source| {
            let stage = ledger.stage_for_url(&source.url);
            let citable = stage.is_none_or(EvidenceStage::is_citable);
            if !citable {
                return None;
            }
            let canon = canonicalize_url(&source.url);
            if !cited_urls.insert(canon) {
                return None;
            }
            if let Some(st) = stage {
                source.title = label_source_title(&source.title, st);
            }
            Some(source)
        })
        .collect();
    if candidates > 0 {
        let mut step = StepProgress::new(StageName::Gather, StepName::ChunksSelected);
        step.progress = Some([output.texts.len() as u64, candidates as u64]);
        events.send(NativeEvent::Step(step));
        events.send(NativeEvent::Sources(output.sources.clone()));
    }
}
