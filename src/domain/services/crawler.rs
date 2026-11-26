use crate::{
    core::{config::Config, process_numbers},
    dal::remote::web_page::{PageReader, page_url},
    domain::{
        models::{
            context::ContextText,
            driver::{DriverError, ModelDriver},
            events::{NativeEvent, StepName, StepProgress},
            pipeline::StageName,
            queries::QueryInput,
            run_record::WebMode,
            web::*,
            web_page::*,
        },
        services::{
            chunks::{pasted_urls, rank},
            events::EventSink,
            queries::QueryRewriter,
            research_limits::{
                FetchDecision, InMemoryLimitStore, ResearchPageCache, ResearchPolitenessGuard,
                evaluate_publisher_policy,
            },
            research_router::{EvidenceItem, EvidenceLedger, EvidenceStage},
            robots::{CrawlPermission, Robots},
            search::WebSearch,
            security::SecurityShield,
            web::{
                FastWeb, PolicedPageError, ResearchProviders, gather_routed_providers, notice,
                read_policed_page, record_policed_error, select,
            },
        },
    },
};
use futures_util::{StreamExt, stream::FuturesUnordered};
use std::{collections::HashSet, sync::Arc};
use tokio::{sync::watch, time::Instant};
pub(crate) async fn cancelled(mut signal: watch::Receiver<bool>) {
    loop {
        if *signal.borrow() {
            return;
        }
        if signal.changed().await.is_err() {
            std::future::pending::<()>().await;
        }
    }
}
fn time_notice(output: &mut GatheredWeb) {
    if !output
        .notices
        .iter()
        .any(|n| n.process.number == process_numbers::WEB_TIME_LIMIT.number)
    {
        notice(output, process_numbers::WEB_TIME_LIMIT, None);
    }
}
#[derive(Clone)]
pub struct WebRetriever {
    fast: FastWeb,
    search: WebSearch,
    reader: PageReader,
    queries: QueryRewriter,
    shield: Arc<SecurityShield>,
    politeness: Arc<ResearchPolitenessGuard>,
    page_cache: Arc<ResearchPageCache>,
    providers: ResearchProviders,
    enforce_robots_direct: bool,
    config: Config,
}
impl WebRetriever {
    pub fn new(
        config: &Config,
        search: WebSearch,
        reader: PageReader,
        shield: Arc<SecurityShield>,
        driver: Arc<dyn ModelDriver>,
    ) -> Self {
        let politeness = Arc::new(ResearchPolitenessGuard::from_config(
            config,
            Arc::new(InMemoryLimitStore::default()),
        ));
        let page_cache = Arc::new(ResearchPageCache::in_memory(config.research_cache_ttl));
        Self {
            fast: FastWeb::new(config, search.clone(), reader.clone(), shield.clone())
                .with_politeness(politeness.clone(), page_cache.clone()),
            search,
            reader,
            queries: QueryRewriter::new(config, driver),
            shield,
            politeness,
            page_cache,
            providers: ResearchProviders::default(),
            enforce_robots_direct: false,
            config: config.clone(),
        }
    }

    pub fn with_politeness(
        mut self,
        politeness: Arc<ResearchPolitenessGuard>,
        page_cache: Arc<ResearchPageCache>,
    ) -> Self {
        self.politeness = politeness.clone();
        self.page_cache = page_cache.clone();
        self.fast = self.fast.with_politeness(politeness, page_cache);
        self
    }

    pub fn with_providers(mut self, providers: ResearchProviders) -> Self {
        self.providers = providers.clone();
        self.fast = self.fast.with_providers(providers);
        self
    }

