use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::remote::{cache::RedisCache, temporary::TemporaryStore},
    domain::{
        models::{
            cache::Lifetime,
            chat_session::{NewChat, NextInput, SessionFile},
            history::FinishedOutput,
            identity::Caller,
            temporary::*,
        },
        services::chat_session,
    },
};
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// This service has only a Redis store. It has no SQL or Hippocampus writer.
#[derive(Clone)]
pub struct TemporaryChats {
    store: TemporaryStore,
    config: Config,
}
impl TemporaryChats {
    pub fn new(config: &Config, cache: RedisCache) -> Self {
        Self {
            store: TemporaryStore::new(cache),
            config: config.clone(),
        }
    }
    fn lifetime(&self) -> Result<Lifetime, ProcessNumber> {
        Lifetime::new(self.config.temporary_ttl).map_err(|_| process_numbers::TEMPORARY_UNAVAILABLE)
    }
    async fn data(
        &self,
        caller: &Caller,
        id: Uuid,
    ) -> Result<crate::dal::remote::temporary::Snapshot, ProcessNumber> {
        let snapshot = self
            .store
            .get(id)
            .await
            .map_err(|_| process_numbers::TEMPORARY_UNAVAILABLE)?
            .ok_or(process_numbers::CHAT_NOT_FOUND)?;
        if snapshot.data.app_id != caller.app_id
            || snapshot.data.user_id != caller.user_id.as_deref().unwrap_or_default()
        {
            return Err(process_numbers::CHAT_NOT_FOUND);
        }
        Ok(snapshot)
    }
    pub async fn file(&self, caller: &Caller, id: Uuid) -> Result<TemporaryFile, ProcessNumber> {
        self.data(caller, id)
            .await?
            .data
            .file
            .ok_or(process_numbers::CHAT_NOT_FOUND)
    }
    pub async fn save_file(
        &self,
        caller: &Caller,
        mut file: TemporaryFile,
    ) -> Result<Uuid, ProcessNumber> {
        if !self.config.allowed_apps.contains(&caller.app_id.as_str()) {
            return Err(process_numbers::UNKNOWN_APP);
        }
        file.file_id = Uuid::now_v7();
        let id = file.file_id;
        for (index, page) in file.pages.iter_mut().enumerate() {
            page.source = Some(format!("{id}#page={}", index + 1));
        }
        let data = TemporaryData {
            app_id: caller.app_id.clone(),
            user_id: caller.user_id.clone().unwrap_or_default(),
            chat: None,
            file: Some(file),
        };
        if !self
            .store
            .create(id, &data, self.lifetime()?)
            .await
            .map_err(|_| process_numbers::TEMPORARY_UNAVAILABLE)?
        {
            return Err(process_numbers::TEMPORARY_UNAVAILABLE);
        }
        Ok(id)
    }
    pub async fn get(&self, caller: &Caller, id: Uuid) -> Result<TemporaryChat, ProcessNumber> {
        self.data(caller, id)
            .await?
            .data
            .chat
            .ok_or(process_numbers::CHAT_NOT_FOUND)
    }
    async fn files(
        &self,
        caller: &Caller,
        input: &NextInput,
        existing: &[TemporaryFile],
    ) -> Result<Vec<TemporaryFile>, ProcessNumber> {
        let mut files = vec![];
        for id in &input.file_ids {
            let id = Uuid::parse_str(id).map_err(|_| process_numbers::INVALID_DATA)?;
            files.push(match existing.iter().find(|f| f.file_id == id) {
                Some(file) => file.clone(),
                None => self.file(caller, id).await?,
            });
        }
        Ok(files)
    }
    pub async fn start(
        &self,
        caller: &Caller,
        body: NewChat,
        now: DateTime<Utc>,
    ) -> Result<TemporaryChat, ProcessNumber> {
        if !body.temporary {
            return Err(process_numbers::INVALID_DATA);
        }
        if body.app_id != caller.app_id
            || !self.config.allowed_apps.contains(&caller.app_id.as_str())
        {
            return Err(process_numbers::UNKNOWN_APP);
        }
        let files = self.files(caller, &body.input(), &[]).await?;
        let metadata = metadata(&files);
        let session_id = Uuid::now_v7();
        let prepared = chat_session::prepare(
            caller.storage_scope(),
            session_id,
            body.input(),
            &metadata,
            now,
        )?;
        let chat = TemporaryChat {
            session_id,
            title: chat_session::title(&self.config, &body.text, &metadata),
            persona_id: body.persona_id,
            inputs: vec![prepared.input],
            outputs: vec![TemporaryOutput {
                output: prepared.output,
                sources: vec![],
                memories: vec![],
            }],
            files,
        };
        let data = TemporaryData {
            app_id: caller.app_id.clone(),
            user_id: caller.user_id.clone().unwrap_or_default(),
            chat: Some(chat.clone()),
            file: None,
        };
        if !self
            .store
            .create(session_id, &data, self.lifetime()?)
            .await
            .map_err(|_| process_numbers::TEMPORARY_UNAVAILABLE)?
        {
            return Err(process_numbers::TEMPORARY_UNAVAILABLE);
        }
        Ok(chat)
    }
    async fn update(
        &self,
        caller: &Caller,
        id: Uuid,
        refresh: bool,
        change: impl Fn(&mut TemporaryChat) -> Result<(), ProcessNumber>,
    ) -> Result<TemporaryChat, ProcessNumber> {
        let deadline = tokio::time::Instant::now() + self.config.cache_operation_timeout;
        loop {
            let previous = self.data(caller, id).await?;
            let mut data = previous.data.clone();
            let chat = data.chat.as_mut().ok_or(process_numbers::CHAT_NOT_FOUND)?;
            change(chat)?;
            if self
                .store
                .replace(
                    id,
                    &previous,
                    &data,
                    if refresh {
                        Some(self.lifetime()?)
                    } else {
                        None
                    },
                )
                .await
                .map_err(|_| process_numbers::TEMPORARY_UNAVAILABLE)?
            {
                return Ok(data.chat.expect("chat retained"));
            }
            if tokio::time::Instant::now() >= deadline {
                return Err(process_numbers::TEMPORARY_UNAVAILABLE);
            }
        }
    }
    pub async fn append(
        &self,
        caller: &Caller,
        id: Uuid,
        body: NextInput,
        now: DateTime<Utc>,
    ) -> Result<TemporaryChat, ProcessNumber> {
        let previous = self.get(caller, id).await?;
        let files = self.files(caller, &body, &previous.files).await?;
        let prepared =
            chat_session::prepare(caller.storage_scope(), id, body, &metadata(&files), now)?;
        self.update(caller, id, true, |chat| {
            chat.inputs.push(prepared.input.clone());
            chat.outputs.push(TemporaryOutput {
                output: prepared.output.clone(),
                sources: vec![],
                memories: vec![],
            });
            for file in &files {
                if !chat.files.iter().any(|f| f.file_id == file.file_id) {
                    chat.files.push(file.clone());
                }
            }
            Ok(())
        })
        .await
    }
    pub async fn finish(
        &self,
        caller: &Caller,
        done: &FinishedOutput,
    ) -> Result<TemporaryChat, ProcessNumber> {
        if done.memory_id.is_some() {
            return Err(process_numbers::INVALID_DATA);
        }
        self.update(caller, done.session_id, false, |chat| {
            let output = chat
                .outputs
                .iter_mut()
                .find(|o| o.output.output_id == done.output_id)
                .ok_or(process_numbers::OUTPUT_NOT_FOUND)?;
            if output.output.status != "running" {
                return Err(process_numbers::INVALID_DATA);
            }
            output.output.status = done.status.name().into();
            output.output.text = done.text.clone();
            output.output.thinking_text = done.thinking_text.clone();
            output.output.finished_at = Some(done.finished_at);
            output.sources = done.sources.clone();
            output.memories = done.memories.clone();
            Ok(())
        })
        .await
    }
}
fn metadata(files: &[TemporaryFile]) -> Vec<SessionFile> {
    files
        .iter()
        .map(|file| SessionFile {
            file_id: file.file_id.to_string(),
            memory_id: String::new(),
            name: file.name.clone(),
        })
        .collect()
}
