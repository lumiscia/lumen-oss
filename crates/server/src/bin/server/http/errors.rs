use axum::{
    Json,
    extract::rejection::JsonRejection,
    http::StatusCode,
    response::{IntoResponse, Response},
};

use crate::server::RenderJobError;

use super::MAX_REQUEST_BODY_BYTES;

/// A request that failed, answered with the status and a `{ "code", "error" }` body.
#[derive(Debug)]
pub(super) struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
}

#[derive(serde::Serialize)]
struct ApiErrorBody {
    code: String,
    error: String,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
        }
    }

    pub fn rejected(status: StatusCode, error: RenderJobError) -> Self {
        Self {
            status,
            code: error.code,
            message: error.message,
        }
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "render_not_found",
            "render was not found",
        )
    }
}

impl From<JsonRejection> for ApiError {
    fn from(rejection: JsonRejection) -> Self {
        let status = rejection.status();
        if status == StatusCode::PAYLOAD_TOO_LARGE {
            Self::new(
                status,
                "request_too_large",
                format!("request body exceeds the {MAX_REQUEST_BODY_BYTES} byte limit"),
            )
        } else {
            Self::new(status, "invalid_request_body", rejection.body_text())
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                code: self.code,
                error: self.message,
            }),
        )
            .into_response()
    }
}
