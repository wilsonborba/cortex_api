use crate::domain::services::run_evidence::RunEvidence;
use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::{local::chats::Chats, remote::hippocampus::Hippocampus},
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
    pub hippocampus: Hippocampus,
    pub runs: Option<RunEvidence>,
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
    pub async fn inspection(
        &self,
        user: &str,
        session: Uuid,
        output: Uuid,
    ) -> Result<crate::domain::models::inspection::AnswerInspection, ProcessNumber> {
        let (_, scope) = self.owned(user, session).await?;
        let saved = self
            .repo
            .output(&scope, output)
            .await
            .map_err(error)?
            .ok_or(process_numbers::OUTPUT_NOT_FOUND)?;
        if saved.session_id != session {
            return Err(process_numbers::OUTPUT_NOT_FOUND);
        }
        self.repo.inspection(&scope, output).await.map_err(error)
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
        let continued_from = self
            .repo
            .continued_from(scope, output.output_id)
            .await
            .map_err(error)?;
        Ok(OutputDetails {
            output,
            sources,
            memories,
            run: None,
            continued_from,
        })
    }
    fn run_metadata(
        scope: &AppUser,
        output: &ChatOutput,
        record: &crate::domain::models::run_record::RunRecord,
    ) -> Option<OutputRunMetadata> {
        if record.output_id != output.output_id
            || record.session_id != output.session_id
            || record.app_id != scope.app_id
            || record.user_id != scope.user_id
        {
            return None;
        }
        let terminal_failure = output.status == "failed"
            || (output.status == "running"
                && record
                    .stages
                    .last()
                    .is_some_and(|stage| stage.status == "failed"));
        let error_number = terminal_failure
            .then(|| {
                record.stages.iter().find_map(|stage| {
                    (stage.status == "failed")
                        .then_some(stage.error_number)
                        .flatten()
                })
            })
            .flatten();
        let tokens = if record.calls.is_empty() {
            None
        } else {
            record
                .calls
                .iter()
                .try_fold((0u64, 0u64), |(i, o), call| {
                    Some((
                        i.checked_add(call.input_tokens)?,
                        o.checked_add(call.output_tokens)?,
                    ))
                })
                .and_then(|(input, output)| {
                    input.checked_add(output).map(|total| OutputTokenCounts {
                        input,
                        output,
                        total,
                    })
                })
        };
        Some(OutputRunMetadata {
            error_number,
            total_ms: record.total_ms,
            tokens,
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
        if let Some(runs) = &self.runs {
            let ids: Vec<_> = outputs.iter().map(|item| item.output.output_id).collect();
            let mut records = Vec::new();
            let mut available = true;
            for batch in ids.chunks(100) {
                match runs.get_many(batch).await {
                    Ok(batch_records) => records.extend(batch_records),
                    Err(_) => {
                        available = false;
                        break;
                    }
                }
            }
            if available {
                for record in records {
                    if let Some(item) = outputs
                        .iter_mut()
                        .find(|item| item.output.output_id == record.output_id)
                    {
                        item.run = Self::run_metadata(&scope, &item.output, &record);
                    }
                }
            }
        }
        let mut memory_node_id = None;
        // Context reads do not create resources. A remote outage must not hide SQL history.
        for memory in self
            .repo
            .exchange_memory_ids(&scope, session)
            .await
            .map_err(error)?
        {
            match self
                .hippocampus
                .graph(&scope, Some(&memory), Some(1), Some(0))
                .await
            {
                Ok(graph) => {
                    memory_node_id = graph["nodes"]
                        .as_array()
                        .and_then(|nodes| {
                            nodes.iter().find(|node| {
                                node["node_type"] == "chat"
                                    && node["metadata"]["source_system"] == "cortex"
                                    && node["metadata"]["external_id"] == session.to_string()
                                    && node["id"]
                                        .as_str()
                                        .is_some_and(|id| id.starts_with("resource:"))
                            })
                        })
                        .and_then(|node| node["id"].as_str())
                        .map(str::to_owned);
                    if memory_node_id.is_some() {
                        break;
                    }
                }
                Err(crate::domain::models::memory_graph::GraphError::NotFound) => continue,
                Err(_) => break,
            }
        }
        Ok(ChatPage {
            chat: ChatWithMemory {
                chat,
                memory_node_id,
            },
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
        let mut details = self.details(&scope, output).await?;
        if let Some(runs) = &self.runs
            && let Ok(records) = runs.get_many(&[details.output.output_id]).await
        {
            details.run = records
                .iter()
                .find_map(|record| Self::run_metadata(&scope, &details.output, record));
        }
        Ok(details)
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

#[cfg(test)]
mod run_metadata_tests {
    use super::*;
    use crate::domain::models::run_record::{ModelCall, RunCounts, RunMode, StageRecord, WebMode};
    use chrono::Utc;

    fn output(status: &str, output_id: Uuid, session_id: Uuid) -> ChatOutput {
        ChatOutput {
            output_id,
            input_id: Uuid::new_v4(),
            session_id,
            status: status.into(),
            text: "answer".into(),
            thinking_text: None,
            memory_id: None,
            created_at: Utc::now(),
            finished_at: None,
        }
    }
    fn record(
        output_id: Uuid,
        session_id: Uuid,
        stages: Vec<StageRecord>,
    ) -> crate::domain::models::run_record::RunRecord {
        crate::domain::models::run_record::RunRecord {
            output_id,
            session_id,
            app_id: "cortex".into(),
            user_id: "owner".into(),
            created_at: Utc::now(),
            mode: RunMode {
                web_mode: WebMode::Fast,
                thinking: false,
            },
            total_ms: 42,
            stages,
            calls: vec![],
            counts: RunCounts {
                pages_read: 0,
                chunks_used: 0,
                files: 0,
                file_pages: 0,
                audio_seconds: 0.0,
            },
        }
    }
    fn stage(status: &str, error_number: Option<u16>) -> StageRecord {
        StageRecord {
            name: "answer".into(),
            status: status.into(),
            error_number: (status == "failed").then_some(error_number).flatten(),
            notice_number: (status == "notice").then_some(error_number).flatten(),
            time_ms: 1,
        }
    }

    fn model_call(input_tokens: u64, output_tokens: u64) -> ModelCall {
        ModelCall {
            images: None,
            audio: None,
            role: "answer".into(),
            model: "test-model".into(),
            input_tokens,
            output_tokens,
            time_to_first_token_ms: Some(5),
            total_ms: 10,
            started_at: Utc::now(),
            finished_at: Utc::now(),
        }
    }

    #[test]
    fn aggregates_model_call_tokens_and_omits_overflow_without_losing_duration() {
        let output_id = Uuid::new_v4();
        let session = Uuid::new_v4();
        let scope = AppUser {
            app_id: "cortex".into(),
            user_id: "owner".into(),
        };

        let mut multiple = record(output_id, session, vec![stage("ok", None)]);
        multiple.calls = vec![model_call(7, 3), model_call(5, 2)];
        let metadata =
            ChatHistory::run_metadata(&scope, &output("done", output_id, session), &multiple)
                .unwrap();
        assert_eq!(metadata.total_ms, 42);
        let tokens = metadata.tokens.unwrap();
        assert_eq!(tokens.input, 12);
        assert_eq!(tokens.output, 5);
        assert_eq!(tokens.total, 17);
        assert_eq!(metadata.error_number, None);

        let mut overflow = record(output_id, session, vec![stage("ok", None)]);
        overflow.calls = vec![model_call(u64::MAX, 1), model_call(1, 1)];
        let metadata =
            ChatHistory::run_metadata(&scope, &output("done", output_id, session), &overflow)
                .unwrap();
        assert_eq!(metadata.total_ms, 42);
        assert!(metadata.tokens.is_none());
        assert_eq!(metadata.error_number, None);
    }

    #[test]
    fn exposes_error_only_for_matching_failed_output_and_omits_fallback_error() {
        let output_id = Uuid::new_v4();
        let session = Uuid::new_v4();
        let scope = AppUser {
            app_id: "cortex".into(),
            user_id: "owner".into(),
        };
        let failed = record(output_id, session, vec![stage("failed", Some(1042))]);
        assert_eq!(
            ChatHistory::run_metadata(&scope, &output("failed", output_id, session), &failed)
                .unwrap()
                .error_number,
            Some(1042)
        );

        let fallback = record(
            output_id,
            session,
            vec![stage("notice", Some(1042)), stage("ok", None)],
        );
        assert_eq!(
            ChatHistory::run_metadata(&scope, &output("done", output_id, session), &fallback)
                .unwrap()
                .error_number,
            None
        );
        assert!(
            ChatHistory::run_metadata(&scope, &output("done", output_id, session), &fallback)
                .unwrap()
                .tokens
                .is_none()
        );

        let mut partial_value = serde_json::to_value(&fallback).unwrap();
        partial_value.as_object_mut().unwrap().remove("calls");
        let partial: crate::domain::models::run_record::RunRecord =
            serde_json::from_value(partial_value).unwrap();
        let partial_metadata =
            ChatHistory::run_metadata(&scope, &output("done", output_id, session), &partial)
                .unwrap();
        assert_eq!(partial_metadata.total_ms, 42);
        assert!(partial_metadata.tokens.is_none());

        let mut forged = failed;
        forged.user_id = "other".into();
        assert!(
            ChatHistory::run_metadata(&scope, &output("failed", output_id, session), &forged)
                .is_none()
        );

        // Live ChatRuntime emits PipelineResult::error(), which selects the
        // first failed stage. A recoverable notice does not mask a later failure.
        let terminal = record(
            output_id,
            session,
            vec![stage("notice", Some(1042)), stage("failed", Some(1043))],
        );
        assert_eq!(
            ChatHistory::run_metadata(&scope, &output("failed", output_id, session), &terminal)
                .unwrap()
                .error_number,
            Some(1043)
        );
        let save_failure = record(output_id, session, vec![stage("failed", Some(1017))]);
        assert_eq!(
            ChatHistory::run_metadata(
                &scope,
                &output("running", output_id, session),
                &save_failure
            )
            .unwrap()
            .error_number,
            Some(1017)
        );
        assert!(
            ChatHistory::run_metadata(&scope, &output("done", output_id, session), &terminal)
                .unwrap()
                .tokens
                .is_none()
        );
        assert!(
            ChatHistory::run_metadata(
                &scope,
                &output("failed", output_id, Uuid::new_v4()),
                &fallback
            )
            .is_none()
        );
        assert!(
            ChatHistory::run_metadata(
                &AppUser {
                    app_id: "other".into(),
                    user_id: "owner".into()
                },
                &output("failed", output_id, session),
                &fallback
            )
            .is_none()
        );
    }
}
