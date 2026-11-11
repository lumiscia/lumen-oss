use std::fmt::Debug;
use std::sync::{Mutex, MutexGuard};

use crate::audio::AudioResolver;
use crate::error::{LumenError, MediaError, MediaFailure, RenderError};

use super::{
    AssetKind, EnvironmentResolver, FontResolver, ImageResolver, MediaIdentity, ModelResolver,
    VideoFrameResolver,
};

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
    /// that [`MediaStore::resolve`] takes.
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

/// A resolver of one kind of media.
pub enum MediaResolver {
    Image(Box<dyn ImageResolver>),
    Video(Box<dyn VideoFrameResolver>),
    Audio(Box<dyn AudioResolver>),
    /// One family: the files of its faces, among which weight and style choose.
    Font(Box<dyn FontResolver>),
    Model(Box<dyn ModelResolver>),
    Environment(Box<dyn EnvironmentResolver>),
}

impl MediaResolver {
    pub fn kind(&self) -> AssetKind {
        match self {
            Self::Image(_) => AssetKind::Image,
            Self::Video(_) => AssetKind::Video,
            Self::Audio(_) => AssetKind::Audio,
            Self::Font(_) => AssetKind::Font,
            Self::Model(_) => AssetKind::Model,
            Self::Environment(_) => AssetKind::Environment,
        }
    }
}

/// What a store serves for a source: which bytes, and a resolver that reads them.
pub struct ResolvedMedia {
    pub identity: MediaIdentity,
    pub resolver: MediaResolver,
}

impl ResolvedMedia {
    pub fn new(identity: MediaIdentity, resolver: MediaResolver) -> Self {
        Self { identity, resolver }
    }
}

/// Where a render's media and fonts come from. A source or family may be an asset reference,
/// `asset:<id>`, which a store with an asset store resolves with [`super::resolve_asset`] and any
/// other store fails as `media.not_found`.
pub trait MediaStore: Send + Sync + Debug {
    /// `source` as media of `kind`, which is what the use reads it as. Video decode settings are
    /// the store's own. The engine asks again each frame a source is used, to learn whether its
    /// identity changed, so resolving should be cheap once a source has been opened, and font,
    /// model and environment resolvers should read their bytes only when asked.
    ///
    /// A source this store doesn't have fails with `media.not_found`, which for a font family
    /// that isn't an asset reference makes its text fall back to other fonts. One it has but
    /// can't use as `kind` fails with why: `media.wrong_kind`, `media.unsupported`,
    /// `media.too_large` or `media.decode`.
    fn resolve(&self, source: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError>;

    /// Where this store's fonts load. A store whose fonts can differ from another's under one
    /// family name returns its own; the default is a session shared across the process.
    fn text_session(&self) -> Option<&TextSession> {
        None
    }
}

/// One of the resolver traits, which [`resolve_as`] asks a store for.
pub trait ResolverKind {
    const KIND: AssetKind;

    /// The resolver, if it is of this kind.
    fn take(resolver: MediaResolver) -> Option<Box<Self>>;
}

macro_rules! resolver_kinds {
    ($($resolver:ident => $variant:ident),* $(,)?) => {$(
        impl ResolverKind for dyn $resolver {
            const KIND: AssetKind = AssetKind::$variant;

            fn take(resolver: MediaResolver) -> Option<Box<Self>> {
                match resolver {
                    MediaResolver::$variant(resolver) => Some(resolver),
                    _ => None,
                }
            }
        }
    )*};
}

resolver_kinds!(
    ImageResolver => Image,
    VideoFrameResolver => Video,
    AudioResolver => Audio,
    FontResolver => Font,
    ModelResolver => Model,
    EnvironmentResolver => Environment,
);

/// `source` resolved by `store` as a `K`, with its identity. A store that serves another kind
/// fails as `media.wrong_kind`.
pub fn resolve_as<K: ResolverKind + ?Sized>(
    store: &dyn MediaStore,
    source: &str,
) -> Result<(MediaIdentity, Box<K>), MediaError> {
    let ResolvedMedia { identity, resolver } = store.resolve(source, K::KIND)?;
    let found = resolver.kind();
    K::take(resolver)
        .map(|resolver| (identity, resolver))
        .ok_or_else(|| MediaFailure::wrong_kind(source, &[K::KIND], Some(found), "").into())
}
