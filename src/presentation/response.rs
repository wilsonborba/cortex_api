use crate::core::process_numbers::ProcessNumber;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;

/// Shared HTTP body and SSE data body.
#[derive(Debug, Serialize)]
pub struct Envelope<T> {
    pub message: String,
    pub data: T,
}

#[derive(Debug, Serialize)]
pub struct ErrorData {
    pub error_number: u16,
}
#[derive(Debug, Serialize)]
pub struct NoticeData<T> {
    pub notice_number: u16,
    #[serde(flatten)]
    pub details: T,
}

impl<T> Envelope<T> {
    pub fn new(message: impl Into<String>, data: T) -> Self {
        Self {
            message: message.into(),
            data,
        }
    }
}
impl Envelope<ErrorData> {
    pub fn error(process: ProcessNumber) -> Self {
        Self::new(
            process.message,
            ErrorData {
                error_number: process.number,
            },
        )
    }
}
impl<T> Envelope<NoticeData<T>> {
    pub fn notice(process: ProcessNumber, details: T) -> Self {
        Self::new(
            process.message,
            NoticeData {
                notice_number: process.number,
                details,
            },
        )
    }
}

#[derive(Clone)]
pub(crate) struct StructuredResponse;

impl<T: Serialize> IntoResponse for Envelope<T> {
    fn into_response(self) -> Response {
        let mut response = Json(self).into_response();
        response.extensions_mut().insert(StructuredResponse);
        response
    }
}

pub fn error(status: StatusCode, process: ProcessNumber) -> Response {
    (status, Envelope::error(process)).into_response()
}
