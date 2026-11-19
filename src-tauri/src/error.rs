//! Application error type shared by every Tauri command.
//!
//! Errors cross the IPC boundary as `{ code, message }` so the frontend can
//! branch on a stable machine-readable code instead of parsing strings.

use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// kubeconfig missing, unreadable or malformed.
    Kubeconfig,
    /// Could not build a client or reach the API server.
    Connect,
    /// API server answered 401.
    Unauthorized,
    /// API server answered 403 (RBAC).
    Forbidden,
    /// Object or API not found (404).
    NotFound,
    /// Any other API server error.
    Api,
    /// Unknown view/subscription id or invalid argument from the frontend.
    InvalidRequest,
    /// Bug or unexpected internal failure.
    Internal,
}

#[derive(Debug, Error)]
pub enum AppError {
    #[error("kubeconfig: {0}")]
    Kubeconfig(String),

    #[error("{0}")]
    Connect(String),

    #[error("{0}")]
    Kube(#[from] kube::Error),

    #[error("{0}")]
    InvalidRequest(String),

    #[error("{0}")]
    Internal(String),
}

impl AppError {
    pub fn code(&self) -> ErrorCode {
        match self {
            AppError::Kubeconfig(_) => ErrorCode::Kubeconfig,
            AppError::Connect(_) => ErrorCode::Connect,
            AppError::InvalidRequest(_) => ErrorCode::InvalidRequest,
            AppError::Internal(_) => ErrorCode::Internal,
            AppError::Kube(err) => kube_error_code(err),
        }
    }
}

/// Maps a kube client error to the code shown to the user.
pub fn kube_error_code(err: &kube::Error) -> ErrorCode {
    match err {
        kube::Error::Api(status) => match status.code {
            401 => ErrorCode::Unauthorized,
            403 => ErrorCode::Forbidden,
            404 => ErrorCode::NotFound,
            _ => ErrorCode::Api,
        },
        kube::Error::Auth(_) => ErrorCode::Unauthorized,
        kube::Error::HyperError(_)
        | kube::Error::Service(_)
        | kube::Error::HttpError(_)
        | kube::Error::RustlsTls(_)
        | kube::Error::TlsRequired
        | kube::Error::InferConfig(_) => ErrorCode::Connect,
        _ => ErrorCode::Api,
    }
}

/// Returns true when the error is an RBAC denial.
pub fn is_forbidden(err: &kube::Error) -> bool {
    kube_error_code(err) == ErrorCode::Forbidden
}

/// Error payload sent to the frontend, both as command errors and inside
/// streamed subscription events.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorInfo {
    pub code: ErrorCode,
    pub message: String,
}

impl ErrorInfo {
    pub fn from_kube(err: &kube::Error) -> Self {
        ErrorInfo {
            code: kube_error_code(err),
            message: err.to_string(),
        }
    }
}

impl From<&AppError> for ErrorInfo {
    fn from(err: &AppError) -> Self {
        ErrorInfo {
            code: err.code(),
            message: err.to_string(),
        }
    }
}

impl Serialize for AppError {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        ErrorInfo::from(self).serialize(serializer)
    }
}

pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_code_and_message() {
        let err = AppError::InvalidRequest("unknown view 7".into());
        let json = serde_json::to_value(&err).unwrap();
        assert_eq!(json["code"], "invalid_request");
        assert_eq!(json["message"], "unknown view 7");
    }
}
