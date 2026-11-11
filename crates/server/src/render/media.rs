use std::{
    collections::HashMap,
    path::{Component, Path, PathBuf},
    sync::{Arc, PoisonError, RwLock},
};

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::AudioResolver,
    error::{MediaError, MediaFailure},
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    image::ImageFileResolver,
    media::{
        Asset, AssetKind, AssetResolver, EnvironmentResolver, ImageResolver, LutResolver,
        MediaIdentity, MediaResolver, MediaStore, ModelResolver, NoAssets, ResolvedMedia,
        TextSession, VideoFrameResolver, asset_id, resolve_asset,
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
    audios: RwLock<HashMap<String, Opened<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Opened<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Opened<FfmpegVideoResolver>>>,
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

    /// The file `source` names: the asset it references, which must be of kind `kind` and is
    /// named by its id, or a path under the root, which is named by the file's metadata when it
    /// is opened.
    fn stored_source(
        &self,
        source: &str,
        kind: AssetKind,
    ) -> Result<(String, Option<MediaIdentity>), MediaError> {
        if let Some(id) = asset_id(source) {
            return match resolve_asset(&*self.assets, id, kind)? {
                Asset::Image(path)
                | Asset::Video(path)
                | Asset::Audio(path)
                | Asset::Model(path)
                | Asset::Environment(path)
                | Asset::Lut(path) => {
                    let identity = asset_identity(id, [&path]);
                    Ok((path.to_string_lossy().to_string(), Some(identity)))
                }
                Asset::Font(_) => unreachable!("resolve_asset returns the kind it is asked for"),
            };
        }
        resolve_local_path_with_root(source, &self.root)
            .map(|path| (path.to_string_lossy().to_string(), None))
            .map_err(|error| {
                local_path_failure(source, kind, &error, self.explain_missing_sources).into()
            })
    }

    /// The fonts `font_family` names: the font asset it references, or the family's files.
    fn resolved_font(&self, font_family: &str) -> Result<Option<ResolvedFont>, MediaError> {
        if let Some(id) = asset_id(font_family) {
            return match resolve_asset(&*self.assets, id, AssetKind::Font)? {
                Asset::Font(files) => {
                    let identity = asset_identity(id, &files);
                    Ok(Some(ResolvedFont::new(font_family, identity, files)))
                }
                _ => unreachable!("resolve_asset returns the kind it is asked for"),
            };
        }
        Ok(self.fonts.resolve(font_family, || {
            resolve_local_path_with_root(font_family, &self.root).ok()
        }))
    }

    fn image_resolver(
        &self,
        source: &str,
        identity: Option<MediaIdentity>,
    ) -> Result<Opened<ImageFileResolver>, MediaError> {
        cached_resolver(&self.images, source, identity, || {
            ImageFileResolver::open(source.to_string())
        })
    }

    fn video_resolver(
        &self,
        source: &str,
        identity: Option<MediaIdentity>,
    ) -> Result<Opened<FfmpegVideoResolver>, MediaError> {
        cached_resolver(&self.videos, source, identity, || {
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

    fn audio_resolver(
        &self,
        source: &str,
        identity: Option<MediaIdentity>,
    ) -> Result<Opened<FfmpegAudioResolver>, MediaError> {
        cached_resolver(&self.audios, source, identity, || {
            FfmpegAudioResolver::open(source.to_string())
        })
    }

    fn resolve_font(&self, font_family: &str) -> Result<ResolvedMedia, MediaError> {
        let font = self.resolved_font(font_family)?.ok_or_else(|| {
            MediaFailure::not_found(font_family, "no font has this family name")
                .wanting(&[AssetKind::Font])
        })?;
        Ok(ResolvedMedia::new(
            font.identity().clone(),
            MediaResolver::Font(Box::new(font)),
        ))
    }
}

/// A file opened once per store, and its identity when it was opened.
type Opened<T> = (MediaIdentity, Arc<T>);

/// A resolver of the file `resolved`, whose failures name `written`, the source as the composition
/// wrote it, instead: a path in the media root or the asset store means nothing to a client, and
/// is not theirs to learn.
struct Written<T> {
    written: String,
    resolved: String,
    inner: Arc<T>,
}

impl<T> Written<T> {
    fn new(written: &str, resolved: &str, inner: Arc<T>) -> Box<Self> {
        Box::new(Self {
            written: written.to_string(),
            resolved: resolved.to_string(),
            inner,
        })
    }

    fn named(&self, error: MediaError) -> MediaError {
        error.written_as(&self.resolved, &self.written)
    }
}

impl<T: ImageResolver> ImageResolver for Written<T> {
    fn metadata(&self) -> lumen_engine::media::ImageMetadata {
        self.inner.metadata()
    }

    fn frame(&self) -> Result<lumen_engine::media::MediaFrame, MediaError> {
        self.inner.frame().map_err(|error| self.named(error))
    }
}

impl<T: VideoFrameResolver> VideoFrameResolver for Written<T> {
    fn metadata(&self) -> lumen_engine::media::VideoMetadata {
        self.inner.metadata()
    }

    fn enqueue_frame(&self, frame: u32) -> Result<(), MediaError> {
        self.inner
            .enqueue_frame(frame)
            .map_err(|error| self.named(error))
    }

    fn frame(&self, frame: u32) -> Result<lumen_engine::media::MediaFrame, MediaError> {
        self.inner.frame(frame).map_err(|error| self.named(error))
    }

    fn retain_frames(&self, frames: &[u32]) {
        self.inner.retain_frames(frames);
    }
}

impl<T: AudioResolver> AudioResolver for Written<T> {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<lumen_engine::audio::AudioBuffer>, MediaError> {
        self.inner
            .resolve_range(start_sample, frames)
            .map_err(|error| self.named(error))
    }
}

/// An asset, by its id and where its store keeps it, as an asset's content never changes under
/// its id and two stores in one process may use one id.
fn asset_identity<P: AsRef<Path>>(id: &str, files: impl IntoIterator<Item = P>) -> MediaIdentity {
    let mut key = format!("asset:{id}");
    for file in files {
        key.push_str(&format!(":{:?}", file.as_ref()));
    }
    MediaIdentity::of_key(&key)
}

/// The resolver cached for the file `source`, opened with `open` if there is none, and named by
/// `identity`, or else by what the file was just before. Concurrent first requests may each open
/// one, but all of them get the one that was cached first.
fn cached_resolver<T>(
    cache: &RwLock<HashMap<String, Opened<T>>>,
    source: &str,
    identity: Option<MediaIdentity>,
    open: impl FnOnce() -> Result<T, MediaError>,
) -> Result<Opened<T>, MediaError> {
    if let Some((identity, resolver)) = cache
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .get(source)
    {
        return Ok((identity.clone(), Arc::clone(resolver)));
    }

    let identity = identity.unwrap_or_else(|| MediaIdentity::of_file(source));
    let resolver = Arc::new(open()?);
    let mut cache = cache.write().unwrap_or_else(PoisonError::into_inner);
    let (identity, resolver) = cache
        .entry(source.to_string())
        .or_insert((identity, resolver));
    Ok((identity.clone(), Arc::clone(resolver)))
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
    fn resolve(&self, source: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
        if kind == AssetKind::Font {
            return self.resolve_font(source);
        }
        let (resolved, identity) = self.stored_source(source, kind)?;
        let named = |error: MediaError| error.written_as(&resolved, source);
        let (identity, resolver) = match kind {
            AssetKind::Image => {
                let (identity, image) = self.image_resolver(&resolved, identity).map_err(named)?;
                let image = Written::new(source, &resolved, image);
                (identity, MediaResolver::Image(image))
            }
            AssetKind::Video => {
                let (identity, video) = self.video_resolver(&resolved, identity).map_err(named)?;
                let video = Written::new(source, &resolved, video);
                (identity, MediaResolver::Video(video))
            }
            AssetKind::Audio => {
                let (identity, audio) = self.audio_resolver(&resolved, identity).map_err(named)?;
                let audio = Written::new(source, &resolved, audio);
                (identity, MediaResolver::Audio(audio))
            }
            AssetKind::Model => {
                let limit = lumen_3d::model::MAX_MODEL_BYTES;
                let file = SceneFile::open(source, resolved, identity, "model", limit)?;
                (file.identity.clone(), MediaResolver::Model(Box::new(file)))
            }
            AssetKind::Environment => {
                let limit = lumen_3d::environment::MAX_ENVIRONMENT_BYTES;
                let file = SceneFile::open(source, resolved, identity, "environment map", limit)?;
                let identity = file.identity.clone();
                (identity, MediaResolver::Environment(Box::new(file)))
            }
            AssetKind::Lut => {
                let limit = lumen_engine::media::MAX_LUT_BYTES;
                let file = SceneFile::open(source, resolved, identity, "LUT", limit)?;
                let identity = file.identity.clone();
                (identity, MediaResolver::Lut(Box::new(file)))
            }
            AssetKind::Font => unreachable!("fonts resolve above"),
        };
        Ok(ResolvedMedia::new(identity, resolver))
    }

    fn text_session(&self) -> Option<&TextSession> {
        Some(&self.text)
    }
}

/// A model or environment map file, read only when a render hasn't loaded its identity.
struct SceneFile {
    /// The source the composition wrote, which failures name.
    source: String,
    path: String,
    identity: MediaIdentity,
    /// What the file holds, such as "model".
    kind: &'static str,
    limit: usize,
}

impl SceneFile {
    fn open(
        source: &str,
        path: String,
        identity: Option<MediaIdentity>,
        kind: &'static str,
        limit: usize,
    ) -> Result<Self, MediaError> {
        let metadata = std::fs::metadata(&path)
            .map_err(|error| MediaFailure::from_io(source, kind, &error))?;
        if metadata.len() > limit as u64 {
            return Err(too_large(source, kind, limit, Some(metadata.len())).into());
        }
        Ok(Self {
            source: source.to_string(),
            identity: identity.unwrap_or_else(|| MediaIdentity::of_file(&path)),
            path,
            kind,
            limit,
        })
    }

    fn read(&self) -> Result<Vec<u8>, MediaError> {
        use std::io::Read;

        let file = std::fs::File::open(&self.path)
            .map_err(|error| MediaFailure::from_io(&self.source, self.kind, &error))?;
        let mut bytes = Vec::new();
        file.take(self.limit as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(|error| MediaFailure::from_io(&self.source, self.kind, &error))?;
        if bytes.len() > self.limit {
            return Err(too_large(&self.source, self.kind, self.limit, None).into());
        }
        Ok(bytes)
    }
}

/// `kind` is larger than the `limit` bytes a file of it may be.
fn too_large(source: &str, kind: &str, limit: usize, actual: Option<u64>) -> MediaFailure {
    MediaFailure::too_large(
        source,
        format!(
            "the {kind} is larger than the {} MiB it may be",
            limit >> 20
        ),
        limit as u64,
        actual,
    )
}

impl ModelResolver for SceneFile {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        self.read()
    }
}

impl EnvironmentResolver for SceneFile {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        self.read()
    }
}

impl LutResolver for SceneFile {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        self.read()
    }
}

/// Why `source`, a path under the media root, doesn't resolve: `media.not_found`, unless the system
/// ran short of descriptors or memory while looking, which can pass and says so.
fn local_path_failure(
    source: &str,
    kind: AssetKind,
    error: &anyhow::Error,
    explain: bool,
) -> MediaFailure {
    let shortage = error
        .chain()
        .find_map(|cause| cause.downcast_ref::<std::io::Error>())
        .map(|io| MediaFailure::from_io(source, "the file", io))
        .filter(|failure| failure.transient);
    shortage
        .unwrap_or_else(|| {
            let reason = if explain {
                format!("{error:#}")
            } else {
                "no such file in the media root".to_string()
            };
            MediaFailure::not_found(source, reason)
        })
        .wanting(&[kind])
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
    use lumen_engine::{
        audio::AudioResolver,
        error::MediaReason,
        expr::{Expression, ExpressionContext},
        media::{FontResolver, ResolverKind, resolve_as},
    };

    use super::*;

    const ROBOTO: &[u8] = include_bytes!("../../../text/assets/roboto/Roboto-Regular.ttf");

    fn resolver<K: ResolverKind + ?Sized>(
        store: &LocalMediaStore,
        source: &str,
    ) -> Result<Box<K>, MediaError> {
        resolve_as::<K>(store, source).map(|(_, resolver)| resolver)
    }

    fn identity(store: &LocalMediaStore, source: &str, kind: AssetKind) -> MediaIdentity {
        match store.resolve(source, kind) {
            Ok(resolved) => resolved.identity,
            Err(error) => panic!("{source}: {error}"),
        }
    }

    /// The cases `@lumiscia/lumen-preview` checks too, so the server and the browser preview accept
    /// the same sources for the same composition.
    #[test]
    fn sources_resolve_as_the_browser_preview_resolves_them() {
        #[derive(serde::Deserialize)]
        struct Case {
            source: String,
            provided: bool,
            code: Option<String>,
        }
        #[derive(serde::Deserialize)]
        struct Rules {
            cases: Vec<Case>,
        }
        #[derive(Debug)]
        struct OneModel(PathBuf);
        impl AssetResolver for OneModel {
            fn resolve(&self, id: &str) -> Option<Asset> {
                (id == "logo").then(|| Asset::Model(self.0.clone()))
            }
        }

        let rules: Rules =
            serde_json::from_str(include_str!("../../tests/fixtures/source-rules.json")).unwrap();
        for case in rules.cases {
            let dir = tempfile::tempdir().unwrap();
            let root = dir.path().join("root");
            std::fs::create_dir_all(root.join("clips")).unwrap();
            let mut store = LocalMediaStore::new(root.clone(), &RenderConfig::default());
            if case.provided {
                if asset_id(&case.source).is_some() {
                    let file = dir.path().join("stored.glb");
                    std::fs::write(&file, b"glTF").unwrap();
                    store = store.with_assets(Arc::new(OneModel(file)));
                } else if !case.source.contains("://") || case.source.starts_with("file://") {
                    let path = case.source.trim_start_matches("file://");
                    std::fs::write(root.join(path), b"glTF").unwrap();
                }
            }
            let code = store
                .resolve(&case.source, AssetKind::Model)
                .err()
                .map(|error| error.failure().expect("a media failure").code().to_string());
            assert_eq!(code, case.code, "{}", case.source);
        }
    }

    #[cfg(unix)]
    #[test]
    fn running_out_of_descriptors_while_resolving_a_path_is_not_a_missing_file() {
        let shortage = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::EMFILE))
            .context("failed to canonicalize asset path `/media/a.png`");
        let failure = local_path_failure("a.png", AssetKind::Image, &shortage, false);
        assert_eq!(failure.code(), "media.decode");
        assert!(failure.transient);
        let missing = anyhow::Error::new(std::io::Error::from_raw_os_error(libc::ENOENT))
            .context("failed to canonicalize asset path");
        let failure = local_path_failure("a.png", AssetKind::Image, &missing, false);
        assert_eq!(failure.code(), "media.not_found");
        assert!(!failure.transient);
    }

    #[test]
    fn a_source_outside_the_root_fails_like_a_missing_one() {
        let outside = tempfile::NamedTempFile::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = LocalMediaStore::new(
            root.path().canonicalize().unwrap(),
            &RenderConfig::default(),
        );
        let outside = outside.path().display().to_string();
        let failure = |source: &str| match resolver::<dyn ImageResolver>(&store, source) {
            Ok(_) => panic!("{source} should not open"),
            Err(error) => error.to_string(),
        };

        assert_eq!(
            failure("missing.png"),
            "image `missing.png` not found: no such file in the media root"
        );
        assert_eq!(
            failure(&outside),
            format!("image `{outside}` not found: no such file in the media root")
        );
    }

    #[test]
    fn a_store_for_its_own_user_says_why_a_source_failed() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        std::fs::write(root.join("corrupt.png"), b"not a png").unwrap();
        let store = LocalMediaStore::new(root.clone(), &RenderConfig::default())
            .explaining_missing_sources();
        let failure = |source: &str| match resolver::<dyn ImageResolver>(&store, source) {
            Ok(_) => panic!("{source} should not open"),
            Err(error) => error.to_string(),
        };

        assert_eq!(
            failure("missing.png"),
            format!(
                "image `missing.png` not found: failed to canonicalize asset path `{}`: No such file or directory (os error 2)",
                root.join("missing.png").display()
            )
        );
        assert_eq!(
            failure("corrupt.png"),
            "image `corrupt.png` failed to decode: the file is not in a recognised image format"
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

        let id = identity(&store, "Inter", AssetKind::Font);
        let id = id.as_str();
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
    fn a_changed_font_file_changes_its_identity() {
        let (root, store) = job_with_roboto(ROBOTO);
        let before = identity(&store, "Roboto", AssetKind::Font);
        let changed = [ROBOTO, &[0]].concat();
        std::fs::write(root.path().join("assets/fonts/Roboto.ttf"), changed).unwrap();

        let next_job = LocalMediaStore::new(
            root.path().canonicalize().unwrap(),
            &RenderConfig::default(),
        );
        assert_ne!(identity(&next_job, "Roboto", AssetKind::Font), before);
    }

    #[test]
    fn a_rewritten_file_has_a_new_identity_in_the_next_job_for_every_kind() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        // Each version of a file has other content, of another length.
        let write = |name: &str, version: u32| -> bool {
            let path = root.join(name);
            let (lavfi, output): (String, &[&str]) = match name {
                "plate.png" => (
                    format!("color=c=red:s={}x2", 2 * version),
                    &["-frames:v", "1"],
                ),
                "clip.mp4" => (
                    format!("color=c=red:s={}x16:d=0.2", 16 * version),
                    &["-c:v", "libx264", "-pix_fmt", "yuv420p"],
                ),
                "tone.wav" => (format!("sine=frequency=440:duration=0.{version}"), &[]),
                _ => {
                    std::fs::write(&path, b"glTF#?RADIANCE".repeat(version as usize)).unwrap();
                    return true;
                }
            };
            let status = std::process::Command::new("ffmpeg")
                .args(["-y", "-loglevel", "error", "-f", "lavfi", "-i", &lavfi])
                .args(output)
                .arg(&path)
                .status();
            matches!(status, Ok(status) if status.success())
        };
        let next_job = || LocalMediaStore::new(root.clone(), &RenderConfig::default());
        for (name, kind) in [
            ("plate.png", AssetKind::Image),
            ("clip.mp4", AssetKind::Video),
            ("tone.wav", AssetKind::Audio),
            ("fox.glb", AssetKind::Model),
            ("sky.hdr", AssetKind::Environment),
        ] {
            if !write(name, 1) {
                eprintln!("ffmpeg CLI unavailable; skipping {name}");
                continue;
            }
            let before = identity(&next_job(), name, kind);
            assert_eq!(identity(&next_job(), name, kind), before, "{name}");
            assert!(write(name, 2));
            assert_ne!(identity(&next_job(), name, kind), before, "{name}");
        }
    }

    #[test]
    fn an_asset_is_named_by_its_id_and_file_not_the_files_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let stored = dir.path().join("m_1");
        std::fs::write(&stored, b"glTF").unwrap();
        let store = LocalMediaStore::new(dir.path().to_path_buf(), &RenderConfig::default())
            .with_assets(Arc::new(Assets(HashMap::from([
                ("one", Asset::Model(stored.clone())),
                ("two", Asset::Model(stored.clone())),
                ("face", Asset::Font(vec![stored.clone()])),
            ]))));
        let model = |id: &str| identity(&store, &format!("asset:{id}"), AssetKind::Model);
        let font = || identity(&store, "asset:face", AssetKind::Font);
        let (one, face) = (model("one"), font());
        assert_ne!(model("two"), one);
        // A file the asset store holds doesn't change under its id, so it isn't looked at.
        std::fs::write(&stored, b"glTF, longer").unwrap();
        assert_eq!(model("one"), one);
        assert_eq!(font(), face);
    }

    #[test]
    fn a_font_staged_under_its_alias_serves_the_family_of_that_name() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("brand"), ROBOTO).unwrap();
        let store = LocalMediaStore::new(
            root.path().canonicalize().unwrap(),
            &RenderConfig::default(),
        );

        let font = resolver::<dyn FontResolver>(&store, "brand").unwrap();
        assert_eq!(font.data().unwrap(), vec![ROBOTO.to_vec()]);
        let other = resolver::<dyn FontResolver>(&store, "other").err().unwrap();
        assert_eq!(other.code(), "media.not_found");
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
            let first = scope.spawn(|| cached_resolver(&cache, "a.png", None, open).unwrap().1);
            let second = scope.spawn(|| cached_resolver(&cache, "a.png", None, open).unwrap().1);
            (first.join().unwrap(), second.join().unwrap())
        });

        assert!(Arc::ptr_eq(&first, &second));
        assert!(Arc::ptr_eq(
            &first,
            &cached_resolver(&cache, "a.png", None, || panic!("opened a cached source"))
                .unwrap()
                .1
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
    fn a_failure_after_open_names_the_source_as_written_not_the_file() {
        let root = tempfile::tempdir().unwrap();
        let root = root.path().canonicalize().unwrap();
        let assets = tempfile::tempdir().unwrap();
        let assets = assets.path().canonicalize().unwrap();
        let tone = |path: &Path| {
            let status = std::process::Command::new("ffmpeg")
                .args(["-y", "-loglevel", "error", "-f", "lavfi"])
                .args(["-i", "sine=frequency=440:duration=1", "-c:a", "pcm_s16le"])
                .arg(path.with_extension("wav"))
                .status();
            let made = matches!(status, Ok(status) if status.success());
            if made {
                std::fs::rename(path.with_extension("wav"), path).unwrap();
            }
            made
        };
        let (plain, stored) = (root.join("tone.wav"), assets.join("voice"));
        if !tone(&plain) || !tone(&stored) {
            eprintln!("ffmpeg CLI unavailable; skipping");
            return;
        }
        let store =
            LocalMediaStore::new(root.clone(), &RenderConfig::default()).with_assets(Arc::new(
                Assets(HashMap::from([("voice", Asset::Audio(stored.clone()))])),
            ));
        for (source, file) in [("tone.wav", &plain), ("asset:voice", &stored)] {
            let resolver = resolver::<dyn AudioResolver>(&store, source).unwrap();
            resolver.resolve_range(0, 1024).unwrap();
            // The file goes away, as a truncated or replaced one would fail the next read.
            std::fs::remove_file(file).unwrap();
            let error = resolver.resolve_range(40_000, 1024).unwrap_err();
            let failure = error.failure().expect("a media failure");
            assert_eq!(failure.source, source);
            for dir in [&root, &assets] {
                let shown = dir.display().to_string();
                assert!(!error.to_string().contains(&shown), "{error}");
            }
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
                ("fox", Asset::Model(fox)),
                ("sky", Asset::Environment(sky)),
            ]))),
        );
        let image_error = |source: &str| match resolver::<dyn ImageResolver>(&store, source) {
            Ok(_) => panic!("{source} should not open"),
            Err(error) => error,
        };

        let decode = image_error("asset:logo");
        assert_eq!(decode.code(), "media.decode");
        assert!(decode.to_string().contains("asset:logo"), "{decode}");
        assert_eq!(image_error("asset:other").code(), "media.not_found");
        assert_eq!(image_error("asset:brand").code(), "media.wrong_kind");
        let model = |source: &str| {
            store
                .resolve(source, AssetKind::Model)
                .map(|resolved| resolved.identity)
        };
        assert!(model("asset:fox").is_ok());
        assert_eq!(model("asset:logo").unwrap_err().code(), "media.wrong_kind");
        let sky = resolver::<dyn EnvironmentResolver>(&store, "asset:sky").unwrap();
        assert_eq!(sky.data().unwrap(), b"#?RADIANCE");
        let wrong_sky = resolver::<dyn EnvironmentResolver>(&store, "asset:fox")
            .err()
            .unwrap();
        assert_eq!(wrong_sky.code(), "media.wrong_kind");
        let font = resolver::<dyn FontResolver>(&store, "asset:brand").unwrap();
        assert_eq!(font.data().unwrap(), vec![ROBOTO.to_vec()]);
        let wrong_font = resolver::<dyn FontResolver>(&store, "asset:logo")
            .err()
            .unwrap();
        assert_eq!(wrong_font.code(), "media.wrong_kind");

        let big = assets.path().join("big");
        let file = std::fs::File::create(&big).unwrap();
        file.set_len(lumen_3d::model::MAX_MODEL_BYTES as u64 + 1)
            .unwrap();
        let store = LocalMediaStore::new(root.clone(), &RenderConfig::default()).with_assets(
            Arc::new(Assets(HashMap::from([("big", Asset::Model(big))]))),
        );
        let too_large = resolver::<dyn ModelResolver>(&store, "asset:big")
            .err()
            .unwrap();
        assert_eq!(too_large.code(), "media.too_large");
        let failure = too_large.failure().unwrap();
        assert_eq!(failure.source, "asset:big");
        assert!(matches!(
            failure.reason,
            MediaReason::TooLarge { limit, actual: Some(actual), .. }
                if limit == lumen_3d::model::MAX_MODEL_BYTES as u64 && actual == limit + 1
        ));

        let without_assets = LocalMediaStore::new(root, &RenderConfig::default());
        let missing = resolver::<dyn FontResolver>(&without_assets, "asset:brand")
            .err()
            .unwrap();
        assert_eq!(missing.code(), "media.not_found");
    }
}
