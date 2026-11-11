use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, PoisonError, RwLock},
};

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::AudioResolver,
    error::MediaError,
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    image::ImageFileResolver,
    media::{
        Asset, AssetKind, AssetResolver, EnvironmentResolver, FontResolver, ImageResolver,
        MediaStore, ModelResolver, NoAssets, TextSession, VideoFrameResolver, asset_id,
        resolve_asset,
    },
};

use super::{
    ErrorCode, GpuSettings, RenderConfig, ServerRenderError,
    fonts::{FontCatalog, ResolvedFont},
};

/// The variable the binaries read [`RenderConfig::hardware_decode`] from, with
/// [`is_enabled_flag`].
pub const HARDWARE_DECODE_ENV: &str = "LUMEN_HARDWARE_DECODE";

/// Serves one render's media from under `root`, with its fonts, and the assets `asset:` sources
/// reference from its asset store.
pub struct LocalMediaStore {
    root: PathBuf,
    assets: Arc<dyn AssetResolver>,
    explain_missing_sources: bool,
    pub(super) video_options: FfmpegResolverOptions,
    pub(super) gpu: GpuSettings,
    audios: RwLock<HashMap<String, Arc<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Arc<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Arc<FfmpegVideoResolver>>>,
    fonts: FontCatalog,
    text: TextSession,
}

impl LocalMediaStore {
    pub fn new(root: PathBuf, config: &RenderConfig) -> Self {
        let video_options = FfmpegResolverOptions::with_hardware_decode(config.hardware_decode);
        let fonts = FontCatalog::new(config.font_dir(), &root);
        tracing::info!(
            media_root = %root.display(),
            prefer_hardware_decode = video_options.prefer_hardware_decode,
            "created local media store"
        );
        Self {
            root,
            assets: Arc::new(NoAssets),
            explain_missing_sources: false,
            video_options,
            gpu: config.gpu.clone(),
            audios: RwLock::new(HashMap::new()),
            images: RwLock::new(HashMap::new()),
            videos: RwLock::new(HashMap::new()),
            fonts,
            text: TextSession::default(),
        }
    }

    /// Makes a source that won't resolve say why. Only for a composition its own user runs, as
    /// telling a missing file from one outside the root lets a composition probe the disk.
    pub fn explaining_missing_sources(mut self) -> Self {
        self.explain_missing_sources = true;
        self
    }

    /// Resolves `asset:` sources from `assets`. Without one, they don't resolve.
    pub fn with_assets(mut self, assets: Arc<dyn AssetResolver>) -> Self {
        self.assets = assets;
        self
    }

    pub fn assets(&self) -> &dyn AssetResolver {
        &*self.assets
    }

    /// The file `source` names: the asset it references, which must be of kind `kind`, or a path
    /// under the root.
    fn stored_source(&self, source: &str, kind: AssetKind) -> Result<String, MediaError> {
        if let Some(id) = asset_id(source) {
            return match resolve_asset(&*self.assets, id, kind)? {
                Asset::Image(path)
                | Asset::Video(path)
                | Asset::Audio(path)
                | Asset::Model(path)
                | Asset::Environment(path) => Ok(path.to_string_lossy().to_string()),
                Asset::Font(_) => unreachable!("resolve_asset returns the kind it is asked for"),
            };
        }
        resolve_local_path_with_root(source, &self.root)
            .map(|path| path.to_string_lossy().to_string())
            .map_err(|error| {
                let reason = if self.explain_missing_sources {
                    format!("{error:#}")
                } else {
                    "no such file in the media root".to_string()
                };
                MediaError::source_not_found(source, reason)
            })
    }

    /// The fonts `font_family` names: the font asset it references, or the family's files.
    fn resolved_font(&self, font_family: &str) -> Result<Option<ResolvedFont>, MediaError> {
        if let Some(id) = asset_id(font_family) {
            return match resolve_asset(&*self.assets, id, AssetKind::Font)? {
                Asset::Font(files) => Ok(Some(ResolvedFont::new(font_family, files))),
                _ => unreachable!("resolve_asset returns the kind it is asked for"),
            };
        }
        Ok(self.fonts.resolve(font_family, || {
            resolve_local_path_with_root(font_family, &self.root).ok()
        }))
    }

    fn image_resolver(&self, source: &str) -> Result<Arc<ImageFileResolver>, MediaError> {
        cached_resolver(&self.images, source, || {
            ImageFileResolver::open(source.to_string())
        })
    }

