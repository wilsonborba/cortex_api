use crate::{
    core::process_numbers::{self, ProcessNumber},
    dal::remote::hippocampus::Hippocampus,
    domain::services::chat_history::ChatHistory,
};
use uuid::Uuid;
#[derive(Clone)]
pub struct ChatDelete {
    pub history: ChatHistory,
    pub hippocampus: Hippocampus,
}
impl ChatDelete {
    /// Hippocampus first, then SQL (19.4). If Hippocampus fails, the chat
    /// stays and the user can try again. There is no retry table.
    pub async fn delete(&self, user: &str, session: Uuid) -> Result<(), ProcessNumber> {
        let (_, scope) = self.history.owned(user, session).await?;
        let repo = &self.history.repo;
        let ids = repo
            .chat_memory_ids(&scope, session)
            .await
            .map_err(|_| process_numbers::HISTORY_FAILED)?;
        if !ids.is_empty() {
            self.hippocampus
                .delete_memories(&scope, &ids)
                .await
                .map_err(|_| process_numbers::CHAT_DELETE_FAILED)?;
        }
        repo.delete_chat(&scope, session)
            .await
            .map_err(|_| process_numbers::HISTORY_FAILED)?;
        Ok(())
    }
}
