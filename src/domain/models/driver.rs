use crate::{
    core::config::Config,
    domain::models::{
        identity::Caller,
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
    kind: ModelKind,
}
impl OutputModel {
    pub fn new(caller: &Caller, output_id: uuid::Uuid, has_images: bool) -> Self {
        Self {
            output_id,
            actor: caller.balancer_identity(),
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
    pub fn is_vision(&self) -> bool {
        self.kind == ModelKind::Vision
    }
}
pub struct CallInput {
    pub role: String,
    pub system: String,
    pub prompt: String,
    pub images: Vec<String>,
    pub temperature: f64,
    pub max_tokens: u32,
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
    pub text: String,
    pub usage: ModelCall,
    pub error: Option<DriverError>,
    pub load_ms: u64,
    pub tokens_per_second: Option<f64>,
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