    fn video_resolver(&self, source: &str) -> Result<Arc<FfmpegVideoResolver>, MediaError> {
        cached_resolver(&self.videos, source, || {
            let resolver = FfmpegVideoResolver::open_with_options(
                source.to_string(),
                self.video_options.clone(),
            )
            .inspect_err(|error| {
                tracing::warn!(source, %error, "failed opening video media resolver");
            })?;
            tracing::info!(
                source,
                width = resolver.metadata().width,
                height = resolver.metadata().height,
                frames = resolver.metadata().frame_count,
                fps = resolver.metadata().fps,
                decode_mode = resolver.decode_mode_label(),
                decode_unavailable_reason = resolver.decode_unavailable_reason(),
                "opened video media resolver"
            );
            Ok(resolver)
        })
    }

    fn audio_resolver(&self, source: &str) -> Result<Arc<FfmpegAudioResolver>, MediaError> {
        cached_resolver(&self.audios, source, || {
            FfmpegAudioResolver::open(source.to_string())
        })
    }
}

/// The resolver cached for `source`, opened with `open` if there is none. Concurrent first
/// requests may each open one, but all of them get the one that was cached first.
fn cached_resolver<T>(
    cache: &RwLock<HashMap<String, Arc<T>>>,
    source: &str,
    open: impl FnOnce() -> Result<T, MediaError>,
) -> Result<Arc<T>, MediaError> {
    if let Some(resolver) = cache
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(source)
    {
        return Ok(Arc::clone(resolver));
    }

    let resolver = Arc::new(open()?);
    let mut cache = cache.write().unwrap_or_else(PoisonError::into_inner);
    Ok(Arc::clone(
        cache.entry(source.to_string()).or_insert(resolver),
    ))
}

impl std::fmt::Debug for LocalMediaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMediaStore")
            .field("root", &self.root)
            .field("video_options", &self.video_options)
            .finish_non_exhaustive()
    }
}

/// Whether a variable's `value` turns its setting on: `1`, `true`, `yes` or `on`, in any case.
pub fn is_enabled_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Result<Box<dyn ImageResolver>, MediaError> {
        let resolved = self.stored_source(source, AssetKind::Image)?;
        Ok(Box::new(self.image_resolver(&resolved)?))
    }

    fn get_video_resolver(
        &self,
        stream_id: &str,
    ) -> Result<Box<dyn VideoFrameResolver>, MediaError> {
        let resolved = self.stored_source(stream_id, AssetKind::Video)?;
        Ok(Box::new(self.video_resolver(&resolved)?))
    }

    fn get_audio_resolver(&self, source_id: &str) -> Result<Box<dyn AudioResolver>, MediaError> {
        let resolved = self.stored_source(source_id, AssetKind::Audio)?;
        Ok(Box::new(self.audio_resolver(&resolved)?))
    }

    fn get_model_resolver(&self, source: &str) -> Result<Box<dyn ModelResolver>, MediaError> {
        let resolved = self.stored_source(source, AssetKind::Model)?;
        Ok(Box::new(SceneFile::open(
            resolved,
            "model",
            lumen_3d::model::MAX_MODEL_BYTES,
        )?))
    }

    fn get_environment_resolver(
        &self,
        source: &str,
    ) -> Result<Box<dyn EnvironmentResolver>, MediaError> {
        let resolved = self.stored_source(source, AssetKind::Environment)?;
        Ok(Box::new(SceneFile::open(
            resolved,
            "environment map",
            lumen_3d::environment::MAX_ENVIRONMENT_BYTES,
        )?))
    }

    fn font_resolver_id(&self, font_family: &str) -> Option<String> {
        let font = self.resolved_font(font_family).ok().flatten()?;
        Some(font.id().to_string())
    }

    fn get_font_resolver(
        &self,
        font_family: &str,
    ) -> Result<Option<Box<dyn FontResolver>>, MediaError> {
        let Some(font) = self.resolved_font(font_family)? else {
            return Ok(None);
        };
        tracing::info!(
            font_family,
            sources = font.files().len(),
            "opened font media resolver"
        );
        Ok(Some(Box::new(font)))
    }

    fn text_session(&self) -> Option<&TextSession> {
        Some(&self.text)
    }
}

/// A model or environment map file, read only when a render hasn't loaded what its ID names.
struct SceneFile {
    path: String,
    /// The path, length and modification time, so that a changed file reloads.
    id: String,
    /// What the file holds, such as "model".
    kind: &'static str,
    limit: usize,
}

