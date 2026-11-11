use crate::{
    core::{config::Config, process_numbers},
    dal::remote::hippocampus::Hippocampus,
    domain::{
        models::{
            context::{AssembledContext, ContextInput, ContextText},
            events::{Memory, NativeEvent, StepName, StepProgress},
            pipeline::{SourceNotice, StageName},
            recall::Recalled,
            storage::AppUser,
        },
        services::{
            chunks::{rank, split},
            context::assemble,
            events::EventSink,
            security::SecurityShield,
        },
    },
};
use std::{collections::HashSet, sync::Arc};

#[derive(Clone)]
pub struct MemoryRecall {
    client: Hippocampus,
    shield: Arc<SecurityShield>,
    config: Config,
}
impl MemoryRecall {
    pub fn new(config: &Config, client: Hippocampus, shield: Arc<SecurityShield>) -> Self {
        Self {
            client,
            shield,
            config: config.clone(),
        }
    }
    pub async fn gather(
        &self,
        scope: &AppUser,
        question: &str,
        use_memory: bool,
        topic: Option<&str>,
        events: &EventSink,
    ) -> Recalled {
        let mut result = Recalled::default();
        if !use_memory {
            return result;
        }
        events.send(NativeEvent::Step(StepProgress::new(
            StageName::Gather,
            StepName::MemorySearch,
        )));
        let memories = match self
            .client
            .search(scope, question, topic, self.config.memory_context_count)
            .await
        {
            Ok(memories) => memories,
            Err(_) => {
                result.notices.push(SourceNotice {
                    process: process_numbers::MEMORY_LOAD_FAILED,
                    label: "memory".into(),
                    source: None,
                });
                return result;
            }
        };
        let mut seen = HashSet::new();
        for memory in memories {
            if !seen.insert(memory.id.clone()) {
                continue;
            }
            let text = memory.content.unwrap_or_default();
            if text.trim().is_empty() {
                continue;
            }
            let reference = Memory {
                id: memory.id.clone(),
                title: memory.title.clone(),
            };
            let chunks = if memory.memory_type == "file" {
                split(&self.config, &text)
                    .into_iter()
                    .map(|text| ContextText {
                        label: format!("file: {}", memory.title),
                        source: Some(memory.id.clone()),
                        text,
                    })
                    .collect()
            } else {
                vec![ContextText {
                    label: format!("memory: {}", memory.title),
                    source: Some(memory.id.clone()),
                    text,
                }]
            };
            let (safe, notices) = self.shield.filter_chunks(chunks).await;
            for process in notices {
                if process.number != process_numbers::SECURITY_BLOCK.number {
                    result.notices.push(SourceNotice {
                        process,
                        label: "memory".into(),
                        source: None,
                    });
                }
            }
            if !safe.is_empty() {
                result.references.push(reference);
            }
            if memory.memory_type == "file" {
                result.files.extend(safe);
            } else {
                result.memories.extend(safe);
            }
        }
        // Rank only file chunks. The order of normal memories stays unchanged.
        let question = question.to_owned();
        let files = std::mem::take(&mut result.files);
        result.files = tokio::task::spawn_blocking(move || rank(&question, files))
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|r| r.chunk)
            .collect();
        // Apply the same encoded 1500-token limit as the final context assembler.
        if let Ok(bounded) = assemble(
            &self.config,
            ContextInput {
                memories: std::mem::take(&mut result.memories),
                ..Default::default()
            },
        ) {
            // The assembler orders the final context at its edges. Restore Hippo
            // order here so the final assembler applies that ordering only once.
            let mut texts = bounded.texts;
            for reference in &result.references {
                if let Some(index) = texts
                    .iter()
                    .position(|t| t.source.as_deref() == Some(&reference.id))
                {
                    result.memories.push(texts.remove(index));
                }
            }
        }
        result.references.retain(|reference| {
            result
                .memories
                .iter()
                .chain(&result.files)
                .any(|text| text.source.as_deref() == Some(&reference.id))
        });
        let mut step = StepProgress::new(StageName::Gather, StepName::MemoriesFound);
        step.progress = Some([
            result.references.len() as u64,
            result.references.len() as u64,
        ]);
        events.send(NativeEvent::Step(step));
        result
    }
}
impl Recalled {
    /// Publish only records whose text survives the final shared context budget.
    pub fn used(&self, context: &AssembledContext, events: &EventSink) -> Vec<Memory> {
        let used: Vec<_> = self
            .references
            .iter()
            .filter(|m| {
                context
                    .texts
                    .iter()
                    .any(|t| t.source.as_deref() == Some(&m.id))
            })
            .cloned()
            .collect();
        events.send(NativeEvent::Memories(used.clone()));
        used
    }
}
