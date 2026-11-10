use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    dal::remote::hippocampus::Hippocampus,
    domain::{
        models::{
            context::ContextText,
            extraction::DocumentFile,
            files::{FileContent, FileMemory, Upload},
            identity::Caller,
            memory_graph::GraphError,
            temporary::TemporaryFile,
        },
        services::{events::channel, extraction::DocumentExtractor, temporary::TemporaryChats},
    },
};
#[derive(Clone)]
pub struct FileProxy {
    pub hippocampus: Hippocampus,
    pub extractor: DocumentExtractor,
    pub transcriber: std::sync::Arc<dyn crate::domain::services::transcription::AudioTranscriber>,
    pub temporary: TemporaryChats,
    pub config: Config,
}
const AUDIO_EXTENSIONS: [&str; 7] = ["opus", "ogg", "oga", "mp3", "wav", "m4a", "flac"];
fn is_audio(file: &DocumentFile) -> bool {
    file.content_type.to_ascii_lowercase().starts_with("audio/")
        || file
            .name
            .rsplit_once('.')
            .is_some_and(|(_, ext)| AUDIO_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
}
fn lookup_error(error: GraphError) -> ProcessNumber {
    match error {
        GraphError::Unavailable => process_numbers::MEMORY_LOAD_FAILED,
        GraphError::Invalid | GraphError::NotFound => process_numbers::FILE_NOT_FOUND,
    }
}
impl FileProxy {
    /// Extract the text once, then save the file and text in one Hippocampus
    /// call. A temporary chat file stays in Redis only (17.4).
    pub async fn upload(
        &self,
        caller: &Caller,
        file: DocumentFile,
        temporary: bool,
    ) -> Result<Upload, ProcessNumber> {
        if file.bytes.len() > self.config.file_bytes {
            return Err(process_numbers::FILE_SIZE);
        }
        // Upload has no output stream, so the progress events are dropped.
        let (events, _, _) = channel(&self.config, None);
        // Audio is transcribed (7.1); a failed transcription still saves the file (8.4).
        let text = if is_audio(&file) {
            self.transcriber
                .transcribe(caller, &file.name, file.bytes.clone(), &events)
                .await
                .unwrap_or_default()
        } else {
            self.extractor.extract(file.clone(), &events).await.text()
        };
        if temporary {
            let pages = (!text.is_empty())
                .then(|| ContextText {
                    label: file.name.clone(),
                    source: None,
                    text: text.clone(),
                })
                .into_iter()
                .collect();
            let id = self
                .temporary
                .save_file(
                    caller,
                    TemporaryFile {
                        file_id: uuid::Uuid::nil(),
                        name: file.name,
                        pages,
                    },
                )
                .await?;
            return Ok(Upload::Saved(id.to_string()));
        }
        Ok(
            match self
                .hippocampus
                .save_file(&caller.storage_scope(), &file, &text)
                .await
            {
                Ok(id) => Upload::Saved(id),
                Err(_) => Upload::NotSaved,
            },
        )
    }
    /// The Hippocampus file memory of a file inside the caller's scope.
    pub async fn file(&self, caller: &Caller, file_id: &str) -> Result<FileMemory, ProcessNumber> {
        self.hippocampus
            .file_memory(&caller.storage_scope(), file_id)
            .await
            .map_err(lookup_error)
    }
    /// Check the scope first (8.6), then read the bytes through Hippocampus.
    pub async fn download(
        &self,
        caller: &Caller,
        file_id: &str,
    ) -> Result<FileContent, ProcessNumber> {
        self.file(caller, file_id).await?;
        self.hippocampus
            .file_content(&caller.storage_scope(), file_id)
            .await
            .map_err(lookup_error)
    }
}
