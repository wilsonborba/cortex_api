use crate::domain::models::events::OutputEvent;
use serde::{Deserialize, Serialize};
/// Exact public event body, stored in Redis only. Never write it to logs.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoredEvent {
    pub id: u64,
    pub name: String,
    pub body: serde_json::Value,
}
impl From<&OutputEvent> for StoredEvent {
    fn from(event: &OutputEvent) -> Self {
        Self {
            id: event.id,
            name: event.event.name().into(),
            body: serde_json::json!({"message":event.event.message(),"data":event.event.data()}),
        }
    }
}
impl StoredEvent {
    pub fn valid(&self) -> bool {
        self.id > 0
            && matches!(
                self.name.as_str(),
                "stage"
                    | "step"
                    | "sources"
                    | "memories"
                    | "thinking"
                    | "token"
                    | "notice"
                    | "error"
                    | "done"
            )
            && self.body.as_object().is_some_and(|body| {
                body.len() == 2
                    && body.get("message").is_some_and(|v| v.is_string())
                    && body.get("data").is_some_and(|v| v.is_object())
            })
    }
}
