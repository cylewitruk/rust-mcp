use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ApiError {
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

        let payload = ErrorPayload {
            error: self.to_string(),
        };

        (status, Json(payload)).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;
