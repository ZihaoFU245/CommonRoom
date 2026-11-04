//! Browser error projection. Engine failure classes never depend on text matching.
use crate::engine::EngineError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub(crate) struct ApiError {
    pub(crate) status: StatusCode,
    pub(crate) message: String,
    code: &'static str,
    retryable: bool,
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({
                "error": self.message,
                "code": self.code,
                "retryable": self.retryable,
            })),
        )
            .into_response()
    }
}

pub(crate) fn error(status: StatusCode, message: &str) -> ApiError {
    let code = match status {
        StatusCode::BAD_REQUEST => "invalid_request",
        StatusCode::UNAUTHORIZED => "unauthorized",
        StatusCode::FORBIDDEN => "forbidden",
        StatusCode::NOT_FOUND => "not_found",
        StatusCode::CONFLICT => "conflict",
        StatusCode::TOO_MANY_REQUESTS => "rate_limited",
        StatusCode::SERVICE_UNAVAILABLE => "unavailable",
        StatusCode::INTERNAL_SERVER_ERROR => "internal",
        _ => "request_failed",
    };
    ApiError {
        status,
        message: message.to_owned(),
        code,
        retryable: matches!(
            status,
            StatusCode::CONFLICT | StatusCode::TOO_MANY_REQUESTS | StatusCode::SERVICE_UNAVAILABLE
        ),
    }
}
impl From<EngineError> for ApiError {
    fn from(error: EngineError) -> Self {
        let status = match &error {
            EngineError::Invalid(_) => StatusCode::BAD_REQUEST,
            EngineError::NotFound(_) => StatusCode::NOT_FOUND,
            EngineError::Forbidden(_) => StatusCode::FORBIDDEN,
            EngineError::Conflict(_) => StatusCode::CONFLICT,
            EngineError::Unavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            EngineError::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        };
        self::error(status, error.message())
    }
}
impl IntoResponse for EngineError {
    fn into_response(self) -> Response {
        ApiError::from(self).into_response()
    }
}

impl From<crate::bots::error::Error> for ApiError {
    fn from(error: crate::bots::error::Error) -> Self {
        self::error(error.status(), &error.message)
    }
}

impl From<tokio::task::JoinError> for ApiError {
    fn from(error: tokio::task::JoinError) -> Self {
        EngineError::unavailable(error).into()
    }
}

pub(crate) fn hidden(error: EngineError, message: &str) -> ApiError {
    match error {
        EngineError::Unavailable(_) | EngineError::Internal(_) | EngineError::Conflict(_) => {
            error.into()
        }
        _ => EngineError::NotFound(message.into()).into(),
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn errors_keep_their_wire_contract_until_response_serialization() {
        for (error, status, code, retryable) in [
            (
                EngineError::Conflict("Changed".into()),
                StatusCode::CONFLICT,
                "conflict",
                true,
            ),
            (
                EngineError::Unavailable("Unavailable".into()),
                StatusCode::SERVICE_UNAVAILABLE,
                "unavailable",
                true,
            ),
            (
                EngineError::Internal("Internal".into()),
                StatusCode::INTERNAL_SERVER_ERROR,
                "internal",
                false,
            ),
        ] {
            let message = error.message().to_owned();
            let response = ApiError::from(error).into_response();
            assert_eq!(response.status(), status);
            let body = axum::body::to_bytes(response.into_body(), 1024)
                .await
                .unwrap();
            let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
            assert_eq!(
                value,
                serde_json::json!({"error":message,"code":code,"retryable":retryable})
            );
        }
        let bot = crate::bots::error::Error::from(error(StatusCode::FORBIDDEN, "Denied"));
        assert_eq!(bot.code, crate::bots::error::Code::Forbidden);
        assert_eq!(bot.message, "Denied");
        assert!(!bot.retryable);
    }
}
