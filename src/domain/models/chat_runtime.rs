use crate::domain::models::{
    chat_session::{InputIds, NewChat, NextInput},
    context::{AssembledContext, ContextText, Exchange},
    driver::OutputModel,
    events::{Memory, Source},
    identity::Caller,
    run_record::WebMode,
    thinking::ThinkingResult,
};
use std::sync::Arc;
use uuid::Uuid;
/// One input of `POST /chats` or `POST /chats/{session_id}/inputs`.
pub enum TurnRequest {
    New(NewChat),
    Next(Uuid, NextInput),
}
/// Facts fixed by Accept. Later stages only read them.
pub struct AcceptedTurn {
    pub ids: InputIds,
    pub temporary: bool,
    pub question: String,
    pub needs_web: bool,
    pub web_mode: WebMode,
    pub use_memory: bool,
    pub memory_topic: Option<String>,
    pub thinking: bool,
    /// The chat bot persona (23.5). Temporary chats use it but never change it.
    pub persona_id: Option<String>,
    /// Base64 images of this input; they select the vision model (2.6).
    pub images: Vec<String>,
    pub model: OutputModel,
    pub history: Vec<Exchange>,
    /// Temporary chat files. Permanent chat files are read in Gather.
    pub temporary_files: Vec<ContextText>,
}
impl AcceptedTurn {
    pub fn with_persona(self, persona_id: Option<String>) -> Self {
        Self { persona_id, ..self }
    }
    pub fn with_images(self, caller: &Caller, images: Vec<String>) -> Self {
        Self {
            model: OutputModel::new(caller, self.ids.output_id, !images.is_empty()),
            images,
            ..self
        }
    }
}
/// Data carried between stages. Full text never goes to logs or run records.
#[derive(Clone)]
pub struct Turn {
    pub caller: Caller,
    pub request: Arc<TurnRequest>,
    pub accepted: Option<Arc<AcceptedTurn>>,
    pub files: Vec<ContextText>,
    pub memories: Vec<ContextText>,
    pub references: Vec<Memory>,
    pub web: Vec<ContextText>,
    pub sources: Vec<Source>,
    pub pages_read: u32,
    pub context: Option<Arc<AssembledContext>>,
    pub used_memories: Vec<Memory>,
    pub thinking: Option<Arc<ThinkingResult>>,
    pub answer: String,
    pub personas: crate::domain::services::personas::PersonaContext,
}
impl Turn {
    pub fn new(caller: Caller, request: TurnRequest) -> Self {
        Self {
            caller,
            request: Arc::new(request),
            accepted: None,
            files: vec![],
            memories: vec![],
            references: vec![],
            web: vec![],
            sources: vec![],
            pages_read: 0,
            context: None,
            used_memories: vec![],
            thinking: None,
            answer: String::new(),
            personas: Default::default(),
        }
    }
    pub fn accepted(&self) -> &AcceptedTurn {
        self.accepted.as_deref().expect("stage after Accept")
    }
}
impl crate::domain::models::pipeline::PipelineData for Turn {
    fn thinking(&self) -> bool {
        self.accepted.as_ref().is_some_and(|a| a.thinking)
    }
    fn merge_gather(previous: &Self, files: Self, memory: Self, web: Self) -> Self {
        let mut data = previous.clone();
        // Files first, because the user sent them (7.2). Recalled file memories
        // are file chunks too (8.5).
        data.files = files.files;
        data.files.extend(memory.files);
        data.memories = memory.memories;
        data.personas = memory.personas;
        data.references = memory.references;
        data.web = web.web;
        data.sources = web.sources;
        data.pages_read = web.pages_read;
        data
    }
}
