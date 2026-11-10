use axum::http::StatusCode;
use serde::Serialize;

/// Stable process numbers. Add numbers here. Never reuse a number.
#[derive(Clone, Copy, Debug, Serialize)]
pub struct ProcessNumber {
    pub number: u16,
    pub message: &'static str,
}

pub const NOT_FOUND: ProcessNumber = ProcessNumber {
    number: 1001,
    message: "The route was not found",
};
pub const WRONG_METHOD: ProcessNumber = ProcessNumber {
    number: 1002,
    message: "The method is not allowed",
};
pub const INVALID_DATA: ProcessNumber = ProcessNumber {
    number: 1003,
    message: "The request data is not valid",
};
pub const BAD_JSON: ProcessNumber = ProcessNumber {
    number: 1004,
    message: "The JSON body is not valid",
};
pub const MISSING_USER: ProcessNumber = ProcessNumber {
    number: 1005,
    message: "The x-uuid header is required",
};
pub const WRONG_CONTENT_TYPE: ProcessNumber = ProcessNumber {
    number: 1006,
    message: "The content type is not supported",
};
pub const BODY_TOO_LARGE: ProcessNumber = ProcessNumber {
    number: 1007,
    message: "The request body is too large",
};
pub const SERVER_ERROR: ProcessNumber = ProcessNumber {
    number: 1008,
    message: "The server could not complete the request",
};
pub const UNKNOWN_APP: ProcessNumber = ProcessNumber {
    number: 1009,
    message: "The app_id is not allowed",
};
pub const HEAVY_LOAD: ProcessNumber = ProcessNumber {
    number: 1010,
    message: "The server is under heavy load. Try later",
};
pub const QUEUE_FAULT: ProcessNumber = ProcessNumber {
    number: 1011,
    message: "The local call queue failed",
};
pub const OLLAMA_FAILED: ProcessNumber = ProcessNumber {
    number: 1012,
    message: "The local model call failed",
};
pub const INPUT_TOKEN_LIMIT: ProcessNumber = ProcessNumber {
    number: 1201,
    message: "The input is over the token limit",
};
pub const USAGE_UNAVAILABLE: ProcessNumber = ProcessNumber {
    number: 1013,
    message: "The usage data is not available",
};
pub const SECURITY_BLOCK: ProcessNumber = ProcessNumber {
    number: 1014,
    message: "Are you kidding me, clown?",
};
pub const SECURITY_UNAVAILABLE: ProcessNumber = ProcessNumber {
    number: 1015,
    message: "The input security check is not available",
};
pub const CHAT_NOT_FOUND: ProcessNumber = ProcessNumber {
    number: 1016,
    message: "The chat was not found",
};
pub const HISTORY_FAILED: ProcessNumber = ProcessNumber {
    number: 1017,
    message: "The chat history could not be loaded or changed",
};
pub const OUTPUT_NOT_FOUND: ProcessNumber = ProcessNumber {
    number: 1018,
    message: "The output was not found",
};
pub const MEMORY_SAVE_FAILED: ProcessNumber = ProcessNumber {
    number: 1019,
    message: "The exchange memory could not be saved",
};
pub const WEB_SEARCH_FAILED: ProcessNumber = ProcessNumber {
    number: 1020,
    message: "Web search is not available",
};
pub const REGISTRY: &[ProcessNumber] = &[
    NOT_FOUND,
    WRONG_METHOD,
    INVALID_DATA,
    BAD_JSON,
    MISSING_USER,
    WRONG_CONTENT_TYPE,
    BODY_TOO_LARGE,
    SERVER_ERROR,
    UNKNOWN_APP,
    HEAVY_LOAD,
    QUEUE_FAULT,
    OLLAMA_FAILED,
    INPUT_TOKEN_LIMIT,
    USAGE_UNAVAILABLE,
    SECURITY_BLOCK,
    SECURITY_UNAVAILABLE,
    CHAT_NOT_FOUND,
    HISTORY_FAILED,
    OUTPUT_NOT_FOUND,
    MEMORY_SAVE_FAILED,
    WEB_SEARCH_FAILED,
];

pub fn for_status(status: StatusCode) -> ProcessNumber {
    match status {
        StatusCode::NOT_FOUND => NOT_FOUND,
        StatusCode::METHOD_NOT_ALLOWED => WRONG_METHOD,
        StatusCode::UNPROCESSABLE_ENTITY => INVALID_DATA,
        StatusCode::BAD_REQUEST => BAD_JSON,
        StatusCode::UNSUPPORTED_MEDIA_TYPE => WRONG_CONTENT_TYPE,
        StatusCode::PAYLOAD_TOO_LARGE => BODY_TOO_LARGE,
        _ => SERVER_ERROR,
    }
}
