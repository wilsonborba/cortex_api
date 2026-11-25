use crate::domain::models::replay::StoredEvent;
use crate::domain::models::tools::ToolControl;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tokio::sync::{broadcast, watch};
use uuid::Uuid;
/// Live controls of the outputs that run in this process. A finished output
/// is removed; its events stay in the Redis journal (10.4).
#[derive(Clone)]
pub struct Control {
    pub cancel: Arc<watch::Sender<bool>>,
    pub stop_thinking: Arc<watch::Sender<bool>>,
    pub live: broadcast::Sender<StoredEvent>,
    pub tools: ToolControl,
    pub(crate) tool_gate: Arc<Mutex<bool>>,
}
impl Control {
    pub fn new(
        cancel: Arc<watch::Sender<bool>>,
        stop_thinking: Arc<watch::Sender<bool>>,
        live: broadcast::Sender<StoredEvent>,
        tools: ToolControl,
    ) -> Self {
        Self {
            cancel,
            stop_thinking,
            live,
            tools,
            tool_gate: Arc::new(Mutex::new(false)),
        }
    }

    /// Cancellation and a child tool result share one gate so no result can be
    /// accepted after cancellation has won the race.
    pub fn request_cancel(&self) {
        let mut cancelled = self.tool_gate.lock().unwrap_or_else(|e| e.into_inner());
        if !*cancelled {
            *cancelled = true;
            let _ = self.cancel.send(true);
        }
    }

    pub fn submit_tool_if_active(&self, result: crate::domain::models::tools::ToolResult) -> bool {
        let cancelled = self.tool_gate.lock().unwrap_or_else(|e| e.into_inner());
        if *cancelled || *self.cancel.borrow() {
            return false;
        }
        self.tools.submit(result).is_ok()
    }
}
#[derive(Clone, Default)]
pub struct OutputControls(Arc<Mutex<HashMap<Uuid, Control>>>);
impl OutputControls {
    fn map(&self) -> std::sync::MutexGuard<'_, HashMap<Uuid, Control>> {
        self.0.lock().unwrap_or_else(|e| e.into_inner())
    }
    pub fn open(&self, output: Uuid, control: Control) {
        self.map().insert(output, control);
    }
    pub fn close(&self, output: Uuid) {
        self.map().remove(&output);
    }
    pub fn get(&self, output: Uuid) -> Option<Control> {
        self.map().get(&output).cloned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::models::tools::{ToolName, ToolResult};

    #[tokio::test]
    async fn cancellation_gate_rejects_late_child_result_and_orders_prior_result() {
        let (cancel, cancelled) = watch::channel(false);
        let (stop_thinking, _) = watch::channel(false);
        let (live, _) = broadcast::channel(1);
        let (result_tx, mut results) = tokio::sync::mpsc::unbounded_channel();
        let control = Control::new(
            Arc::new(cancel),
            Arc::new(stop_thinking),
            live,
            ToolControl::new(result_tx),
        );
        let result = ToolResult {
            call_id: Uuid::now_v7(),
            name: ToolName::InvokeSubagent,
            output: "completed".into(),
        };
        assert!(
            control
                .tools
                .set_pending(crate::domain::models::tools::PendingTool {
                    call_id: result.call_id,
                    name: result.name,
                    arguments: serde_json::Value::Null,
                })
        );
        assert!(control.submit_tool_if_active(result.clone()));
        assert_eq!(results.recv().await.unwrap().call_id, result.call_id);
        control.request_cancel();
        assert!(*cancelled.borrow());
        assert!(!control.submit_tool_if_active(result));
        assert!(results.try_recv().is_err());
    }
}
