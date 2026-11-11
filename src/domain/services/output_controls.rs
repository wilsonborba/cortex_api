use crate::domain::models::replay::StoredEvent;
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
