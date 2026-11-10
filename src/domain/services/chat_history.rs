use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::local::chats::Chats,
    domain::models::{
        chat_history::*,
        events::{Memory, Source},
        storage::{AppUser, Chat, ChatOutput, StorageError},
    },
};
use uuid::Uuid;
#[derive(Clone)]
pub struct ChatHistory {
    pub repo: Chats,
    pub config: Config,
}
fn error(error: StorageError) -> ProcessNumber {
    match error {
        StorageError::NotFound => process_numbers::CHAT_NOT_FOUND,
        StorageError::Database => process_numbers::HISTORY_FAILED,
    }
}
impl ChatHistory {
    pub async fn owned(&self, user: &str, session: Uuid) -> Result<(Chat, AppUser), ProcessNumber> {
        let chat = self
            .repo
            .owned_chat(user, session)
            .await
            .map_err(error)?
            .ok_or(process_numbers::CHAT_NOT_FOUND)?;
        if !self.config.allowed_apps.contains(&chat.app_id.as_str()) {
            return Err(process_numbers::UNKNOWN_APP);
        }
        let scope = AppUser {
            app_id: chat.app_id.clone(),
            user_id: user.into(),
        };
        Ok((chat, scope))
    }
    pub async fn list(&self, user: &str, query: ListQuery) -> Result<Vec<Chat>, ProcessNumber> {
        match query.status {
            None => self.repo.list_chats(user).await,
            // Temporary chats are never in SQL, so they are never listed (10.4.8).
            Some(ListStatus::Running) => self.repo.running_chats(user).await,
        }
        .map_err(error)
    }
    async fn details(
        &self,
        scope: &AppUser,
        output: ChatOutput,
    ) -> Result<OutputDetails, ProcessNumber> {
        let sources = self
            .repo
            .sources(scope, output.output_id)
            .await
            .map_err(error)?
            .into_iter()
            .map(|s| Source {
                title: s.title,
                url: s.url,
            })
            .collect();
        let memories = self
            .repo
            .memories(scope, output.output_id)
            .await
            .map_err(error)?
            .into_iter()
            .map(|m| Memory {
                id: m.memory_id,
                title: m.title,
            })
            .collect();
        Ok(OutputDetails {
            output,
            sources,
            memories,
        })
    }
    pub async fn page(
        &self,
        user: &str,
        session: Uuid,
        query: PageQuery,
    ) -> Result<ChatPage, ProcessNumber> {
        let limit = query.limit.unwrap_or(self.config.history_page_default);
        if limit == 0 {
            return Err(process_numbers::INVALID_DATA);
        }
        let (chat, scope) = self.owned(user, session).await?;
        let inputs = self
            .repo
            .page_inputs(&scope, session, query.before, limit)
            .await
            .map_err(error)?;
        let mut outputs = vec![];
        for input in &inputs {
            if let Some(output) = self
                .repo
                .output_for_input(&scope, input.input_id)
                .await
                .map_err(error)?
            {
                outputs.push(self.details(&scope, output).await?);
            }
        }
        Ok(ChatPage {
            chat,
            inputs,
            outputs,
        })
    }
    pub async fn output(
        &self,
        user: &str,
        session: Uuid,
        output: Uuid,
    ) -> Result<OutputDetails, ProcessNumber> {
        let (_, scope) = self.owned(user, session).await?;
        let output = self
            .repo
            .output(&scope, output)
            .await
            .map_err(error)?
            .filter(|o| o.session_id == session)
            .ok_or(process_numbers::OUTPUT_NOT_FOUND)?;
        self.details(&scope, output).await
    }
    pub async fn patch(
        &self,
        user: &str,
        session: Uuid,
        patch: ChatPatch,
    ) -> Result<Chat, ProcessNumber> {
        let (_, scope) = self.owned(user, session).await?;
        self.repo
            .patch_chat(&scope, session, &patch)
            .await
            .map_err(error)
    }
}
