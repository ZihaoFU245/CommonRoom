//! Browser error projection. Engine failure classes never depend on text matching.
use crate::engine::EngineError;
use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};

#[derive(Debug)]
pub(crate) struct ApiError(pub StatusCode, pub Json<serde_json::Value>);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.0, self.1).into_response()
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
        Self(
            status,
            Json(
                serde_json::json!({"error":error.message(),"code":error.code(),"retryable":matches!(error, EngineError::Unavailable(_) | EngineError::Conflict(_))}),
            ),
        )
    }
}
impl IntoResponse for EngineError {
    fn into_response(self) -> Response {
        ApiError::from(self).into_response()
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
