use crate::{
    core::process_numbers::{self, ProcessNumber},
    dal::{local::chats::Chats, remote::hippocampus::Hippocampus},
    domain::models::{exchange_memory::Exchange, storage::AppUser},
};
use uuid::Uuid;
#[derive(Clone)]
pub struct ExchangeMemories {
    pub repo: Chats,
    pub hippocampus: Hippocampus,
}
pub struct MemorySave {
    pub memory_id: Option<String>,
    pub notice: Option<ProcessNumber>,
}
impl ExchangeMemories {
    /// Called in Save after SQL stores the finished answer. A failure leaves
    /// that answer intact. There is no retry table and no LLM call.
    pub async fn remember(
        &self,
        scope: &AppUser,
        session: Uuid,
        output: Uuid,
        temporary: bool,
    ) -> MemorySave {
        if temporary {
            return MemorySave {
                memory_id: None,
                notice: None,
            };
        }
        match self.write(scope, session, output).await {
            Ok(memory_id) => MemorySave {
                memory_id,
                notice: None,
            },
            Err(()) => {
                tracing::warn!(
                    notice_number = process_numbers::MEMORY_SAVE_FAILED.number,
                    "Exchange memory save failed"
                );
                MemorySave {
                    memory_id: None,
                    notice: Some(process_numbers::MEMORY_SAVE_FAILED),
                }
            }
        }
    }
    async fn write(
        &self,
        scope: &AppUser,
        session: Uuid,
        output: Uuid,
    ) -> Result<Option<String>, ()> {
        let Some(assignment) = self
            .repo
            .lock_completed_exchange(scope, session, output)
            .await
            .map_err(|_| ())?
        else {
            return Ok(None);
        };
        if let Some(id) = assignment.memory_id() {
            let id = id.to_owned();
            assignment.release().await.map_err(|_| ())?;
            return Ok(Some(id));
        }
        let id = self
            .hippocampus
            .create_exchange(
                scope,
                &Exchange {
                    session,
                    chat_title: assignment.title(),
                    input: assignment.input(),
                    output: assignment.output(),
                    topic: assignment.topic(),
                },
            )
            .await
            .map_err(|_| ())?;
        assignment.assign(&id).await.map_err(|_| ())?;
        Ok(Some(id))
    }
}
