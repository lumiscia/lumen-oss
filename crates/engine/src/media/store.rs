use std::fmt::Debug;
use std::sync::{Mutex, MutexGuard};

use crate::audio::AudioResolver;
use crate::error::{LumenError, RenderError};

use super::{FontResolver, ImageResolver, VideoFrameResolver};

/// The fonts one store has loaded and what was shaped and measured with them. Nothing rendered
/// with another store reads them.
#[derive(Debug)]
pub struct TextSession(Mutex<lumen_text::TextSystem>);

impl TextSession {
    pub(crate) fn lock(&self) -> crate::Result<MutexGuard<'_, lumen_text::TextSystem>> {
        self.0.lock().map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: "text system lock was poisoned".to_string(),
            })
        })
    }

    /// Drops the fonts loaded under the name `family`. A store calls this when it stops serving a
    /// family, as text that no longer names it can still fall back to its faces.
    pub fn remove_family(&self, family: &str) -> crate::Result<()> {
        self.lock()?.remove_family(family);
        Ok(())
    }

    /// Returns to only the bundled fonts, freeing the rest. Do not call this while holding a lock
    /// that `font_resolver_id` or `get_font_resolver` take.
    pub fn reset(&self) -> crate::Result<()> {
        *self.lock()? = lumen_text::TextSystem::new();
        Ok(())
    }
}

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

    /// Returns the current font resolver ID without loading font data. Must match the resolver's
    /// ID and change whenever its font data changes. Resolvers of one family with one ID, from any
    /// store, must serve the same data, as they may share it. Without an ID, `get_font_resolver`
    /// is asked each time the family is used, so a store should give one if that is slow.
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
