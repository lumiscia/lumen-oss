use std::fmt::Debug;
use std::sync::Mutex;

use crate::audio::AudioResolver;

use super::{FontResolver, ImageResolver, VideoFrameResolver};

/// The fonts one store has loaded and what was shaped and measured with them. Nothing rendered
/// with another store reads them.
#[derive(Debug)]
pub struct TextSession(pub(crate) Mutex<lumen_text::TextSystem>);

impl Default for TextSession {
    fn default() -> Self {
        Self(Mutex::new(lumen_text::TextSystem::new()))
    }
}

pub trait MediaStore: Send + Sync + Debug {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>>;

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>>;

    fn get_audio_resolver(&self, _source: &str) -> Option<Box<dyn AudioResolver>> {
        None
    }

    /// Returns the current font resolver ID without loading font data, or `None` when the store
    /// has no such font. Must match the resolver's ID and change whenever its font data changes.
    /// Resolvers with one ID, from any store, must serve the same data, as they may share it.
    fn font_resolver_id(&self, _source: &str) -> Option<String> {
        None
    }

    fn get_font_resolver(&self, _source: &str) -> Option<Box<dyn FontResolver>> {
        None
    }

    /// Where this store's fonts load. A store whose fonts can differ from another's under one
    /// family name returns its own; the default is a session shared across the process.
    fn text_session(&self) -> Option<&TextSession> {
        None
    }
}
