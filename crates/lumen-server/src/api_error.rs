use std::sync::atomic::{AtomicU64, Ordering};

use axum::{
    Json,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde::Serialize;
use tracing::error;

use crate::jobs::StorageError;

static INTERNAL_ERROR_COUNTER: AtomicU64 = AtomicU64::new(1);

#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    pub fn unauthorized(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::UNAUTHORIZED,
            code: "unauthorized",
            message: message.into(),
        }
    }

    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "bad_request",
            message: message.into(),
        }
    }

    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "not_found",
            message: message.into(),
        }
    }

    pub fn internal<E: std::fmt::Display>(err: E) -> Self {
        let error_id = INTERNAL_ERROR_COUNTER.fetch_add(1, Ordering::Relaxed);
        error!(error_id, "internal server error: {err}");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            code: "internal_error",
            message: format!("internal server error (id: {error_id})"),
        }
    }
}

impl From<axum::http::header::InvalidHeaderValue> for ApiError {
    fn from(err: axum::http::header::InvalidHeaderValue) -> Self {
        Self::bad_request(format!("invalid header value: {err}"))
    }
}

impl From<StorageError> for ApiError {
    fn from(err: StorageError) -> Self {
        match err {
            StorageError::CapacityExceeded { resource, limit } => Self {
                status: StatusCode::SERVICE_UNAVAILABLE,
                code: "capacity_exceeded",
                message: format!("{resource} capacity exceeded (limit: {limit})"),
            },
            StorageError::NotFound { resource, id } => Self {
                status: StatusCode::NOT_FOUND,
                code: "not_found",
                message: format!("{resource} not found: {id}"),
            },
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let payload = Json(ErrorPayload {
            error: ErrorBody {
                code: self.code,
                message: self.message,
            },
        });

        (self.status, payload).into_response()
    }
}

#[derive(Serialize)]
struct ErrorPayload {
    error: ErrorBody,
}

#[derive(Serialize)]
struct ErrorBody {
    code: &'static str,
    message: String,
}
