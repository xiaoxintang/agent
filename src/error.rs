//! 将内部错误和上游 gRPC 状态转换为统一的 HTTP 错误响应。

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use utoipa::ToSchema;

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("{0}")]
    BadRequest(String),
    #[error("request body exceeds 4 MiB")]
    PayloadTooLarge,
    #[error("{0}")]
    Conflict(String),
    #[error("{0}")]
    NotFound(String),
    #[error("{0}")]
    Validation(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Unavailable(String),
    // 调用可能已经生效，不能当作普通失败重试；需核对运行时或显式重启恢复。
    #[error("{0}")]
    Uncertain(String),
    #[error("gRPC {code:?}: {message}")]
    Grpc { code: tonic::Code, message: String },
    #[error("{0}")]
    Internal(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Sqlite(#[from] rusqlite::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Serialize, ToSchema)]
pub struct ErrorBody {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grpc_code: Option<String>,
}

impl Error {
    // 只有明确未执行的错误才能直接废弃候选配置；其余情况保留恢复记录。
    pub fn definitely_not_applied(&self) -> bool {
        matches!(
            self,
            Self::BadRequest(_) | Self::Validation(_) | Self::Unsupported(_)
        ) || matches!(
            self,
            Self::Grpc {
                code: tonic::Code::InvalidArgument
                    | tonic::Code::NotFound
                    | tonic::Code::AlreadyExists
                    | tonic::Code::Unimplemented,
                ..
            }
        )
    }

    pub fn status(&self) -> StatusCode {
        // HTTP 表达错误类别，grpc_code 仍保留上游状态，便于管理端准确判断。
        match self {
            Self::BadRequest(_) => StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge => StatusCode::PAYLOAD_TOO_LARGE,
            Self::Conflict(_) => StatusCode::CONFLICT,
            Self::NotFound(_) => StatusCode::NOT_FOUND,
            Self::Validation(_) => StatusCode::UNPROCESSABLE_ENTITY,
            Self::Unsupported(_) => StatusCode::NOT_IMPLEMENTED,
            Self::Unavailable(_) | Self::Uncertain(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::Grpc { code, .. } => match code {
                tonic::Code::InvalidArgument => StatusCode::BAD_REQUEST,
                tonic::Code::NotFound => StatusCode::NOT_FOUND,
                tonic::Code::AlreadyExists | tonic::Code::Aborted => StatusCode::CONFLICT,
                tonic::Code::FailedPrecondition => StatusCode::UNPROCESSABLE_ENTITY,
                tonic::Code::Unimplemented => StatusCode::NOT_IMPLEMENTED,
                tonic::Code::Unavailable => StatusCode::SERVICE_UNAVAILABLE,
                tonic::Code::DeadlineExceeded => StatusCode::GATEWAY_TIMEOUT,
                _ => StatusCode::BAD_GATEWAY,
            },
            _ => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl From<tonic::Status> for Error {
    fn from(status: tonic::Status) -> Self {
        Self::Grpc {
            code: status.code(),
            message: status.message().to_owned(),
        }
    }
}

impl IntoResponse for Error {
    fn into_response(self) -> Response {
        let status = self.status();
        let grpc_code = match &self {
            Self::Grpc { code, .. } => Some(format!("{code:?}")),
            _ => None,
        };
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            // 内部路径或数据库细节只写日志，不直接暴露给远程调用方。
            tracing::error!(error = %self, "request failed");
            "internal error; inspect the agent journal".to_owned()
        } else {
            self.to_string()
        };
        (
            status,
            Json(ErrorBody {
                error: message,
                grpc_code,
            }),
        )
            .into_response()
    }
}
