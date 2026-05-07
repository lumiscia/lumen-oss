use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context, anyhow};
use lumen::{
    audio::{AudioBuffer, AudioResolver, AudioSourceProvider},
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    image::ImageFileResolver,
    media::{ImageResolver, MediaFrame, MediaStore, VideoFrameResolver},
};

pub const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";
pub const HARDWARE_DECODE_ENV: &str = "LUMEN_HARDWARE_DECODE";

pub(super) struct LocalMediaStore {
    root: PathBuf,
    video_options: FfmpegResolverOptions,
    audios: RwLock<HashMap<String, Arc<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Arc<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Arc<FfmpegVideoResolver>>>,
}

impl LocalMediaStore {
    pub(super) fn new(root: PathBuf) -> Self {
        let video_options = video_resolver_options_from_env();
        tracing::info!(
            media_root = %root.display(),
            prefer_hardware_decode = video_options.prefer_hardware_decode,
            hardware_decode_env = env::var(HARDWARE_DECODE_ENV).ok().as_deref().unwrap_or("<unset>"),
            "created local media store"
        );
        Self {
            root,
            video_options,
            audios: RwLock::new(HashMap::new()),
            images: RwLock::new(HashMap::new()),
            videos: RwLock::new(HashMap::new()),
        }
    }

    fn resolve_source(&self, source: &str) -> Option<String> {
        if is_http_url(source) {
            return None;
        }

        resolve_local_path_with_root(source, &self.root)
            .ok()
            .map(|path| path.to_string_lossy().to_string())
    }

    fn image_resolver(&self, source: &str) -> Option<Arc<ImageFileResolver>> {
        if let Ok(cache) = self.images.read()
            && let Some(resolver) = cache.get(source)
        {
            return Some(Arc::clone(resolver));
        }

        let resolver = Arc::new(ImageFileResolver::open(source.to_string()).ok()?);
        if let Ok(mut cache) = self.images.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Some(resolver)
    }

    fn video_resolver(&self, source: &str) -> Option<Arc<FfmpegVideoResolver>> {
        if let Ok(cache) = self.videos.read()
            && let Some(resolver) = cache.get(source)
        {
            return Some(Arc::clone(resolver));
        }

        let resolver = Arc::new(
            FfmpegVideoResolver::open_with_options(source.to_string(), self.video_options).ok()?,
        );
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
        if let Ok(mut cache) = self.videos.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Some(resolver)
    }

    fn audio_resolver(&self, source: &str) -> Option<Arc<FfmpegAudioResolver>> {
        if let Ok(cache) = self.audios.read()
            && let Some(resolver) = cache.get(source)
        {
            return Some(Arc::clone(resolver));
        }

        let resolver = Arc::new(FfmpegAudioResolver::open(source.to_string()).ok()?);
        if let Ok(mut cache) = self.audios.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Some(resolver)
    }
}

impl std::fmt::Debug for LocalMediaStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LocalMediaStore")
            .field("root", &self.root)
            .field("video_options", &self.video_options)
            .finish_non_exhaustive()
    }
}

fn video_resolver_options_from_env() -> FfmpegResolverOptions {
    match env::var(HARDWARE_DECODE_ENV) {
        Ok(value) => FfmpegResolverOptions {
            prefer_hardware_decode: matches_env_flag(&value),
        },
        Err(_) => FfmpegResolverOptions::default(),
    }
}

fn matches_env_flag(value: &str) -> bool {
    matches!(
        value.trim().to_ascii_lowercase().as_str(),
        "1" | "true" | "yes" | "on"
    )
}

#[derive(Debug, Clone)]
struct SharedImageResolver(Arc<ImageFileResolver>);

impl ImageResolver for SharedImageResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen::media::ImageMetadata {
        self.0.metadata()
    }

    fn frame(&self) -> Result<MediaFrame, lumen::error::MediaError> {
        self.0.frame()
    }
}

#[derive(Clone)]
struct SharedVideoResolver(Arc<FfmpegVideoResolver>);

impl VideoFrameResolver for SharedVideoResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen::media::VideoMetadata {
        self.0.metadata()
    }

    fn enqueue_frame(&self, frame: u32) -> Result<(), lumen::error::MediaError> {
        self.0.enqueue_frame(frame)
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, lumen::error::MediaError> {
        self.0.frame(frame)
    }

    fn retain_frames(&self, frames: &[u32]) {
        self.0.retain_frames(frames);
    }
}

#[derive(Clone)]
struct SharedAudioResolver(Arc<FfmpegAudioResolver>);

impl AudioResolver for SharedAudioResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen::audio::AudioMetadata {
        self.0.metadata()
    }

    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, lumen::error::MediaError> {
        self.0.resolve_range(start_sample, frames)
    }
}

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let resolved = self.resolve_source(source)?;
        let resolver = self.image_resolver(&resolved)?;
        Some(Box::new(SharedImageResolver(resolver)))
    }

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        let resolved = self.resolve_source(stream_id)?;
        let resolver = self.video_resolver(&resolved)?;
        Some(Box::new(SharedVideoResolver(resolver)))
    }
}

impl AudioSourceProvider for LocalMediaStore {
    fn get_audio_resolver(&self, source_id: &str) -> Option<Box<dyn AudioResolver>> {
        let resolved = self.resolve_source(source_id)?;
        let resolver = self.audio_resolver(&resolved)?;
        Some(Box::new(SharedAudioResolver(resolver)))
    }
}

pub(super) fn media_root(override_root: Option<&Path>) -> anyhow::Result<PathBuf> {
    let path = match override_root {
        Some(path) => path.to_path_buf(),
        None => {
            let raw = env::var(MEDIA_ROOT_ENV).unwrap_or_else(|_| ".".to_string());
            PathBuf::from(raw)
        }
    };
    path.canonicalize()
        .with_context(|| format!("failed to resolve media root from {MEDIA_ROOT_ENV}"))
}

fn resolve_local_path_with_root(source: &str, root: &Path) -> anyhow::Result<PathBuf> {
    let root = root
        .canonicalize()
        .with_context(|| format!("failed to canonicalize media root `{}`", root.display()))?;

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

fn is_http_url(source: &str) -> bool {
    source.starts_with("http://") || source.starts_with("https://")
}
