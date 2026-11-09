//! References to files in a host's asset store, written `asset:<id>` in a source field.

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use crate::error::MediaError;

/// What starts a source that names an asset rather than a path or `media` alias.
pub const ASSET_SCHEME: &str = "asset:";

/// What an asset holds, which its store knows from checking it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum AssetKind {
    Image,
    Video,
    Audio,
    Font,
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Image => "an image",
            Self::Video => "a video",
            Self::Audio => "audio",
            Self::Font => "a font",
        })
    }
}

/// An asset and where its content is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Asset {
    Image(PathBuf),
    Video(PathBuf),
    Audio(PathBuf),
    /// One family: the files of its faces, among which weight and style choose.
    Font(Vec<PathBuf>),
}

impl Asset {
    pub fn kind(&self) -> AssetKind {
        match self {
            Self::Image(_) => AssetKind::Image,
            Self::Video(_) => AssetKind::Video,
            Self::Audio(_) => AssetKind::Audio,
            Self::Font(_) => AssetKind::Font,
        }
    }
}

/// A host's asset store, as rendering and checking see it.
pub trait AssetResolver: Send + Sync + fmt::Debug {
    /// The asset `id` names, or `None` if this host doesn't have it.
    fn resolve(&self, id: &str) -> Option<Asset>;
}

/// A host without an asset store, where no asset reference resolves.
#[derive(Debug, Clone, Copy, Default)]
pub struct NoAssets;

impl AssetResolver for NoAssets {
    fn resolve(&self, _id: &str) -> Option<Asset> {
        None
    }
}

/// The id `source` references, if it is an asset reference.
pub fn asset_id(source: &str) -> Option<&str> {
    source.strip_prefix(ASSET_SCHEME)
}

/// The asset `id` names in `assets`, which must be of kind `expected`.
pub fn resolve_asset(
    assets: &dyn AssetResolver,
    id: &str,
    expected: AssetKind,
) -> Result<Asset, MediaError> {
    let asset = assets
        .resolve(id)
        .ok_or_else(|| MediaError::AssetNotFound { id: id.to_string() })?;
    if asset.kind() == expected {
        Ok(asset)
    } else {
        Err(MediaError::AssetWrongKind {
            id: id.to_string(),
            expected,
            actual: asset.kind(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug)]
    struct OneImage;

    impl AssetResolver for OneImage {
        fn resolve(&self, id: &str) -> Option<Asset> {
            (id == "logo").then(|| Asset::Image(PathBuf::from("logo.png")))
        }
    }

    #[test]
    fn a_reference_resolves_only_to_an_asset_of_its_kind() {
        assert_eq!(asset_id("asset:logo"), Some("logo"));
        assert_eq!(asset_id("logo.png"), None);
        assert_eq!(
            resolve_asset(&OneImage, "logo", AssetKind::Image).unwrap(),
            Asset::Image(PathBuf::from("logo.png"))
        );

        let wrong = resolve_asset(&OneImage, "logo", AssetKind::Font).unwrap_err();
        assert_eq!(wrong.code(), "asset.wrong_kind");
        assert_eq!(wrong.to_string(), "asset `logo` is an image, not a font");
        let missing = resolve_asset(&NoAssets, "logo", AssetKind::Image).unwrap_err();
        assert_eq!(missing.code(), "asset.not_found");
    }
}
