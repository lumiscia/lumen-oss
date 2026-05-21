use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::{AudioBuffer, AudioResolver},
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    image::ImageFileResolver,
    media::{FontResolver, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver},
};

pub const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";
pub const HARDWARE_DECODE_ENV: &str = "LUMEN_HARDWARE_DECODE";

pub(super) struct LocalMediaStore {
    root: PathBuf,
    font_root: Option<PathBuf>,
    video_options: FfmpegResolverOptions,
    audios: RwLock<HashMap<String, Arc<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Arc<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Arc<FfmpegVideoResolver>>>,
}

impl LocalMediaStore {
    pub(super) fn new(root: PathBuf) -> Self {
        let video_options = video_resolver_options_from_env();
        let font_root = env::var(MEDIA_ROOT_ENV)
            .ok()
            .and_then(|raw| PathBuf::from(raw).canonicalize().ok())
            .filter(|font_root| font_root != &root);
        tracing::info!(
            media_root = %root.display(),
            font_root = font_root.as_ref().map(|root| root.display().to_string()).as_deref().unwrap_or("<media-root>"),
            prefer_hardware_decode = video_options.prefer_hardware_decode,
            hardware_decode_env = env::var(HARDWARE_DECODE_ENV).ok().as_deref().unwrap_or("<unset>"),
            "created local media store"
        );
        Self {
            root,
            font_root,
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

    fn resolve_font_family(&self, font_family: &str) -> Option<Vec<String>> {
        if font_family.trim().is_empty() {
            return None;
        }

        if let Some(direct) = self.resolve_font_source(font_family) {
            return Some(vec![direct]);
        }

        let file_stem = font_family.replace(['/', '\\'], "");
        let mut resolved = self.resolve_font_family_files(&file_stem);
        for candidate in [
            format!("fonts/{file_stem}.ttf"),
            format!("fonts/{file_stem}.otf"),
            format!("fonts/{file_stem}.ttc"),
            format!("fonts/{file_stem}.otc"),
            format!("fonts/{file_stem}-Regular.ttf"),
            format!("fonts/{file_stem}-Regular.otf"),
            format!("fonts/{file_stem}-Regular.ttc"),
            format!("fonts/{file_stem}-Regular.otc"),
        ] {
            if let Some(path) = self.resolve_font_source(&candidate)
                && !resolved.contains(&path)
            {
                resolved.push(path);
            }
        }
        (!resolved.is_empty()).then_some(resolved)
    }

    fn resolve_font_source(&self, source: &str) -> Option<String> {
        self.resolve_source(source).or_else(|| {
            self.font_root
                .as_ref()
                .and_then(|root| resolve_local_path_with_root(source, root).ok())
                .map(|path| path.to_string_lossy().to_string())
        })
    }

    fn resolve_font_family_files(&self, file_stem: &str) -> Vec<String> {
        let mut paths = [Some(&self.root), self.font_root.as_ref()]
            .into_iter()
            .flatten()
            .flat_map(|root| {
                let fonts_dir = root.join("fonts");
                std::fs::read_dir(fonts_dir)
                    .into_iter()
                    .flat_map(|entries| entries.filter_map(Result::ok))
                    .filter_map(|entry| {
                        let path = entry.path();
                        let extension = path.extension()?.to_str()?.to_ascii_lowercase();
                        if !matches!(extension.as_str(), "ttf" | "otf" | "ttc" | "otc") {
                            return None;
                        }
                        let stem = path.file_stem()?.to_str()?;
                        if stem == file_stem || stem.starts_with(&format!("{file_stem}-")) {
                            path.canonicalize()
                                .ok()
                                .map(|path| path.to_string_lossy().to_string())
                        } else {
                            None
                        }
                    })
                    .collect::<Vec<_>>()
            })
            .fold(Vec::new(), |mut paths, path| {
                if !paths.contains(&path) {
                    paths.push(path);
                }
                paths
            });
        paths.sort();
        paths
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

    fn metadata(&self) -> lumen_engine::media::ImageMetadata {
        self.0.metadata()
    }

    fn frame(&self) -> Result<MediaFrame, lumen_engine::error::MediaError> {
        self.0.frame()
    }
}

#[derive(Clone)]
struct SharedVideoResolver(Arc<FfmpegVideoResolver>);

impl VideoFrameResolver for SharedVideoResolver {
    fn id(&self) -> &str {
        self.0.id()
    }

    fn metadata(&self) -> lumen_engine::media::VideoMetadata {
        self.0.metadata()
    }

    fn enqueue_frame(&self, frame: u32) -> Result<(), lumen_engine::error::MediaError> {
        self.0.enqueue_frame(frame)
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, lumen_engine::error::MediaError> {
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

    fn metadata(&self) -> lumen_engine::audio::AudioMetadata {
        self.0.metadata()
    }

    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, lumen_engine::error::MediaError> {
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

    fn get_audio_resolver(&self, source_id: &str) -> Option<Box<dyn AudioResolver>> {
        let resolved = self.resolve_source(source_id)?;
        let resolver = self.audio_resolver(&resolved)?;
        Some(Box::new(SharedAudioResolver(resolver)))
    }

    fn get_font_resolver(&self, font_family: &str) -> Option<Box<dyn FontResolver>> {
        let paths = self.resolve_font_family(font_family)?;
        tracing::info!(
            font_family,
            sources = paths.len(),
            "opened font media resolver"
        );
        Some(Box::new(LocalFontResolver {
            id: font_family.to_string(),
            paths,
        }))
    }
}

#[derive(Debug)]
struct LocalFontResolver {
    id: String,
    paths: Vec<String>,
}

impl FontResolver for LocalFontResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, lumen_engine::error::MediaError> {
        self.paths
            .iter()
            .map(|path| {
                std::fs::read(path).map_err(|err| lumen_engine::error::MediaError::Decode {
                    media_source: self.id.clone(),
                    details: format!("failed reading font data `{path}`: {err}"),
                })
            })
            .collect()
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
