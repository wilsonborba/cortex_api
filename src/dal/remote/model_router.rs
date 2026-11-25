use crate::domain::models::{
    driver::{CallInput, CallOutcome, DriverError, DriverEvent, ModelDriver, OutputModel},
    usage::Pending,
};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// Routes profiles through existing engines while both engines share the same
/// Scheduler instance. It intentionally does not acquire a second permit.
pub struct ModelRouter {
    ollama: Arc<dyn ModelDriver>,
    prism: Arc<dyn ModelDriver>,
}

impl ModelRouter {
    pub fn new(ollama: Arc<dyn ModelDriver>, prism: Arc<dyn ModelDriver>) -> Self {
        Self { ollama, prism }
    }
}

impl ModelDriver for ModelRouter {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        if output.profile().uses_prism() {
            self.prism.call(output, input, events, cancel)
        } else {
            self.ollama.call(output, input, events, cancel)
        }
    }
}
