use axum::{Json, http::HeaderMap, response::IntoResponse};

use crate::server::{RenderJobError, RenderJobResponse};

use super::state::AppState;

pub(super) fn authorize_response(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<axum::response::Response> {
    if let Some(expected_token) = state.api_token.as_deref() {
        let actual_token = headers
            .get(axum::http::header::AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "));
        if actual_token != Some(expected_token) {
            return Some(
                (
                    axum::http::StatusCode::UNAUTHORIZED,
                    Json(RenderJobResponse {
                        ok: false,
                        artifact: None,
                        metrics: None,
                        error: Some(RenderJobError {
                            code: "unauthorized".to_string(),
                            message: "invalid render API token".to_string(),
                            retryable: false,
                        }),
                    }),
                )
                    .into_response(),
            );
        }
    }
    None
}
