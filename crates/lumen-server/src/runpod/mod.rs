mod types;
pub mod worker;

pub use types::RunpodJobRequest;

use crate::server::{RenderJobResponse, handle_render_job};

pub async fn handle_runpod_request(request: RunpodJobRequest) -> RenderJobResponse {
    handle_render_job(request.input).await
}