impl SceneFile {
    fn open(path: String, kind: &'static str, limit: usize) -> Result<Self, MediaError> {
        let metadata = std::fs::metadata(&path)
            .map_err(|error| MediaError::source_not_found(&path, error.to_string()))?;
        let id = format!("{path}:{}:{:?}", metadata.len(), metadata.modified().ok());
        Ok(Self {
            path,
            id,
            kind,
            limit,
        })
    }

    fn read(&self) -> Result<Vec<u8>, MediaError> {
        read_limited(Path::new(&self.path), self.kind, self.limit).map_err(|details| {
            MediaError::Decode {
                media_source: self.path.clone(),
                details,
            }
        })
    }
}

impl ModelResolver for SceneFile {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<u8>, MediaError> {
        self.read()
    }
}

impl EnvironmentResolver for SceneFile {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<u8>, MediaError> {
        self.read()
    }
}

/// The bytes of the `kind` file at `path`, refusing one larger than `limit` before reading it.
fn read_limited(path: &Path, kind: &str, limit: usize) -> Result<Vec<u8>, String> {
    use std::io::Read;

    let file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() > limit {
        return Err(format!(
            "the {kind} is larger than the {} MiB it may be",
            limit >> 20
        ));
    }
    Ok(bytes)
}

/// `root`, or the current directory, made absolute.
pub(super) fn media_root(root: Option<&Path>) -> Result<PathBuf, ServerRenderError> {
    let root = root.unwrap_or(Path::new("."));
    root.canonicalize().map_err(|error| {
        ServerRenderError::new(
            ErrorCode::InvalidConfig,
            format!("failed to resolve media root `{}`", root.display()),
            false,
        )
        .caused_by(error)
    })
}

fn resolve_local_path_with_root(source: &str, root: &Path) -> anyhow::Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize media root `{}`", root.display()))?;

    if source.starts_with("http://") || source.starts_with("https://") {
        return Err(anyhow!("remote media source `{source}` is not supported"));
    }

    if source.contains("://") && !source.starts_with("file://") {
        return Err(anyhow!("unsupported asset URI scheme for `{source}`"));
    }

    let raw_path = source.strip_prefix("file://").unwrap_or(source);
    let path = Path::new(raw_path);
    if path.as_os_str().is_empty() {
        return Err(anyhow!("asset path must not be empty"));
    }

    if path
        .components()
        .any(|component| matches!(component, Component::ParentDir))
    {
        return Err(anyhow!(
            "parent traversal is not allowed in asset paths: `{source}`"
        ));
    }

    let candidate = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.join(path)
    };

    let candidate = candidate.canonicalize().with_context(|| {
        format!(
            "failed to canonicalize asset path `{}`",
            candidate.display()
        )
    })?;

    if !candidate.starts_with(&root) {
        return Err(anyhow!(
            "asset path escapes allowed media root: `{}`",
            candidate.display()
        ));
    }

    Ok(candidate)
}

#[cfg(test)]
mod tests {
    use lumen_engine::expr::{Expression, ExpressionContext};

    use super::*;

    const ROBOTO: &[u8] = include_bytes!("../../../text/assets/roboto/Roboto-Regular.ttf");

    #[test]
    fn a_source_outside_the_root_fails_like_a_missing_one() {
        let outside = tempfile::NamedTempFile::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = LocalMediaStore::new(
            root.path().canonicalize().unwrap(),
            &RenderConfig::default(),
        );
        let outside = outside.path().display().to_string();
        let failure = |source: &str| match store.get_image_resolver(source) {
            Ok(_) => panic!("{source} should not open"),
            Err(error) => error.to_string(),
        };

        assert_eq!(
            failure("missing.png"),
            "media source `missing.png` not found: no such file in the media root"
        );
        assert_eq!(
            failure(&outside),
            format!("media source `{outside}` not found: no such file in the media root")
        );
    }

