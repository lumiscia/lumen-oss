use axum::{
    extract::{Request, State},
    http::{StatusCode, header::AUTHORIZATION},
    middleware::Next,
    response::Response,
};
use subtle::ConstantTimeEq;

use crate::server::RenderJobError;

use super::{errors::error_response, state::AppState};

pub(super) async fn require_token(
    State(state): State<AppState>,
    request: Request,
    next: Next,
) -> Response {
    if let Some(expected_token) = state.api_token.as_deref() {
        let authorized = request
            .headers()
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .is_some_and(|token| bool::from(token.as_bytes().ct_eq(expected_token.as_bytes())));
        if !authorized {
            return error_response(
                StatusCode::UNAUTHORIZED,
                RenderJobError {
                    code: "unauthorized".to_string(),
                    message: "invalid render API token".to_string(),
                    retryable: false,
                },
            );
        }
    }
    next.run(request).await
}
