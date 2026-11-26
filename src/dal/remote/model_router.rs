use crate::{
    dal::remote::{ollama::Ollama, prism::Prism},
    domain::{
        models::{
            driver::{CallInput, CallOutcome, DriverError, DriverEvent, ModelDriver, OutputModel},
            usage::Pending,
        },
        services::balancer::Scheduler,
    },
};
use std::sync::Arc;
use tokio::sync::{mpsc, watch};

/// Routes through one existing Scheduler. Prism transitions acquire here so
/// T0 and Prism teardown/load happen under the same permit; Ollama retains its
/// existing acquisition path for T0 requests.
pub struct ModelRouter {
    ollama: Arc<Ollama>,
    prism: Arc<Prism>,
    queue: Scheduler,
}

impl ModelRouter {
    pub fn new(ollama: Arc<Ollama>, prism: Arc<Prism>, queue: Scheduler) -> Self {
        Self {
            ollama,
            prism,
            queue,
        }
    }
}

impl ModelDriver for ModelRouter {
    fn call<'a>(
        &'a self,
        output: &'a OutputModel,
        input: CallInput,
        events: mpsc::UnboundedSender<DriverEvent>,
        mut cancel: watch::Receiver<bool>,
    ) -> Pending<'a, Result<CallOutcome, DriverError>> {
        if output.profile().uses_prism() {
            Box::pin(async move {
                let _ = events.send(DriverEvent::Waiting);
                let permit = tokio::select! {
                    biased;
                    _ = async {
                        loop {
                            if *cancel.borrow() {
                                return;
                            } else if cancel.changed().await.is_err() {
                                std::future::pending::<()>().await;
                            }
                        }
                    } => return Err(DriverError::Cancelled),
                    p = self.queue.acquire(output.actor.clone(), output.output_id) => p.map_err(DriverError::Queue)?,
                };
                self.ollama
                    .release_models_for_prism_transition(&permit, &mut cancel)
                    .await?;
                self.prism
                    .call_with_permit(output, input, events, cancel, permit)
                    .await
            })
        } else {
            self.ollama.call(output, input, events, cancel)
        }
    }
}
