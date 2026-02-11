use crate::{skia::Image, time::FrameIndex};
use thiserror::Error;

#[derive(Debug, Error)]
pub enum MediaError {
    #[error("asset `{0}` was not found")]
    MissingAsset(String),
    #[error("media decode failed: {0}")]
    Decode(String),
    #[error("media source error: {0}")]
    Source(String),
}

pub trait MediaProvider: Send {
    fn image(&mut self, _asset_id: &str) -> Result<Option<Image>, MediaError> {
        Ok(None)
    }

    fn video_frame(
        &mut self,
        _asset_id: &str,
        _frame: FrameIndex,
    ) -> Result<Option<Image>, MediaError> {
        Ok(None)
    }

    fn svg_bytes(&mut self, _asset_id: &str) -> Result<Option<Vec<u8>>, MediaError> {
        Ok(None)
    }
}

#[derive(Default)]
pub struct NoopMediaProvider;

impl MediaProvider for NoopMediaProvider {}
