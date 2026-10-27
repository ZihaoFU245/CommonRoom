//! Domain failures retain their class until the transport chooses a response.
use std::fmt;

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineError {
    Invalid(String),
    NotFound(String),
    Forbidden(String),
    Conflict(String),
    Unavailable(String),
    Internal(String),
}
impl EngineError {
    pub fn message(&self) -> &str {
        match self {
            Self::Invalid(message)
            | Self::NotFound(message)
            | Self::Forbidden(message)
            | Self::Conflict(message)
            | Self::Unavailable(message)
            | Self::Internal(message) => message,
        }
    }
    pub fn code(&self) -> &'static str {
        match self {
            Self::Invalid(_) => "invalid_request",
            Self::NotFound(_) => "not_found",
            Self::Forbidden(_) => "forbidden",
            Self::Conflict(_) => "conflict",
            Self::Unavailable(_) => "unavailable",
            Self::Internal(_) => "internal",
        }
    }
    pub fn unavailable(cause: impl fmt::Display) -> Self {
        tracing::error!(error = %cause, "Backend storage or worker unavailable");
        Self::Unavailable("Storage unavailable; retry shortly.".into())
    }
}
impl fmt::Display for EngineError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.message())
    }
}
impl std::error::Error for EngineError {}
impl std::ops::Deref for EngineError {
    type Target = str;
    fn deref(&self) -> &str {
        self.message()
    }
}
impl From<&str> for EngineError {
    fn from(message: &str) -> Self {
        Self::Invalid(message.into())
    }
}
impl From<EngineError> for String {
    fn from(error: EngineError) -> Self {
        error.message().to_owned()
    }
}
impl From<super::database::Error> for EngineError {
    fn from(cause: super::database::Error) -> Self {
        match cause {
            super::database::Error::InvalidRequest(message) => Self::Invalid(message.into()),
            super::database::Error::InvalidData(message) => Self::Invalid(message),
            super::database::Error::Conflict(message) => Self::Conflict(message.into()),
            super::database::Error::RestartRequired => {
                Self::Unavailable("Storage unavailable; restart the service.".into())
            }
            super::database::Error::ReceiptConflict => {
                Self::Conflict("Key already belongs to another message request.".into())
            }
            super::database::Error::ReceiptCapacity => {
                Self::Unavailable("Wait for message receipts to expire.".into())
            }
            super::database::Error::MissingMedia => Self::NotFound("Upload not found.".into()),
            super::database::Error::RevisionConflict => {
                Self::Conflict("Conversation changed; retry the operation.".into())
            }
            cause => Self::unavailable(cause),
        }
    }
}
impl From<serde_json::Error> for EngineError {
    fn from(cause: serde_json::Error) -> Self {
        Self::unavailable(cause)
    }
}

impl From<super::HistoryError> for EngineError {
    fn from(error: super::HistoryError) -> Self {
        match error {
            super::HistoryError::Changed => {
                Self::Conflict("History changed; retry the current page.".into())
            }
            super::HistoryError::InvalidRange(message) => Self::Invalid(message),
            super::HistoryError::Unavailable(message) => Self::Unavailable(message),
        }
    }
}

impl From<std::io::Error> for EngineError {
    fn from(cause: std::io::Error) -> Self {
        Self::unavailable(cause)
    }
}
