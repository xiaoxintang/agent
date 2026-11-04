use axum::{
    Json,
    extract::{FromRequest, Request, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use panel_common::{ApiError, ErrorResponse};
use serde::de::DeserializeOwned;

#[derive(Debug)]
pub struct AppError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
}

impl AppError {
    pub const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
        }
    }
    pub const fn invalid() -> Self {
        Self::new(StatusCode::BAD_REQUEST, "invalid_input", "Invalid input")
    }
    pub const fn unauthorized() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "unauthorized",
            "Authentication required or credentials invalid",
        )
    }
    pub const fn forbidden() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf_failed",
            "Origin or CSRF validation failed",
        )
    }
    pub const fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "Internal server error",
        )
    }
    pub const fn not_found() -> Self {
        Self::new(StatusCode::NOT_FOUND, "not_found", "Resource not found")
    }
    pub const fn limited() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "rate_limited",
            "Too many login attempts; try again later",
        )
    }
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for AppError {}
impl From<sqlx::Error> for AppError {
    fn from(_: sqlx::Error) -> Self {
        Self::internal()
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (
            self.status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(ErrorResponse {
                error: ApiError {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}

pub struct ApiJson<T>(pub T);
impl<T: DeserializeOwned, S: Send + Sync> FromRequest<S> for ApiJson<T> {
    type Rejection = AppError;
    async fn from_request(request: Request, state: &S) -> Result<Self, Self::Rejection> {
        Json::<T>::from_request(request, state)
            .await
            .map(|Json(value)| Self(value))
            .map_err(|error: JsonRejection| {
                let status = if error.status() == StatusCode::UNPROCESSABLE_ENTITY {
                    StatusCode::BAD_REQUEST
                } else {
                    error.status()
                };
                AppError::new(
                    status,
                    "invalid_json",
                    "Expected a valid JSON request within 16 KiB",
                )
            })
    }
}
