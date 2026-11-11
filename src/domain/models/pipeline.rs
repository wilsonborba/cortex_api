use crate::{
    core::process_numbers::ProcessNumber,
    domain::models::{
        run_record::{ModelCall, StageRecord},
        usage::Pending,
    },
};
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StageName {
    Accept,
    Gather,
    Assemble,
    Think,
    Answer,
    Save,
}
impl StageName {
    pub fn name(self) -> &'static str {
        match self {
            Self::Accept => "accept",
            Self::Gather => "gather",
            Self::Assemble => "assemble",
            Self::Think => "think",
            Self::Answer => "answer",
            Self::Save => "save",
        }
    }
}
#[derive(Clone, Copy)]
pub enum StageStatus {
    Ok,
    Failed(ProcessNumber),
    Notice(ProcessNumber),
    Cancelled,
}
impl StageStatus {
    pub fn terminal(self) -> bool {
        matches!(self, Self::Failed(_) | Self::Cancelled)
    }
}
#[derive(Clone)]
pub struct LabelledText {
    pub label: String,
    pub text: String,
    pub source: Option<String>,
}
#[derive(Clone)]
pub struct SourceNotice {
    pub process: ProcessNumber,
    pub label: String,
    pub source: Option<String>,
}
/// Full text stays between stages. It has no Serialize or Debug implementation.
pub struct StageOutput<T> {
    pub data: T,
    pub texts: Vec<LabelledText>,
    pub calls: Vec<ModelCall>,
    pub notices: Vec<SourceNotice>,
}
impl<T> StageOutput<T> {
    pub fn new(data: T) -> Self {
        Self {
            data,
            texts: vec![],
            calls: vec![],
            notices: vec![],
        }
    }
}
pub struct StageValue<T> {
    pub status: StageStatus,
    pub output: StageOutput<T>,
}
impl<T> StageValue<T> {
    pub fn ok(data: T) -> Self {
        Self {
            status: StageStatus::Ok,
            output: StageOutput::new(data),
        }
    }
}
pub struct StageResult<T> {
    pub stage: StageName,
    pub status: StageStatus,
    pub time_ms: u64,
    pub output: StageOutput<T>,
}
impl<T> StageResult<T> {
    pub fn record(&self) -> StageRecord {
        let (status, error_number, notice_number) = match self.status {
            StageStatus::Ok => ("ok", None, None),
            StageStatus::Failed(p) => ("failed", Some(p.number), None),
            StageStatus::Notice(p) => ("notice", None, Some(p.number)),
            StageStatus::Cancelled => ("cancelled", None, None),
        };
        StageRecord {
            name: self.stage.name().into(),
            status: status.into(),
            error_number,
            notice_number,
            time_ms: self.time_ms,
        }
    }
}
pub trait PipelineData: Clone + Send + Sync {
    fn thinking(&self) -> bool;
    fn merge_gather(previous: &Self, files: Self, memory: Self, web: Self) -> Self;
}
/// Services return new calls and notices for that stage. The engine carries
/// previous calls/notices forward. Every later service receives only a result.
pub trait StageServices<T: PipelineData>: Send + Sync {
    fn accept<'a>(&'a self, input: T) -> Pending<'a, StageValue<T>>;
    fn files<'a>(&'a self, previous: &'a StageResult<T>) -> Pending<'a, StageValue<T>>;
    fn memory<'a>(&'a self, previous: &'a StageResult<T>) -> Pending<'a, StageValue<T>>;
    fn web<'a>(&'a self, previous: &'a StageResult<T>) -> Pending<'a, StageValue<T>>;
    fn assemble<'a>(&'a self, previous: &'a StageResult<T>) -> Pending<'a, StageValue<T>>;
    fn think<'a>(&'a self, previous: &'a StageResult<T>) -> Pending<'a, StageValue<T>>;
    fn answer<'a>(&'a self, previous: &'a StageResult<T>) -> Pending<'a, StageValue<T>>;
    fn save<'a>(&'a self, previous: &'a StageResult<T>) -> Pending<'a, StageValue<T>>;
}
pub struct PipelineResult<T> {
    pub stages: Vec<StageResult<T>>,
}
impl<T> PipelineResult<T> {
    pub fn error(&self) -> Option<ProcessNumber> {
        self.stages.iter().find_map(|s| {
            if let StageStatus::Failed(p) = s.status {
                Some(p)
            } else {
                None
            }
        })
    }
    pub fn cancelled(&self) -> bool {
        self.stages
            .iter()
            .any(|s| matches!(s.status, StageStatus::Cancelled))
    }
}
pub enum PipelineEvent {
    StageStarted(StageName),
    Stage(StageRecord),
    Notice(SourceNotice),
}