    pub fn with_robots_on_direct_reads(mut self, enforce: bool) -> Self {
        self.enforce_robots_direct = enforce;
        self.fast = self.fast.with_robots(enforce);
        self
    }
    pub async fn gather(
        &self,
        input: WebInput<'_>,
        events: &EventSink,
        cancel: watch::Receiver<bool>,
    ) -> WebResult {
        let mut result = WebResult {
            gathered: GatheredWeb::default(),
            calls: vec![],
            error: None,
        };
        if *cancel.borrow() {
            result.error = Some(DriverError::Cancelled);
            return result;
        }
        if input.mode == WebMode::Fast {
            tokio::select! {gathered=self.fast.gather(input.question,input.needs_web,events)=>result.gathered=gathered,_=cancelled(cancel)=>result.error=Some(DriverError::Cancelled)}
            return result;
        }
        let (depth, budget) = match input.mode {
            WebMode::Medium => (self.config.medium_web_depth, self.config.medium_web_timeout),
            WebMode::Deep => (self.config.deep_web_depth, self.config.deep_web_timeout),
            WebMode::Fast => unreachable!(),
        };
        let deadline = Instant::now() + budget;
        let mut ledger = EvidenceLedger::default();
        let mut all_pages = vec![];
        if input.needs_web {
            let provider_pages = gather_routed_providers(
                &self.providers,
                &self.politeness,
                &self.shield,
                input.question,
                input.mode,
                budget.as_millis() as u64,
                &mut result.gathered,
                &mut ledger,
                events,
            )
            .await;
            all_pages.extend(provider_pages);
        }
        let mut search_queries = vec![];
        if input.needs_web {
            let (stop, signal) = watch::channel(false);
            let query = self.queries.rewrite(
                QueryInput {
                    mode: input.mode,
                    question: input.question,
                    model: input.model,
                    images: input.images,
                },
                events,
                signal,
            );
            tokio::pin!(query);
            let (outcome, timed_out, user_cancelled) = tokio::select! {
                outcome=&mut query=>(outcome,false,false),
                _=tokio::time::sleep_until(deadline)=>{let _=stop.send(true);(query.await,true,false)},
                _=cancelled(cancel.clone())=>{let _=stop.send(true);(query.await,false,true)},
            };
            if let Some(call) = outcome.call {
                result.calls.push(call);
            }
            if user_cancelled {
                result.error = Some(DriverError::Cancelled);
                return result;
            }
            if timed_out {
                time_notice(&mut result.gathered);
                return result;
            }
            if let Some(error) = outcome.error {
                result.error = Some(error);
                return result;
            }
            search_queries = outcome.queries;
        }
        let mut visited = HashSet::new();
        let mut urls = vec![];
        for value in pasted_urls(input.question) {
            match page_url(&value) {
                Ok(url) => {
                    let url = String::from(url);
                    if visited.insert(url.clone()) {
                        if urls.len() < self.config.web_pages_per_level {
                            urls.push(url);
                        } else if !result
                            .gathered
                            .notices
                            .iter()
                            .any(|n| n.process.number == process_numbers::WEB_PAGE_LIMIT.number)
                        {
                            notice(&mut result.gathered, process_numbers::WEB_PAGE_LIMIT, None);
                        }
                    }
                }
                Err(_) => notice(&mut result.gathered, process_numbers::WEB_PAGE_FAILED, None),
            }
        }
        if input.needs_web {
            for query in &search_queries {
                let mut step = StepProgress::new(StageName::Gather, StepName::WebSearch);
                step.params.insert("query".into(), query.clone().into());
                events.send(NativeEvent::Step(step));
            }
            let found = tokio::select! {found=self.search.search_many(&search_queries)=>Some(found),_=tokio::time::sleep_until(deadline)=>{time_notice(&mut result.gathered);None},_=cancelled(cancel.clone())=>{result.error=Some(DriverError::Cancelled);None}};
            let Some(found) = found else { return result };
            if let Some(process) = found.notice {
                notice(&mut result.gathered, process, None);
            }
            for hit in found.hits {
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
                    if urls.len() < self.config.web_pages_per_level
                        && visited.insert(url_str.clone())
                    {
                        urls.push(url_str);
                    }
                }
            }
        }
        let mut robots = Robots::default();
        let mut plans = vec![];
        for url in urls {
            if self.enforce_robots_direct {
                let host = page_url(&url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                    .unwrap_or_else(|| "unknown".into());
                let permission = tokio::select! {
                    permission = robots.check(&self.reader, &self.config, &url, deadline) => Some(permission),
                    _ = tokio::time::sleep_until(deadline) => { time_notice(&mut result.gathered); None },
                    _ = cancelled(cancel.clone()) => { result.error = Some(DriverError::Cancelled); None }
                };
                let Some(permission) = permission else {
                    return result;
                };
                match permission {
                    CrawlPermission::Allowed(start) => plans.push((url, start)),
                    CrawlPermission::Denied => {
                        let decision = evaluate_publisher_policy(&host, None, false);
                        record_policed_error(
                            &mut result.gathered,
                            url,
                            PolicedPageError::Decision { host, decision },
                        );
                    }
                    CrawlPermission::Unavailable => {
                        result
                            .gathered
                            .coverage
                            .record_skipped(&host, format!("{host}: robots.txt unavailable"));
                        notice(
                            &mut result.gathered,
                            process_numbers::WEB_ROBOTS_FAILED,
                            Some(url),
                        );
                    }
                    CrawlPermission::TooLate => time_notice(&mut result.gathered),
                }
            } else {
                plans.push((url, Instant::now()));
            }
        }
        for level in 0..=depth {
            if plans.is_empty() || Instant::now() >= deadline {
                if !plans.is_empty() {
                    time_notice(&mut result.gathered);
                }
                break;
            }
            let (pages, error) = self
                .read_level(
                    plans,
                    level,
                    deadline,
                    events,
                    &mut result.gathered,
                    &mut ledger,
                    cancel.clone(),
                )
                .await;
            if let Some(error) = error {
                result.error = Some(error);
                return result;
            }
            let links: Vec<_> = pages.iter().flat_map(|page| page.links.clone()).collect();
            all_pages.extend(pages);
            if level == depth || Instant::now() >= deadline {
                break;
            }
            let mut candidates = vec![];
            for link in links {
                let Ok(url) = page_url(&link.url) else {
                    continue;
                };
                let url_string = String::from(url.clone());
                if visited.contains(&url_string) {
                    continue;
                }
                let tail = url.path().rsplit('/').next().unwrap_or_default();
                let extension = tail
                    .rsplit_once('.')
                    .map(|(_, extension)| extension.to_ascii_lowercase());
                if extension
                    .as_ref()
                    .is_some_and(|ext| self.config.web_file_extensions.contains(&ext.as_str()))
                {
                    continue;
                }
                candidates.push(ContextText {
                    label: "link".into(),
                    source: Some(url_string),
                    text: link.text,
                });
            }
            let question = input.question.to_owned();
            let ranking = tokio::task::spawn_blocking(move || rank(&question, candidates));
            let ranked = tokio::select! {ranking=ranking=>ranking.unwrap_or_default(),_=tokio::time::sleep_until(deadline)=>{time_notice(&mut result.gathered);break;},_=cancelled(cancel.clone())=>{result.error=Some(DriverError::Cancelled);return result;}};
            plans = vec![];
            let mut selected = 0;
            for candidate in ranked {
                if candidate.score <= 0.0 {
                    continue;
                }
                let Some(url) = candidate.chunk.source else {
                    continue;
                };
                if !visited.insert(url.clone()) {
                    continue;
                }
                if selected == self.config.web_pages_per_level {
                    break;
                }
                selected += 1;
                let permission = tokio::select! {permission=robots.check(&self.reader,&self.config,&url,deadline)=>Some(permission),_=tokio::time::sleep_until(deadline)=>{time_notice(&mut result.gathered);None},_=cancelled(cancel.clone())=>{result.error=Some(DriverError::Cancelled);None}};
                let Some(permission) = permission else {
                    break;
                };
                match permission {
                    CrawlPermission::Allowed(start) => plans.push((url, start)),
                    CrawlPermission::Denied => {
                        let host = page_url(&url)
                            .ok()
                            .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                            .unwrap_or_else(|| "unknown".into());
                        record_policed_error(
                            &mut result.gathered,
                            url,
                            PolicedPageError::Decision {
                                host: host.clone(),
                                decision: FetchDecision::BlockedStop {
                                    reason: format!("{host}: disallowed by robots.txt"),
                                },
                            },
                        );
                    }
                    CrawlPermission::Unavailable => notice(
                        &mut result.gathered,
                        process_numbers::WEB_ROBOTS_FAILED,
                        Some(url),
                    ),
                    CrawlPermission::TooLate => time_notice(&mut result.gathered),
                }
            }
            if result.error.is_some() {
                return result;
            }
        }
        select(
            &self.config,
            input.question,
            deadline,
            all_pages,
            &mut result.gathered,
            &mut ledger,
            events,
        )
        .await;
        result.gathered.evidence = ledger.all_items().to_vec();
        result
    }
    #[allow(clippy::too_many_arguments)]
    async fn read_level(
        &self,
        plans: Vec<(String, Instant)>,
        level: usize,
        deadline: Instant,
        events: &EventSink,
        output: &mut GatheredWeb,
        ledger: &mut EvidenceLedger,
        cancel: watch::Receiver<bool>,
    ) -> (Vec<WebPage>, Option<DriverError>) {
        let total = plans.len();
        let mut pending = FuturesUnordered::new();
        for (index, (url, start)) in plans.into_iter().enumerate() {
            let mut step = StepProgress::new(StageName::Gather, StepName::WebPageRead);
            step.params.insert("url".into(), url.clone().into());
            step.params.insert("level".into(), level.into());
            step.progress = Some([index as u64 + 1, total as u64]);
            events.send(NativeEvent::Step(step));
            pending.push(async move {
                tokio::time::sleep_until(start).await;
                let host = page_url(&url)
                    .ok()
                    .and_then(|u| u.host_str().map(str::to_ascii_lowercase))
                    .unwrap_or_else(|| "unknown".into());
                let scope = format!("host:{host}");
                let response = read_policed_page(
                    &self.reader,
                    &self.politeness,
                    &self.page_cache,
                    &self.shield,
                    &url,
                    &scope,
                )
                .await;
                (index, url, response)
            });
        }
        let mut pages = vec![];
        while !pending.is_empty() {
            tokio::select! {
                _=tokio::time::sleep_until(deadline)=>{time_notice(output);break;},
                _=cancelled(cancel.clone())=>return(pages.into_iter().map(|(_,page)|page).collect(),Some(DriverError::Cancelled)),
                Some((index,url,response))=pending.next()=>match response{
                    Ok((page,notices))=>{
                        output.pages_read+=1;
                        for process in notices{if process.number!=process_numbers::SECURITY_BLOCK.number{notice(output,process,Some(url.clone()));}}
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
                            output.sources.push(page.source.clone());
                            events.send(NativeEvent::Sources(output.sources.clone()));
                        }
                        pages.push((index,page));
                    },
                    Err(error)=>record_policed_error(output, url, error),
                }
            }
        }
        pages.sort_by_key(|(index, _)| *index);
        (pages.into_iter().map(|(_, page)| page).collect(), None)
    }
}
