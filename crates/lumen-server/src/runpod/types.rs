use serde::Deserialize;

use crate::server::RenderJobInput;

#[derive(Debug, Deserialize)]
pub struct RunpodJobRequest {
    pub input: RenderJobInput,
}
