use crate::domain::models::tools::{ToolExchange, ToolRequest};
use crate::{
    core::config::Config,
    domain::models::{
        identity::Caller,
        model_profile::ModelProfile,
        run_record::ModelCall,
        usage::{Pending, QueueError},
    },
};
use tokio::sync::{mpsc, watch};
#[derive(Clone, Copy, PartialEq, Eq)]
enum ModelKind {
    Text,
    Vision,
}
#[derive(Clone)]
pub struct OutputModel {
    pub output_id: uuid::Uuid,
    pub actor: String,
    profile: ModelProfile,
    kind: ModelKind,
}
impl OutputModel {
    pub fn new(caller: &Caller, output_id: uuid::Uuid, has_images: bool) -> Self {
        Self::for_profile(
            caller,
            output_id,
            ModelProfile::legacy_default(),
            has_images,
        )
    }
    pub fn for_profile(
        caller: &Caller,
        output_id: uuid::Uuid,
        profile: ModelProfile,
        has_images: bool,
    ) -> Self {
        Self {
            output_id,
            actor: caller.balancer_identity(),
            profile,
            kind: if has_images {
                ModelKind::Vision
            } else {
                ModelKind::Text
            },
        }
    }
    pub fn name<'a>(&self, config: &'a Config) -> &'a str {
        match self.kind {
            ModelKind::Text => &config.text_model,
            ModelKind::Vision => &config.vision_model,
        }
    }
    pub const fn profile(&self) -> ModelProfile {
        self.profile
    }
    pub const fn model_id(&self) -> &'static str {
        self.profile.id()
    }
    pub fn context_tokens(&self, config: &Config) -> u64 {
        self.profile.context_tokens(config)
    }
    pub fn max_output_tokens(&self, config: &Config) -> u64 {
        self.profile.max_output_tokens(config)
    }
    pub fn supports_images(&self) -> bool {
        matches!(self.kind, ModelKind::Vision) && self.profile == ModelProfile::CortexT0
    }
    pub fn is_vision(&self) -> bool {
        self.kind == ModelKind::Vision
    }
    pub fn with_images(mut self, has_images: bool) -> Self {
        self.kind = if has_images {
            ModelKind::Vision
        } else {
            ModelKind::Text
        };
        self
    }
    pub fn with_profile(mut self, profile: ModelProfile, has_images: bool) -> Self {
        self.profile = profile;
        self.with_images(has_images)
    }
}
#[derive(Clone)]
pub struct CallInput {
    pub role: String,
    pub system: String,
    pub prompt: String,
    pub images: Vec<String>,
    pub temperature: f64,
    pub max_tokens: u32,
    /// Provider context window for this request, bounded by the caller's budget.
    pub context_tokens: u32,
    /// Saved answer prefix. The model continues it as its own message.
    pub prefix: Option<String>,
    /// Only the explicitly opted-in CLI answer path receives these tools.
    pub allow_tools: bool,
    /// Provider-native assistant tool call and exact CLI result pairs.
    pub tool_history: Vec<ToolExchange>,
}
#[derive(Clone)]
pub enum DriverEvent {
    Waiting,
    ModelLoading { model: String },
    ModelReady { model: String, load_ms: u64 },
    Token { role: String, text: String },
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DriverError {
    Queue(QueueError),
    Ollama,
    InvalidInput,
    Cancelled,
}
pub struct CallOutcome {
    pub inspection: Option<crate::domain::models::inspection::AnswerInspection>,
    pub text: String,
    pub usage: ModelCall,
    /// True only when the provider returned both prompt and completion token counts.
    pub usage_measured: bool,
    pub error: Option<DriverError>,
    pub load_ms: u64,
    pub tokens_per_second: Option<f64>,
    pub tool_request: Option<ToolRequest>,
}
pub trait ModelDriver: Send + Sync {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>>;
}
