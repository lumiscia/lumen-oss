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

    if token != &*state.config.secret {
        return Err(ApiError::unauthorized("invalid bearer token"));
    }

    Ok(())
}
