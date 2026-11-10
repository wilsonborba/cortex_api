use serde::{Deserialize, Serialize};
/// Only the context encoder serializes this text. Never log it.
#[derive(Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ContextText {
    pub label: String,
    pub source: Option<String>,
    pub text: String,
}
#[derive(Clone)]
pub struct Exchange {
    pub input: String,
    pub output: String,
}
/// Inputs are already ranked by their source. Personas contain only schema
/// fields marked in_context. History has only input and final answer text.
#[derive(Default)]
pub struct ContextInput {
    pub question: String,
    pub thinking: bool,
    pub history: Vec<Exchange>, // newest first
    pub self_persona: Option<ContextText>,
    pub bot_persona: Option<ContextText>,
    pub named_personas: Vec<ContextText>,
    pub files: Vec<ContextText>,
    pub memories: Vec<ContextText>,
    pub web: Vec<ContextText>,
}
pub struct AssembledContext {
    pub system: String,
    pub prompt: String,
    pub system_personas: Vec<ContextText>,
    pub texts: Vec<ContextText>,
    pub history: Vec<Exchange>,
    pub estimated_input_tokens: u64,
    pub reserved_output_tokens: u64,
    pub reserved_notes_tokens: u64,
}
