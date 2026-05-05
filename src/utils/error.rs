use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("Not found: {0}")]
    NotFound(String),

    #[error("Unauthorized: {0}")]
    Unauthorized(String),

    #[error("Forbidden: {0}")]
    Forbidden(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Conflict: {0}")]
    Conflict(String),

    #[error("Rate limit exceeded: {0}")]
    RateLimitExceeded(String),

    #[error("Payload too large: {0}")]
    PayloadTooLarge(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Service unavailable: {0}")]
    ServiceUnavailable(String),

    #[error("Database error: {0}")]
    Database(#[from] sqlx::Error),

    #[error("JWT error: {0}")]
    Jwt(#[from] jsonwebtoken::errors::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),

    #[error("Invalid UUID: {0}")]
    Uuid(#[from] uuid::Error),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, message) = match &self {
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg.clone()),
            AppError::Unauthorized(msg) => {
                tracing::warn!("Unauthorized: {}", msg);
                (StatusCode::UNAUTHORIZED, msg.clone())
            }
            AppError::Forbidden(msg) => {
                tracing::warn!("Forbidden: {}", msg);
                (StatusCode::FORBIDDEN, msg.clone())
            }
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg.clone()),
            AppError::Conflict(msg) => (StatusCode::CONFLICT, msg.clone()),
            AppError::RateLimitExceeded(msg) => {
                tracing::warn!("Rate limit exceeded: {}", msg);
                (StatusCode::TOO_MANY_REQUESTS, msg.clone())
            }
            AppError::PayloadTooLarge(msg) => (StatusCode::PAYLOAD_TOO_LARGE, msg.clone()),
            AppError::Internal(msg) => {
                tracing::error!("Internal error: {}", msg);
                (StatusCode::INTERNAL_SERVER_ERROR, msg.clone())
            }
            AppError::ServiceUnavailable(msg) => {
                tracing::error!("Service unavailable: {}", msg);
                (StatusCode::SERVICE_UNAVAILABLE, msg.clone())
            }
            AppError::Database(e) => {
                tracing::error!("Database error: {}", e);
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Database error".to_string(),
                )
            }
            AppError::Jwt(e) => {
                tracing::error!("JWT error: {}", e);
                (StatusCode::UNAUTHORIZED, "Invalid token".to_string())
            }
            AppError::Http(e) => {
                tracing::error!("HTTP error: {}", e);
                (StatusCode::BAD_GATEWAY, "Upstream error".to_string())
            }
            AppError::Io(e) => {
                tracing::error!("IO error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "IO error".to_string())
            }
            AppError::Uuid(e) => {
                tracing::error!("UUID error: {}", e);
                (StatusCode::INTERNAL_SERVER_ERROR, "Invalid UUID".to_string())
            }
        };

        let body = Json(json!({
            "error": message,
            "status": status.as_u16()
        }));

        (status, body).into_response()
    }
}

pub type AppResult<T> = Result<T, AppError>;
