mod renders;

use axum::{
    Router, middleware,
    routing::{get, post},
};

use crate::{app_state::AppState, middleware::authorization};

pub fn build_router(state: AppState) -> Router {
    Router::new()
        .route("/renders", get(renders::list_renders))
        .route("/renders", post(renders::create_render))
        .route("/renders/{job_id}/cancel", post(renders::cancel_render))
        .route("/renders/{job_id}/retry", post(renders::retry_render))
        .route("/renders/{job_id}", get(renders::get_render))
        .route("/renders/{job_id}/artifact", get(renders::get_artifact))
        .route(
            "/renders/{job_id}/frames/{frame_index}",
            get(renders::get_frame),
        )
        .layer(middleware::from_fn_with_state(
            state.clone(),
            authorization::require_bearer,
        ))
        .with_state(state)
}
