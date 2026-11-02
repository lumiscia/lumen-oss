use axum::{Json, http::StatusCode, response::IntoResponse};

use crate::server::{RenderJobError, RenderJobResponse};

pub(super) fn error_response(
    status: StatusCode,
    error: RenderJobError,
) -> axum::response::Response {
    (
        status,
        Json(RenderJobResponse {
            ok: false,
            artifact: None,
            metrics: None,
            error: Some(error),
        }),
    )
        .into_response()
}

pub(super) fn not_found_response() -> axum::response::Response {
    error_response(
        StatusCode::NOT_FOUND,
        RenderJobError {
            code: "render_not_found".to_string(),
            message: "render was not found".to_string(),
            retryable: false,
        },
    )
}
