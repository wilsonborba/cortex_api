use crate::{
    core::config::Config,
    domain::{
        models::{
            context::ContextInput,
            driver::{CallInput, DriverError, ModelDriver},
            events::{CallStreamRole, NativeEvent, StepName, StepProgress},
            pipeline::StageName,
            queries::*,
            run_record::WebMode,
        },
        services::{context::assemble, events::EventSink},
    },
};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};
/// The query contract is plain lines. Structured JSON and fence markers are
/// not queries. A blank or unusable result falls back to the original input.
pub fn parse(text: &str, question: &str, limit: usize) -> Vec<String> {
    let value = text.trim();
    if value.starts_with(['[', '{']) {
        return vec![question.into()];
    }
    let mut lines = vec![];
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        if line.starts_with(['[', '{', '}', ']']) {
            continue;
        }
        lines.push(line.to_owned());
        if lines.len() == limit.max(1) {
            break;
        }
    }
    if lines.is_empty() {
        vec![question.into()]
    } else {
        lines
    }
}
#[derive(Clone)]
pub struct QueryRewriter {
    driver: Arc<dyn ModelDriver>,
    config: Config,
}
impl QueryRewriter {
    pub fn new(config: &Config, driver: Arc<dyn ModelDriver>) -> Self {
        Self {
            driver,
            config: config.clone(),
        }
    }
    pub async fn rewrite(
        &self,
        input: QueryInput<'_>,
        events: &EventSink,
        cancel: watch::Receiver<bool>,
    ) -> QueryResult {
        let limit = match input.mode {
            WebMode::Fast => {
                return QueryResult {
                    queries: vec![input.question.into()],
                    call: None,
                    error: None,
                };
            }
            WebMode::Medium => self.config.medium_query_count,
            WebMode::Deep => self.config.deep_query_count,
        };
        if *cancel.borrow() {
            return QueryResult {
                queries: vec![],
                call: None,
                error: Some(DriverError::Cancelled),
            };
        }
        let assembled = match assemble(
            &self.config,
            ContextInput {
                question: input.question.into(),
                ..Default::default()
            },
        ) {
            Ok(context) => context,
            Err(_) => {
                return QueryResult {
                    queries: vec![],
                    call: None,
                    error: Some(DriverError::InvalidInput),
                };
            }
        };
        // Keep the selected data format and repeated plain question. Only the
        // role's task instruction changes; it requests queries, never an answer.
        let system = self
            .config
            .query_instruction
            .replace("{limit}", &limit.to_string());
        let budget = self
            .config
            .estimate_tokens(&system)
            .saturating_add(self.config.estimate_tokens(&assembled.prompt))
            .saturating_add(self.config.chat_template_token_reserve)
            .saturating_add(u64::from(self.config.query_output_tokens));
        if budget > self.config.model_context_tokens {
            return QueryResult {
                queries: vec![],
                call: None,
                error: Some(DriverError::InvalidInput),
            };
        }
        let mut step = StepProgress::new(StageName::Gather, StepName::WebQueries);
        step.params.insert("limit".into(), limit.into());
        events.send(NativeEvent::Step(step));
        let (tx, mut rx) = mpsc::unbounded_channel();
        let sink = events.clone();
        let forward = tokio::spawn(async move {
            while let Some(event) = rx.recv().await {
                sink.driver(StageName::Gather, CallStreamRole::Queries, event);
            }
        });
        let outcome = self
            .driver
            .call(
                input.model,
                CallInput {
                    role: "queries".into(),
                    system,
                    prompt: assembled.prompt,
                    images: input.images,
                    temperature: self.config.query_temperature,
                    max_tokens: self.config.query_output_tokens,
                },
                tx,
                cancel,
            )
            .await;
        // A driver releases its sender before returning. Drain load/wait events.
        let _ = forward.await;
        match outcome {
            Ok(outcome) => {
                let queries = if outcome.error.is_none() {
                    parse(&outcome.text, input.question, limit)
                } else {
                    vec![]
                };
                QueryResult {
                    queries,
                    call: Some(outcome.usage),
                    error: outcome.error,
                }
            }
            Err(error) => QueryResult {
                queries: vec![],
                call: None,
                error: Some(error),
            },
        }
    }
}
