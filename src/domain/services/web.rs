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
            web_page::{PageFailure, WebPage},
        },
        services::{
            chunks::{pasted_urls, rank},
            events::EventSink,
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
type PageResult = (
    usize,
    String,
    Result<(WebPage, Vec<ProcessNumber>), PageFailure>,
);
#[derive(Clone)]
pub struct FastWeb {
    search: WebSearch,
    reader: PageReader,
    shield: Arc<SecurityShield>,
    config: Config,
}
fn notice(output: &mut GatheredWeb, process: ProcessNumber, source: Option<String>) {
    // The stage reporter emits these once with the final Gather result.
    output.notices.push(SourceNotice {
        process,
        label: "web".into(),
        source,
    });
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
            config: config.clone(),
        }
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
            let result = async {
                let mut page = self.reader.read(&url).await?;
                let (safe, notices) = self.shield.filter_chunks(page.chunks).await;
                page.chunks = safe;
                Ok((page, notices))
            }
            .await;
            (index, url, result)
        }));
    }
    pub async fn gather(&self, question: &str, needs_web: bool, events: &EventSink) -> GatheredWeb {
        let deadline = Instant::now() + self.config.fast_web_timeout;
        let mut output = GatheredWeb::default();
        let mut pages = FuturesUnordered::new();
        let mut seen = HashSet::new();
        let mut scheduled = 0;
        let mut collected = vec![];
        let pasted = pasted_urls(question);
        let mut page_limit_notice = false;
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
            self.schedule(
                scheduled,
                url,
                if needs_web {
                    None
                } else {
                    Some(pasted.len().min(self.config.fast_web_pages) as u64)
                },
                events,
                &mut pages,
            );
            scheduled += 1;
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
                    let candidates:Vec<_>=results.hits.into_iter().filter_map(|hit|page_url(&hit.hit.url).ok()).map(String::from).filter(|url|seen.insert(url.clone())).take(self.config.fast_web_pages.saturating_sub(scheduled)).collect();let total=scheduled+candidates.len();
                    for url in candidates{self.schedule(scheduled,url,Some(total as u64),events,&mut pages);scheduled+=1;}
                }
                Some((index,url,result))=pages.next(),if !pages.is_empty()=>match result{
                    Ok((page,notices))=>{output.pages_read+=1;for process in notices{if process.number!=process_numbers::SECURITY_BLOCK.number{notice(&mut output,process,Some(url.clone()));}}
                        if !page.chunks.is_empty(){collected.push((index,page));collected.sort_by_key(|(index,_)|*index);events.send(NativeEvent::Sources(collected.iter().map(|(_,p)|p.source.clone()).collect()));}
                    }
                    Err(error)=>notice(&mut output,error.process(),Some(url)),
                }
            }
        }
        // Dropping pending page futures stops HTTP work at the global deadline.
        drop(pages);
        drop(search);
        let chunks: Vec<_> = collected
            .iter_mut()
            .flat_map(|(_, page)| std::mem::take(&mut page.chunks))
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
                        notice(&mut output, process_numbers::WEB_TIME_LIMIT, None);
                    }
                    chunks
                }
            }
        } else {
            chunks
        };
        let mut per_page = HashMap::<Option<String>, usize>::new();
        for chunk in ranked {
            if output.texts.len() == self.config.context_chunk_count {
                break;
            }
            let count = per_page.entry(chunk.source.clone()).or_default();
            if *count < self.config.chunks_per_page {
                *count += 1;
                output.texts.push(chunk);
            }
        }
        output.sources = collected
            .into_iter()
            .map(|(_, p)| p.source)
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
        output
    }
}