    #[test]
    fn a_store_for_its_own_user_says_why_a_source_failed() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("corrupt.png"), b"not a png").unwrap();
        let store = LocalMediaStore::new(root.clone(), &RenderConfig::default())
            .explaining_missing_sources();
        let failure = |source: &str| match store.get_image_resolver(source) {
            Ok(_) => panic!("{source} should not open"),
            Err(error) => error.to_string(),
        };

        assert_eq!(
            failure("missing.png"),
            format!(
                "media source `missing.png` not found: failed to canonicalize asset path `{}`: No such file or directory (os error 2)",
                root.join("missing.png").display()
            )
        );
        assert_eq!(
            failure("corrupt.png"),
            format!(
                "media decoding failed for {}: failed decoding image source: The image format could not be determined",
                root.join("corrupt.png").display()
            )
        );
        assert!(failure("../secret.png").contains("parent traversal"));
        assert!(failure("https://example.com/image.png").contains("remote media source"));
    }

    /// A media root holding `font` as the job's `Roboto`.
    fn job_with_roboto(font: &[u8]) -> (tempfile::TempDir, LocalMediaStore) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("assets/fonts")).unwrap();
        std::fs::write(root.path().join("assets/fonts/Roboto.ttf"), font).unwrap();
        let store = LocalMediaStore::new(
            root.path().canonicalize().unwrap(),
            &RenderConfig::default(),
        );
        (root, store)
    }

    fn roboto_width(store: &LocalMediaStore) -> f64 {
        Expression::parse("text_width(\"WWW\", 32, 0, \"Roboto\")")
            .unwrap()
            .evaluate(&ExpressionContext {
                media: Some(store),
                ..Default::default()
            })
            .unwrap()
            .as_f64()
            .unwrap()
    }

    #[test]
    fn jobs_measure_one_family_with_their_own_font_files() {
        // A larger em square draws the same outlines at half the size.
        let mut half_size = ROBOTO.to_vec();
        let tables = u16::from_be_bytes([half_size[4], half_size[5]]) as usize;
        let head = half_size[12..12 + tables * 16]
            .chunks(16)
            .find(|table| &table[..4] == b"head")
            .unwrap();
        let units = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
        let em = u16::from_be_bytes([half_size[units], half_size[units + 1]]);
        half_size[units..units + 2].copy_from_slice(&(em * 2).to_be_bytes());

        let (_full_root, full) = job_with_roboto(ROBOTO);
        let (_half_root, half) = job_with_roboto(&half_size);
        let full_width = roboto_width(&full);
        assert!((roboto_width(&half) * 2.0 - full_width).abs() < 1.0);

        // Jobs running together keep to their own files too.
        std::thread::scope(|scope| {
            scope.spawn(|| {
                for _ in 0..100 {
                    assert_eq!(roboto_width(&full), full_width);
                }
            });
            scope.spawn(|| {
                for _ in 0..100 {
                    assert!((roboto_width(&half) * 2.0 - full_width).abs() < 1.0);
                }
            });
        });
    }

    #[test]
    fn a_store_takes_its_font_directory_and_decode_switch_from_its_config() {
        const INTER: &[u8] = include_bytes!("../../../text/assets/inter/InterVariable-Latin.ttf");
        let fonts = tempfile::tempdir().unwrap();
        std::fs::write(fonts.path().join("inter.ttf"), INTER).unwrap();
        let root = tempfile::tempdir().unwrap();
        let config = RenderConfig {
            font_dir: Some(fonts.path().to_path_buf()),
            hardware_decode: true,
            gpu: GpuSettings {
                force_fallback_adapter: true,
                ..GpuSettings::default()
            },
            ..RenderConfig::default()
        };

        let store = LocalMediaStore::new(root.path().canonicalize().unwrap(), &config);

        let id = store.font_resolver_id("Inter").expect("Inter");
        assert!(id.contains(&fonts.path().display().to_string()), "{id}");
        assert!(store.video_options.prefer_hardware_decode);
        assert!(store.gpu.force_fallback_adapter);
        let default = LocalMediaStore::new(root.path().to_path_buf(), &RenderConfig::default());
        assert!(!default.video_options.prefer_hardware_decode);
        assert!(!default.gpu.force_fallback_adapter);
    }

    #[test]
    fn enabled_flags_are_the_usual_yes_words() {
        for value in ["1", "true", "YES", " on "] {
            assert!(is_enabled_flag(value), "{value}");
        }
        for value in ["", "0", "off", "enabled"] {
            assert!(!is_enabled_flag(value), "{value}");
        }
    }

    #[test]
    fn a_changed_font_file_changes_the_resolver_id() {
        let (root, store) = job_with_roboto(ROBOTO);
        let before = store.font_resolver_id("Roboto").unwrap();
        let changed = [ROBOTO, &[0]].concat();
        std::fs::write(root.path().join("assets/fonts/Roboto.ttf"), changed).unwrap();

        let next_job = LocalMediaStore::new(
            root.path().canonicalize().unwrap(),
            &RenderConfig::default(),
        );
        assert_ne!(next_job.font_resolver_id("Roboto").unwrap(), before);
    }

    #[test]
    fn a_font_staged_under_its_alias_serves_the_family_of_that_name() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("brand"), ROBOTO).unwrap();
        let store = LocalMediaStore::new(
            root.path().canonicalize().unwrap(),
            &RenderConfig::default(),
        );

        let resolver = store.get_font_resolver("brand").unwrap().unwrap();
        assert_eq!(resolver.data().unwrap(), vec![ROBOTO.to_vec()]);
        assert!(store.get_font_resolver("other").unwrap().is_none());
    }

    #[test]
    fn concurrent_first_requests_share_one_resolver() {
        let cache = RwLock::new(HashMap::new());
        let opened = std::sync::Barrier::new(2);
        let open = || {
            opened.wait();
            Ok(())
        };

        let (first, second) = std::thread::scope(|scope| {
            let first = scope.spawn(|| cached_resolver(&cache, "a.png", open).unwrap());
            let second = scope.spawn(|| cached_resolver(&cache, "a.png", open).unwrap());
            (first.join().unwrap(), second.join().unwrap())
        });

        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(
            &first,
            &cached_resolver(&cache, "a.png", || panic!("opened a cached source")).unwrap()
        ));
    }

    #[derive(Debug)]
    struct Assets(HashMap<&'static str, Asset>);

    impl AssetResolver for Assets {
        fn resolve(&self, id: &str) -> Option<Asset> {
            self.0.get(id).cloned()
        }
    }

    #[test]
    fn asset_sources_resolve_through_the_asset_store_wherever_it_keeps_them() {
        let assets = tempfile::tempdir().unwrap();
        let image = assets.path().join("logo");
        std::fs::write(&image, b"not a png").unwrap();
        let font = assets.path().join("brand");
        std::fs::write(&font, ROBOTO).unwrap();
        let fox = assets.path().join("fox");
        std::fs::write(&fox, b"glTF").unwrap();
        let sky = assets.path().join("sky");
        std::fs::write(&sky, b"#?RADIANCE").unwrap();
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let store = LocalMediaStore::new(root.clone(), &RenderConfig::default()).with_assets(
            Arc::new(Assets(HashMap::from([
                ("logo", Asset::Image(image.clone())),
                ("brand", Asset::Font(vec![font])),
                ("fox", Asset::Model(fox.clone())),
                ("sky", Asset::Environment(sky)),
            ]))),
        );
        let image_error = |source: &str| match store.get_image_resolver(source) {
            Ok(_) => panic!("{source} should not open"),
            Err(error) => error,
        };

        let decode = image_error("asset:logo");
        assert_eq!(decode.code(), "media.decode");
        assert!(decode.to_string().contains(&image.display().to_string()));
        assert_eq!(image_error("asset:other").code(), "asset.not_found");
        assert_eq!(image_error("asset:brand").code(), "asset.wrong_kind");
        let model = |source: &str| {
            store
                .get_model_resolver(source)
                .map(|model| model.id().to_string())
        };
        let id = model("asset:fox").unwrap();
        std::fs::write(&fox, b"glTF, changed").unwrap();
        assert_ne!(
            model("asset:fox").unwrap(),
            id,
            "a changed file has a new ID"
        );
        assert_eq!(model("asset:logo").unwrap_err().code(), "asset.wrong_kind");
        let sky = store.get_environment_resolver("asset:sky").unwrap();
        assert_eq!(sky.data().unwrap(), b"#?RADIANCE");
        let wrong_sky = store.get_environment_resolver("asset:fox").err().unwrap();
        assert_eq!(wrong_sky.code(), "asset.wrong_kind");
        let font = store.get_font_resolver("asset:brand").unwrap().unwrap();
        assert_eq!(font.data().unwrap(), vec![ROBOTO.to_vec()]);
        assert_eq!(
            store.font_resolver_id("asset:brand").as_deref(),
            Some(font.id())
        );
        let wrong_font = store.get_font_resolver("asset:logo").err().unwrap();
        assert_eq!(wrong_font.code(), "asset.wrong_kind");

        let without_assets = LocalMediaStore::new(root, &RenderConfig::default());
        let missing = without_assets
            .get_font_resolver("asset:brand")
            .err()
            .unwrap();
        assert_eq!(missing.code(), "asset.not_found");
    }
}
