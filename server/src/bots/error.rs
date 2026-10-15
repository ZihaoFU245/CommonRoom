use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum Code {
    InvalidRequest,
    Unauthorized,
    Forbidden,
    NotFound,
    MethodNotAllowed,
    Conflict,
    IdempotencyConflict,
    MessageGone,
    PayloadTooLarge,
    RateLimited,
    IdempotencyCapacity,
    RuntimeRejected,
    Unavailable,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub(crate) struct Error {
    pub code: Code,
    pub message: String,
    pub retryable: bool,
}
impl Error {
    pub fn new(code: Code, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            retryable: matches!(
                code,
                Code::Conflict | Code::RateLimited | Code::IdempotencyCapacity | Code::Unavailable
            ),
        }
    }
    pub fn storage(_: impl std::fmt::Display) -> Self {
        Self::new(Code::Unavailable, "Storage or worker unavailable.")
    }
    pub fn status(&self) -> StatusCode {
        match self.code {
            Code::InvalidRequest | Code::RuntimeRejected => StatusCode::BAD_REQUEST,
            Code::Unauthorized => StatusCode::UNAUTHORIZED,
            Code::Forbidden => StatusCode::FORBIDDEN,
            Code::NotFound => StatusCode::NOT_FOUND,
            Code::MethodNotAllowed => StatusCode::METHOD_NOT_ALLOWED,
            Code::Conflict | Code::IdempotencyConflict => StatusCode::CONFLICT,
            Code::MessageGone => StatusCode::GONE,
            Code::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Code::RateLimited | Code::IdempotencyCapacity => StatusCode::TOO_MANY_REQUESTS,
            Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
        }
    }
}
impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}
impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();
        let retry = match self.code {
            Code::RateLimited => Some("10"),
            Code::IdempotencyCapacity => Some("86400"),
            _ => None,
        };
        let mut response = (status, Json(serde_json::json!({"error":self}))).into_response();
        if let Some(retry) = retry {
            response
                .headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static(retry));
        }
        response
    }
}
