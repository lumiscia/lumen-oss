//! References to files in a host's asset store, written `asset:<id>` in a source field.

use std::fmt;
use std::path::PathBuf;

use serde::Serialize;

use crate::error::{MediaError, MediaFailure};

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
    /// A binary glTF (`.glb`) 3D model.
    Model,
    /// An equirectangular Radiance `.hdr` or OpenEXR `.exr` environment map.
    Environment,
}

impl AssetKind {
    /// The kind's name in JSON and in `#[media(...)]`.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Image => "image",
            Self::Video => "video",
            Self::Audio => "audio",
            Self::Font => "font",
            Self::Model => "model",
            Self::Environment => "environment",
        }
    }
}

impl fmt::Display for AssetKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Image => "an image",
            Self::Video => "a video",
            Self::Audio => "audio",
            Self::Font => "a font",
            Self::Model => "a model",
            Self::Environment => "an environment map",
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
    Model(PathBuf),
    Environment(PathBuf),
}

impl Asset {
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

/// A host's asset store, as rendering and checking see it. An asset's content never changes
/// under its id, so stores name assets by their id instead of looking at their files; a host
/// that replaces an asset's content gives it a new id.
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
    resolve_asset_of(assets, id, &[expected])
}

/// The asset `id` names in `assets`, which must be of one of `kinds`, of which there must be at
/// least one. An asset of another kind fails as `media.wrong_kind`.
pub fn resolve_asset_of(
    assets: &dyn AssetResolver,
    id: &str,
    kinds: &[AssetKind],
) -> Result<Asset, MediaError> {
    let source = format!("{ASSET_SCHEME}{id}");
    let asset = assets.resolve(id).ok_or_else(|| {
        MediaFailure::not_found(&source, "this host has no asset with that id").wanting(kinds)
    })?;
    if kinds.contains(&asset.kind()) {
        Ok(asset)
    } else {
        Err(MediaFailure::wrong_kind(&source, kinds, Some(asset.kind()), "").into())
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
        assert_eq!(wrong.code(), "media.wrong_kind");
        assert_eq!(wrong.to_string(), "`asset:logo` is an image, not a font");
        let missing = resolve_asset(&NoAssets, "logo", AssetKind::Image).unwrap_err();
        assert_eq!(missing.code(), "media.not_found");
        assert_eq!(
            missing.to_string(),
            "image `asset:logo` not found: this host has no asset with that id"
        );
    }
}
