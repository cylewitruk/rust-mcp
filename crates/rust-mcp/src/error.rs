use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use thiserror::Error;

/// API-level error type mapped to HTTP responses.
#[derive(Debug, Error)]
pub enum ApiError {
    /// Database access failure.
    #[error("database unavailable: {0}")]
    Database(#[from] sqlx::Error),
}

#[derive(Debug, Serialize)]
struct ErrorPayload {
    error: String,
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let status = match self {
            ApiError::Database(_) => StatusCode::SERVICE_UNAVAILABLE,
        };

        let payload = ErrorPayload { error: self.to_string() };

        (status, Json(payload)).into_response()
    }
}

/// Convenience result alias for API handlers.
pub type ApiResult<T> = Result<T, ApiError>;
