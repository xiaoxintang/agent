use axum::{
    Json,
    extract::{FromRequest, Request, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use panel_common::{ApiError, ErrorResponse};
use serde::de::DeserializeOwned;

#[derive(Debug, Clone)]
pub struct AppError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: &'static str,
    pub(crate) database: Option<DatabaseError>,
}

#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct DatabaseError {
    kind: &'static str,
    code: Option<String>,
}

impl AppError {
    pub const fn new(status: StatusCode, code: &'static str, message: &'static str) -> Self {
        Self {
            status,
            code,
            message,
            database: None,
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
            "CSRF validation failed",
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
    fn from(error: sqlx::Error) -> Self {
        let kind = match &error {
            sqlx::Error::Database(_) => "database",
            sqlx::Error::PoolTimedOut => "pool_timeout",
            sqlx::Error::PoolClosed => "pool_closed",
            sqlx::Error::Io(_) => "io",
            sqlx::Error::RowNotFound => "row_not_found",
            sqlx::Error::ColumnDecode { .. } | sqlx::Error::Decode(_) => "decode",
            _ => "sqlx",
        };
        let code = error
            .as_database_error()
            .and_then(|error| error.code())
            .filter(|code| code.len() <= 16 && code.bytes().all(|byte| byte.is_ascii_digit()))
            .map(|code| code.into_owned());
        let mut result = Self::internal();
        result.database = Some(DatabaseError { kind, code });
        result
    }
}
impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let mut response = (
            self.status,
            [(header::CACHE_CONTROL, "no-store")],
            Json(ErrorResponse {
                error: ApiError {
                    code: self.code,
                    message: self.message,
                },
            }),
        )
            .into_response();
        response.extensions_mut().insert(self);
        response
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
