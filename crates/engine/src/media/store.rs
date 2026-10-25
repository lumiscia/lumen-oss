use std::fmt::Debug;

use crate::audio::AudioResolver;

use super::{FontResolver, ImageResolver, VideoFrameResolver};

pub trait MediaStore: Send + Sync + Debug {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>>;

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>>;

    fn get_audio_resolver(&self, _source: &str) -> Option<Box<dyn AudioResolver>> {
        None
    }

    /// Returns the current font resolver ID without loading font data.
    /// Must match the resolver's ID and change whenever its font data changes.
    fn font_resolver_id(&self, _source: &str) -> Option<String> {
        None
    }

    fn get_font_resolver(&self, _source: &str) -> Option<Box<dyn FontResolver>> {
        None
    }
}
