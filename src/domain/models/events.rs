use crate::{
    core::process_numbers::ProcessNumber,
    domain::models::{pipeline::StageName, run_record::StageRecord, usage::Usage},
};
use serde::Serialize;
use std::collections::BTreeMap;
use uuid::Uuid;
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum StepName {
    #[serde(rename = "web.queries")]
    WebQueries,
    #[serde(rename = "queue.wait")]
    QueueWait,
    #[serde(rename = "model.load")]
    ModelLoad,
    #[serde(rename = "model.ready")]
    ModelReady,
    #[serde(rename = "security.ok")]
    SecurityOk,
    #[serde(rename = "file.read.page")]
    FileReadPage,
    #[serde(rename = "file.ocr.page")]
    OcrPage,
    #[serde(rename = "file.transcribe")]
    Transcribe,
    #[serde(rename = "memory.search")]
    MemorySearch,
    #[serde(rename = "memory.found")]
    MemoriesFound,
    #[serde(rename = "web.search")]
    WebSearch,
    #[serde(rename = "web.page.read")]
    WebPageRead,
    #[serde(rename = "chunks.selected")]
    ChunksSelected,
    #[serde(rename = "thinking.step")]
    ThinkingStep,
    #[serde(rename = "thinking.stopped")]
    ThinkingStopped,
}
impl StepName {
    pub fn message(self) -> &'static str {
        match self {
            Self::WebQueries => "Preparing search queries",
            Self::QueueWait => "Waiting for the model",
            Self::ModelLoad => "Loading the model",
            Self::ModelReady => "The model is ready",
            Self::SecurityOk => "Security check OK",
            Self::FileReadPage => "Reading the file page",
            Self::OcrPage => "Reading the page image",
            Self::Transcribe => "Transcribing audio",
            Self::MemorySearch => "Searching your memory",
            Self::MemoriesFound => "Memories found",
            Self::WebSearch => "Searching the web",
            Self::WebPageRead => "Reading the web page",
            Self::ChunksSelected => "Text parts selected",
            Self::ThinkingStep => "Thinking step",
            Self::ThinkingStopped => "Thinking stopped by the user",
        }
    }
}
#[derive(Clone, Serialize)]
pub struct StepProgress {
    pub stage: StageName,
    pub step: StepName,
    pub params: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<[u64; 2]>,
    pub elapsed_ms: u64,
}
impl StepProgress {
    pub fn new(stage: StageName, step: StepName) -> Self {
        Self {
            stage,
            step,
            params: Default::default(),
            progress: None,
            elapsed_ms: 0,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct StageProgress {
    pub stage: StageName,
    pub status: String,
    pub time_ms: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_number: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice_number: Option<u16>,
    /// Present on the first stage event. Null means no completed runs exist.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_ms: Option<Option<u64>>,
}
impl StageProgress {
    pub fn start(stage: StageName) -> Self {
        Self {
            stage,
            status: "started".into(),
            time_ms: 0,
            error_number: None,
            notice_number: None,
            expected_ms: None,
        }
    }
    pub fn end(stage: StageName, record: StageRecord) -> Self {
        Self {
            stage,
            status: record.status,
            time_ms: record.time_ms,
            error_number: record.error_number,
            notice_number: record.notice_number,
            expected_ms: None,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct Source {
    pub title: String,
    pub url: String,
}
#[derive(Clone, Serialize)]
pub struct Memory {
    pub id: String,
    pub title: String,
}
#[derive(Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingRole {
    Plan,
    Work,
    Check,
}
#[derive(Clone)]
pub enum CallStreamRole {
    Answer,
    Thinking { role: ThinkingRole, round: u8 },
    Queries,
}
#[derive(Clone)]
pub enum NativeEvent {
    Stage(StageProgress),
    Step(StepProgress),
    Sources(Vec<Source>),
    Memories(Vec<Memory>),
    Thinking {
        role: ThinkingRole,
        round: u8,
        text: String,
    },
    Token(String),
    Notice(ProcessNumber),
    Error(ProcessNumber),
    Done {
        session_id: Uuid,
        output_id: Uuid,
        tokens: Usage,
        total_ms: u64,
    },
}
impl NativeEvent {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Stage(_) => "stage",
            Self::Step(_) => "step",
            Self::Sources(_) => "sources",
            Self::Memories(_) => "memories",
            Self::Thinking { .. } => "thinking",
            Self::Token(_) => "token",
            Self::Notice(_) => "notice",
            Self::Error(_) => "error",
            Self::Done { .. } => "done",
        }
    }
    pub fn message(&self) -> &str {
        match self {
            Self::Stage(s) => {
                if s.status == "started" {
                    "Stage started"
                } else {
                    "Stage completed"
                }
            }
            Self::Step(s) => s.step.message(),
            Self::Sources(_) => "Sources found",
            Self::Memories(_) => "Memories used",
            Self::Thinking { .. } => "Thinking",
            Self::Token(_) => "Answer text",
            Self::Notice(p) | Self::Error(p) => p.message,
            Self::Done { .. } => "Output complete",
        }
    }
    pub fn data(&self) -> serde_json::Value {
        use serde_json::{json, to_value};
        match self {
            Self::Stage(s) => to_value(s).expect("stage serializes"),
            Self::Step(s) => to_value(s).expect("step serializes"),
            Self::Sources(s) => json!({"sources":s}),
            Self::Memories(m) => json!({"memories":m}),
            Self::Thinking { role, round, text } => json!({"role":role,"round":round,"text":text}),
            Self::Token(text) => json!({"text":text}),
            Self::Notice(p) => json!({"notice_number":p.number}),
            Self::Error(p) => json!({"error_number":p.number}),
            Self::Done {
                session_id,
                output_id,
                tokens,
                total_ms,
            } => {
                json!({"session_id":session_id,"output_id":output_id,"tokens":{"input":tokens.input,"output":tokens.output,"total":tokens.input.saturating_add(tokens.output)},"total_ms":total_ms})
            }
        }
    }
    pub fn public(&self) -> bool {
        !matches!(
            self,
            Self::Stage(StageProgress {
                stage: StageName::Save,
                ..
            }) | Self::Step(StepProgress {
                stage: StageName::Save,
                ..
            })
        )
    }
}
#[derive(Clone)]
pub struct OutputEvent {
    pub id: u64,
    pub event: NativeEvent,
}
