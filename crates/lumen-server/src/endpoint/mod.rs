mod renders;

use axum::{
    Router,
    http::{Method, header},
    middleware,
    routing::{get, post},
};
use tower_http::cors::{Any, CorsLayer};

use crate::{app_state::AppState, middleware::authorization};

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_origin(Any)
        .allow_methods([Method::GET, Method::POST, Method::OPTIONS])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE]);

    Router::new()
        .route("/renders", get(renders::list_renders))
        .route("/renders", post(renders::create_render))
        .route("/renders/{job_id}/cancel", post(renders::cancel_render))
        .route("/renders/{job_id}/retry", post(renders::retry_render))
        .route("/renders/{job_id}", get(renders::get_render))
        .route(
            "/renders/{job_id}/events",
            get(renders::stream_render_events),
        )
        .route("/renders/{job_id}/artifact", get(renders::get_artifact))
        .route(
            "/renders/{job_id}/frames/{frame_index}",
            get(renders::get_frame),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authorization::require_bearer,
        ))
        .layer(cors)
        .with_state(state)
}
