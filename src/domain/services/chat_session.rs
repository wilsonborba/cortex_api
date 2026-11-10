use crate::{
    core::{
        config::Config,
        process_numbers::{self, ProcessNumber},
    },
    domain::models::{chat_session::*, identity::Caller, run_record::WebMode, storage::*},
};
use chrono::{DateTime, Utc};
use uuid::Uuid;
/// Keep Unicode characters intact. A word longer than the entire limit uses
/// the character limit, since it has no earlier word boundary.
pub fn title(config: &Config, text: &str, files: &[SessionFile]) -> String {
    if text.trim().is_empty() {
        return files.first().map(|f| f.name.clone()).unwrap_or_default();
    }
    let first = text.lines().next().unwrap_or_default().trim();
    let end = first
        .char_indices()
        .nth(config.chat_title_characters)
        .map(|(i, _)| i);
    let Some(end) = end else { return first.into() };
    let prefix = &first[..end];
    if first[end..].starts_with(char::is_whitespace) {
        return prefix.trim_end().into();
    }
    if let Some((boundary, _)) = prefix.char_indices().rfind(|(_, c)| c.is_whitespace()) {
        return prefix[..boundary].trim_end().into();
    }
    prefix.into()
}
fn prepare(
    scope: AppUser,
    session: Uuid,
    input: NextInput,
    files: &[SessionFile],
    now: DateTime<Utc>,
) -> Result<PreparedInput, ProcessNumber> {
    if (input.text.trim().is_empty() && input.file_ids.is_empty())
        || input.file_ids.len() != files.len()
        || input
            .file_ids
            .iter()
            .zip(files)
            .any(|(id, file)| id.is_empty() || id != &file.file_id)
        || files.len() > i32::MAX as usize
    {
        return Err(process_numbers::INVALID_DATA);
    }
    let input_id = Uuid::now_v7();
    let output_id = Uuid::now_v7();
    let attachments = files
        .iter()
        .enumerate()
        .map(|(index, file)| InputFile {
            input_id,
            file_id: file.file_id.clone(),
            memory_id: file.memory_id.clone(),
            position: index as i32,
        })
        .collect();
    let web_mode = match input.web_mode {
        WebMode::Fast => "fast",
        WebMode::Medium => "medium",
        WebMode::Deep => "deep",
    };
    Ok(PreparedInput {
        new_chat: None,
        scope,
        input: ChatInput {
            input_id,
            session_id: session,
            text: input.text,
            needs_web: input.needs_web,
            web_mode: web_mode.into(),
            use_memory: input.use_memory,
            memory_topic: input.memory_topic,
            thinking: input.thinking,
            created_at: now,
        },
        output: ChatOutput {
            output_id,
            input_id,
            session_id: session,
            status: "running".into(),
            text: String::new(),
            thinking_text: None,
            memory_id: None,
            created_at: now,
            finished_at: None,
        },
        files: attachments,
    })
}
pub fn new_permanent(
    config: &Config,
    caller: &Caller,
    body: NewChat,
    files: &[SessionFile],
    now: DateTime<Utc>,
) -> Result<PreparedInput, ProcessNumber> {
    if caller.app_id != body.app_id || !config.allowed_apps.contains(&body.app_id.as_str()) {
        return Err(process_numbers::UNKNOWN_APP);
    }
    // The temporary-chat service handles this branch. It must never reach SQL.
    if body.temporary {
        return Err(process_numbers::INVALID_DATA);
    }
    let session = Uuid::now_v7();
    let title = title(config, &body.text, files);
    let mut prepared = prepare(caller.storage_scope(), session, body.input(), files, now)?;
    prepared.new_chat = Some(Chat {
        session_id: session,
        app_id: caller.app_id.clone(),
        user_id: caller.user_id.clone().unwrap_or_default(),
        title,
        persona_id: body.persona_id,
        is_pinned: false,
        created_at: now,
        updated_at: now,
    });
    Ok(prepared)
}
pub fn continue_permanent(
    caller: &Caller,
    chat: &Chat,
    body: NextInput,
    files: &[SessionFile],
    now: DateTime<Utc>,
) -> Result<PreparedInput, ProcessNumber> {
    if chat.app_id != caller.app_id || chat.user_id != caller.user_id.as_deref().unwrap_or_default()
    {
        return Err(process_numbers::NOT_FOUND);
    }
    prepare(caller.storage_scope(), chat.session_id, body, files, now)
}
