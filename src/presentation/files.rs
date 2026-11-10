use crate::{
    core::process_numbers,
    domain::{
        models::{extraction::DocumentFile, files::Upload},
        services::files::FileProxy,
    },
    presentation::{
        identity::CallerContext,
        response::{self, Envelope},
    },
};
use axum::{
    Router,
    extract::{DefaultBodyLimit, Multipart, Path, Query, State, multipart::MultipartRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::Deserialize;
use serde_json::json;
use std::sync::Arc;
pub fn router(state: FileProxy) -> Router {
    let limit = state.config.file_bytes + state.config.file_form_overhead;
    Router::new()
        .route("/files", post(upload).layer(DefaultBodyLimit::max(limit)))
        .route("/files/{file_id}/content", get(download))
        .with_state(state)
}
fn status(process: process_numbers::ProcessNumber) -> StatusCode {
    match process.number {
        n if n == process_numbers::FILE_SIZE.number => StatusCode::PAYLOAD_TOO_LARGE,
        n if n == process_numbers::FILE_NOT_FOUND.number => StatusCode::NOT_FOUND,
        n if n == process_numbers::MEMORY_LOAD_FAILED.number
            || n == process_numbers::TEMPORARY_UNAVAILABLE.number =>
        {
            StatusCode::SERVICE_UNAVAILABLE
        }
        _ => StatusCode::BAD_REQUEST,
    }
}
fn invalid() -> Response {
    response::error(StatusCode::BAD_REQUEST, process_numbers::INVALID_DATA)
}
/// Multipart fields: `file`, `app_id` and an optional `temporary=true`.
async fn upload(
    State(state): State<FileProxy>,
    context: CallerContext,
    form: Result<Multipart, MultipartRejection>,
) -> Result<Response, Response> {
    let mut form = form.map_err(|_| invalid())?;
    let (mut file, mut app_id, mut temporary) = (None, None, false);
    while let Some(field) = form.next_field().await.map_err(|e| {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            response::error(StatusCode::PAYLOAD_TOO_LARGE, process_numbers::FILE_SIZE)
        } else {
            invalid()
        }
    })? {
        match field.name().unwrap_or_default() {
            "file" if file.is_none() => {
                let name = field.file_name().unwrap_or("upload.bin").to_owned();
                let content_type = field
                    .content_type()
                    .unwrap_or("application/octet-stream")
                    .to_owned();
                let bytes = field.bytes().await.map_err(|e| {
                    if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
                        response::error(StatusCode::PAYLOAD_TOO_LARGE, process_numbers::FILE_SIZE)
                    } else {
                        invalid()
                    }
                })?;
                file = Some(DocumentFile {
                    name,
                    content_type,
                    bytes: Arc::new(bytes.to_vec()),
                });
            }
            "app_id" if app_id.is_none() => {
                app_id = Some(field.text().await.map_err(|_| invalid())?)
            }
            "temporary" => {
                temporary = match field.text().await.map_err(|_| invalid())?.as_str() {
                    "true" => true,
                    "false" => false,
                    _ => return Err(invalid()),
                }
            }
            _ => return Err(invalid()),
        }
    }
    let (Some(file), Some(app_id)) = (file, app_id) else {
        return Err(invalid());
    };
    let caller = context
        .for_app(&app_id, true)
        .map_err(IntoResponse::into_response)?;
    match state.upload(&caller, file, temporary).await {
        Ok(Upload::Saved(file_id)) => Ok((
            StatusCode::CREATED,
            Envelope::new("File saved", json!({"file_id": file_id})),
        )
            .into_response()),
        Ok(Upload::NotSaved) => {
            Ok(Envelope::notice(process_numbers::FILE_NOT_SAVED, json!({})).into_response())
        }
        Err(process) => Err(response::error(status(process), process)),
    }
}
#[derive(Deserialize)]
struct AppQuery {
    app_id: String,
}
async fn download(
    State(state): State<FileProxy>,
    context: CallerContext,
    Path(file_id): Path<String>,
    Query(query): Query<AppQuery>,
) -> Result<Response, Response> {
    let caller = context
        .for_app(&query.app_id, true)
        .map_err(IntoResponse::into_response)?;
    let file = state
        .download(&caller, &file_id)
        .await
        .map_err(|process| response::error(status(process), process))?;
    let mut response = file.bytes.into_response();
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_str(&file.content_type)
            .unwrap_or(HeaderValue::from_static("application/octet-stream")),
    );
    if let Some(value) = file
        .disposition
        .and_then(|v| HeaderValue::from_str(&v).ok())
    {
        headers.insert(header::CONTENT_DISPOSITION, value);
    }
    Ok(response)
}
