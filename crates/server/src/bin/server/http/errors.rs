use axum::{
    Json,
    body::to_bytes,
    extract::{Request, rejection::JsonRejection},
    http::{
        StatusCode,
        header::{ALLOW, CONTENT_TYPE},
    },
    middleware::Next,
    response::{IntoResponse, Response},
};

use lumen_engine::{diagnostic::Diagnostic, json::ParseError};
use lumen_server_api::{ErrorBody, ErrorCode};
use serde_json::Value;

use crate::server::{RenderJobError, sanitized_diagnostics};

use super::MAX_REQUEST_BODY_BYTES;

/// A request that failed, answered with the status and an [`ErrorBody`], which holds
/// `diagnostics` too when the composition was invalid and `media` when a media source failed.
#[derive(Debug)]
pub(super) struct ApiError {
    status: StatusCode,
    body: ErrorBody,
}

impl ApiError {
    pub fn new(status: StatusCode, code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            status,
            body: ErrorBody {
                code,
                error: message.into(),
                diagnostics: Vec::new(),
                media: None,
            },
        }
    }

    pub fn rejected(status: StatusCode, error: RenderJobError) -> Self {
        let mut rejected = Self::new(status, error.code, error.message);
        rejected.body.media = error.media;
        rejected
    }

    fn with_diagnostics(mut self, diagnostics: Vec<Value>) -> Self {
        self.body.diagnostics = diagnostics;
        self
    }

    /// A `422` for a composition that doesn't parse, listing what is wrong with it.
    pub fn invalid_composition(error: &ParseError) -> Self {
        Self::rejected(
            StatusCode::UNPROCESSABLE_ENTITY,
            RenderJobError::invalid_composition(&error.to_string()),
        )
        .with_diagnostics(sanitized_diagnostics(error.diagnostics()))
    }

    /// A `422` for a composition that parses but can't render here, such as one naming an asset
    /// this server doesn't have, listing every error in `diagnostics`.
    pub fn invalid_diagnostics(diagnostics: &[Diagnostic]) -> Self {
        let errors: Vec<Diagnostic> = diagnostics
            .iter()
            .filter(|diagnostic| diagnostic.is_error())
            .cloned()
            .collect();
        let message = errors
            .iter()
            .map(|diagnostic| diagnostic.message.as_str())
            .collect::<Vec<_>>()
            .join("; ");
        Self::rejected(
            StatusCode::UNPROCESSABLE_ENTITY,
            RenderJobError::invalid_composition(&message),
        )
        .with_diagnostics(sanitized_diagnostics(&errors))
    }

    /// A `503` for a render that would wait behind more than `--max-queued-renders` others. No
    /// `Retry-After` is sent, since how long the queue takes to drain depends on the renders in it.
    pub fn queue_full(max_queued: usize) -> Self {
        Self::new(
            StatusCode::SERVICE_UNAVAILABLE,
            ErrorCode::RenderQueueFull,
            format!(
                "every render slot is busy and {max_queued} renders are already waiting; try again later"
            ),
        )
    }

    pub fn not_found() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            ErrorCode::RenderNotFound,
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
                ErrorCode::RequestTooLarge,
                format!("request body exceeds the {MAX_REQUEST_BODY_BYTES} byte limit"),
            )
        } else {
            Self::new(status, ErrorCode::InvalidRequestBody, rejection.body_text())
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (self.status, Json(self.body)).into_response()
    }
}

/// Gives the errors axum answers by itself, such as unknown routes, wrong methods and bad path
/// segments, the same body as every other error.
pub(super) async fn json_errors(request: Request, next: Next) -> Response {
    let response = next.run(request).await;
    let status = response.status();
    let is_json = response
        .headers()
        .get(CONTENT_TYPE)
        .is_some_and(|value| value.as_bytes().starts_with(b"application/json"));
    if !(status.is_client_error() || status.is_server_error()) || is_json {
        return response;
    }
    let (parts, body) = response.into_parts();
    let text = to_bytes(body, 4096).await.unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let message = match text.trim() {
        "" => status.canonical_reason().unwrap_or("request failed"),
        text => text,
    };
    let code = match status {
        StatusCode::NOT_FOUND => ErrorCode::RouteNotFound,
        StatusCode::METHOD_NOT_ALLOWED => ErrorCode::MethodNotAllowed,
        _ => ErrorCode::InvalidRequest,
    };
    let mut json = ApiError::new(status, code, message).into_response();
    if let Some(allow) = parts.headers.get(ALLOW) {
        json.headers_mut().insert(ALLOW, allow.clone());
    }
    json
}
