use axum::{Json, http::StatusCode, response::IntoResponse};

use crate::server::{RenderJobError, RenderJobResponse};

pub(super) fn not_found_response() -> axum::response::Response {
    (
        StatusCode::NOT_FOUND,
        Json(RenderJobResponse {
            ok: false,
            artifact: None,
            metrics: None,
            error: Some(RenderJobError {
                code: "render_not_found".to_string(),
                message: "render was not found".to_string(),
                retryable: false,
            }),
        }),
    )
        .into_response()
}
