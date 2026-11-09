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

use crate::server::{RenderJobError, sanitized_diagnostics};

use super::MAX_REQUEST_BODY_BYTES;

/// A request that failed, answered with the status and a `{ "code", "error" }` body, which holds
/// `diagnostics` too when the composition was invalid.
#[derive(Debug)]
pub(super) struct ApiError {
    status: StatusCode,
    code: String,
    message: String,
    diagnostics: Vec<Diagnostic>,
}

#[derive(serde::Serialize)]
struct ApiErrorBody {
    code: String,
    error: String,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    diagnostics: Vec<Diagnostic>,
}

impl ApiError {
    pub fn new(status: StatusCode, code: &str, message: impl Into<String>) -> Self {
        Self {
            status,
            code: code.to_string(),
            message: message.into(),
            diagnostics: Vec::new(),
        }
    }

    pub fn rejected(status: StatusCode, error: RenderJobError) -> Self {
        Self::new(status, &error.code, error.message)
    }

    /// A `422` for a composition that doesn't parse, listing what is wrong with it.
    pub fn invalid_composition(error: &ParseError) -> Self {
        let mut rejected = Self::rejected(
            StatusCode::UNPROCESSABLE_ENTITY,
            RenderJobError::invalid_composition(&error.to_string()),
        );
        rejected.diagnostics = sanitized_diagnostics(error.diagnostics());
        rejected
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
        let mut rejected = Self::rejected(
            StatusCode::UNPROCESSABLE_ENTITY,
            RenderJobError::invalid_composition(&message),
        );
        rejected.diagnostics = sanitized_diagnostics(&errors);
        rejected
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
                diagnostics: self.diagnostics,
            }),
        )
            .into_response()
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
        StatusCode::NOT_FOUND => "route_not_found",
        StatusCode::METHOD_NOT_ALLOWED => "method_not_allowed",
        _ => "invalid_request",
    };
    let mut json = ApiError::new(status, code, message).into_response();
    if let Some(allow) = parts.headers.get(ALLOW) {
        json.headers_mut().insert(ALLOW, allow.clone());
    }
    json
}
