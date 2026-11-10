use std::fmt::Debug;
use std::sync::{Mutex, MutexGuard};

use crate::audio::AudioResolver;
use crate::error::{LumenError, MediaError, RenderError};

use super::{EnvironmentResolver, FontResolver, ImageResolver, ModelResolver, VideoFrameResolver};

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

/// Where a render's media and fonts come from. A source or family may be an asset reference,
/// `asset:<id>`, which a store with an asset store resolves with [`super::resolve_asset`] and any
/// other store fails as [`MediaError::AssetNotFound`].
pub trait MediaStore: Send + Sync + Debug {
    /// The resolver for `source`. A source this store doesn't have fails with
    /// `MediaError::SourceNotFound`, and one it has but can't open fails with why.
    fn get_image_resolver(&self, source: &str) -> Result<Box<dyn ImageResolver>, MediaError> {
        Err(MediaError::source_not_found(
            source,
            "this store has no images",
        ))
    }

    fn get_video_resolver(
        &self,
        stream_id: &str,
    ) -> Result<Box<dyn VideoFrameResolver>, MediaError> {
        Err(MediaError::source_not_found(
            stream_id,
            "this store has no videos",
        ))
    }

    fn get_audio_resolver(&self, source: &str) -> Result<Box<dyn AudioResolver>, MediaError> {
        Err(MediaError::source_not_found(
            source,
            "this store has no audio",
        ))
    }

    /// The resolver for the 3D model file `source`.
    fn get_model_resolver(&self, source: &str) -> Result<Box<dyn ModelResolver>, MediaError> {
        Err(MediaError::source_not_found(
            source,
            "this store has no models",
        ))
    }

    /// The resolver for the environment map file `source`.
    fn get_environment_resolver(
        &self,
        source: &str,
    ) -> Result<Box<dyn EnvironmentResolver>, MediaError> {
        Err(MediaError::source_not_found(
            source,
            "this store has no environment maps",
        ))
    }

    /// Returns the current font resolver ID without loading font data. Must match the resolver's
    /// ID and change whenever its font data changes. Resolvers of one family with one ID, from any
    /// store, must serve the same data, as they may share it. Without an ID, `get_font_resolver`
    /// is asked each time the family is used, so a store should give one if that is slow.
    fn font_resolver_id(&self, _source: &str) -> Option<String> {
        None
    }

    /// The resolver for the fonts of `family`, or `None` if this store has none, so its text falls
    /// back to another family. A family this store can't serve as asked, such as an asset
    /// reference it doesn't know, fails with why.
    fn get_font_resolver(
        &self,
        _family: &str,
    ) -> Result<Option<Box<dyn FontResolver>>, MediaError> {
        Ok(None)
    }

    /// Where this store's fonts load. A store whose fonts can differ from another's under one
    /// family name returns its own; the default is a session shared across the process.
    fn text_session(&self) -> Option<&TextSession> {
        None
    }
}
