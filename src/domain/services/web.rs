use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::remote::web_page::{PageReader, page_url},
    domain::{
        models::{
            events::{NativeEvent, StepName, StepProgress},
            pipeline::{SourceNotice, StageName},
            web::GatheredWeb,
            web_page::{PageFailure, PageFetchOutcome, WebPage},
        },
        services::{
            chunks::{pasted_urls, rank},
            events::EventSink,
            research_limits::{
                CachedWebPage, FetchDecision, InMemoryLimitStore, ResearchPageCache,
                ResearchPolitenessGuard, evaluate_publisher_policy,
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

#[derive(Clone)]
pub struct FastWeb {
    search: WebSearch,
    reader: PageReader,
    shield: Arc<SecurityShield>,
    politeness: Arc<ResearchPolitenessGuard>,
    page_cache: Arc<ResearchPageCache>,
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
        let mut pages = FuturesUnordered::new();
        let mut seen = HashSet::new();
        let mut scheduled = 0;
        let mut collected = vec![];
        let mut robots = Robots::default();
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
                    let candidates:Vec<_>=results.hits.into_iter().filter_map(|hit|page_url(&hit.hit.url).ok()).map(String::from).filter(|url|seen.insert(url.clone())).take(self.config.fast_web_pages.saturating_sub(scheduled)).collect();
                    let mut allowed = vec![];
                    for url in candidates {
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
                    Ok((page,notices))=>{output.pages_read+=1;for process in notices{if process.number!=process_numbers::SECURITY_BLOCK.number{notice(&mut output,process,Some(url.clone()));}}
                        if !page.chunks.is_empty(){collected.push((index,page));collected.sort_by_key(|(index,_)|*index);events.send(NativeEvent::Sources(collected.iter().map(|(_,p)|p.source.clone()).collect()));}
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
            events,
        )
        .await;
        output
    }
}

pub(crate) async fn select(
    config: &Config,
    question: &str,
    deadline: Instant,
    mut collected: Vec<WebPage>,
    output: &mut GatheredWeb,
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
            output.texts.push(chunk);
        }
    }
    output.sources = collected
        .into_iter()
        .map(|p| p.source)
        .filter(|source| {
            output
                .texts
                .iter()
                .any(|t| t.source.as_deref() == Some(source.url.as_str()))
        })
        .collect();
    if candidates > 0 {
        let mut step = StepProgress::new(StageName::Gather, StepName::ChunksSelected);
        step.progress = Some([output.texts.len() as u64, candidates as u64]);
        events.send(NativeEvent::Step(step));
        events.send(NativeEvent::Sources(output.sources.clone()));
    }
}
