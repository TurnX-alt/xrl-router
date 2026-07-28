use thiserror::Error;

/// Unified error types for the application.
#[derive(Error, Debug)]
pub enum AppError {
    #[error("Database error: {0}")]
    Database(#[from] rusqlite::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("Authentication failed")]
    Unauthorized,

    #[error("Resource not found: {0}")]
    NotFound(String),

    #[error("Internal error: {0}")]
    Internal(String),

    #[error("Bad request: {0}")]
    BadRequest(String),

    #[error("Rate limited")]
    RateLimited,

    #[error("Encryption error: {0}")]
    EncryptionError(String),
}

impl From<AppError> for axum::response::Response {
    fn from(err: AppError) -> Self {
        use axum::http::StatusCode;
        use axum::response::IntoResponse;

        let (status, code) = match &err {
            AppError::Database(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            AppError::Json(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            AppError::Http(_) => (StatusCode::BAD_GATEWAY, "BAD_GATEWAY"),
            AppError::Unauthorized => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            AppError::NotFound(_) => (StatusCode::NOT_FOUND, "NOT_FOUND"),
            AppError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL_ERROR"),
            AppError::BadRequest(_) => (StatusCode::BAD_REQUEST, "BAD_REQUEST"),
            AppError::RateLimited => (StatusCode::TOO_MANY_REQUESTS, "RATE_LIMITED"),
            AppError::EncryptionError(_) => (StatusCode::INTERNAL_SERVER_ERROR, "ENCRYPTION_ERROR"),
        };
        let message = err.to_string();

        (
            status,
            axum::Json(serde_json::json!({"error": {"code": code, "message": message}})),
        )
            .into_response()
    }
}
