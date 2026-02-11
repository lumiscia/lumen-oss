use axum::{
    body::Body,
    extract::State,
    http::{Request, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

use crate::{api_error::ApiError, app_state::AppState};

pub async fn require_bearer(
    State(state): State<AppState>,
    request: Request<Body>,
    next: Next,
) -> Response {
    match validate(&state, &request) {
        Ok(()) => next.run(request).await,
        Err(err) => err.into_response(),
    }
}

fn validate(state: &AppState, request: &Request<Body>) -> Result<(), ApiError> {
    let value = request
        .headers()
        .get(header::AUTHORIZATION)
        .ok_or_else(|| ApiError::unauthorized("missing authorization header"))?;

    let value = value
        .to_str()
        .map_err(|_| ApiError::unauthorized("invalid authorization header"))?;

    let (scheme, token) = value
        .split_once(' ')
        .ok_or_else(|| ApiError::unauthorized("authorization header must be bearer token"))?;

    if !scheme.eq_ignore_ascii_case("bearer") {
        return Err(ApiError::unauthorized(
            "authorization header must use bearer scheme",
        ));
    }

    if token.is_empty() {
        return Err(ApiError::unauthorized("bearer token is empty"));
    }

    if !constant_time_eq(token.as_bytes(), state.config.secret.as_bytes()) {
        return Err(ApiError::unauthorized("invalid bearer token"));
    }

    Ok(())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let max_len = left.len().max(right.len());
    let mut diff: usize = left.len() ^ right.len();

    for index in 0..max_len {
        let l = left.get(index).copied().unwrap_or(0);
        let r = right.get(index).copied().unwrap_or(0);
        diff |= (l ^ r) as usize;
    }

    diff == 0
}

#[cfg(test)]
mod tests {
    use super::constant_time_eq;

    #[test]
    fn equality_matches_exact_bytes() {
        assert!(constant_time_eq(b"secret", b"secret"));
        assert!(!constant_time_eq(b"secret", b"Secret"));
        assert!(!constant_time_eq(b"secret", b"secretx"));
        assert!(!constant_time_eq(b"", b"x"));
    }
}
