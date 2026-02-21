// Items here are used conditionally by feature-gated backends (libav / subprocess).
#![allow(dead_code)]

use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    env,
    hash::{Hash, Hasher},
    io::Write,
    path::{Component, Path, PathBuf},
    process::{Command, Stdio},
    sync::{Mutex, mpsc},
    time::Duration,
};

use reqwest::blocking::Client;
use tempfile::NamedTempFile;

use anyhow::{Context, anyhow};
use lumen::{
    backend::{FrameImage, FrameProvider, ProvidedFrame, ProviderError, Renderer},
    compile::{
        CompiledLayoutNode, CompiledLayoutNodeKind, CompiledOperationKind, CompiledTimeline,
    },
    model::{Source, SourceKind},
    time::Rational,
};

pub const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";
pub const DEFAULT_ENCODE_QUEUE: usize = 8;
pub const DEFAULT_MAX_DECODED_FRAMES: usize = 120_000;
#[allow(dead_code)]
pub const DEFAULT_STREAM_CACHE_FRAMES: usize = 256;

pub fn create_renderer(width: u32, height: u32) -> anyhow::Result<Box<dyn Renderer>> {
    #[cfg(feature = "renderer-skia")]
    {
        let renderer = lumen::backend::skia::SkiaRenderer::new(width, height)
            .map_err(|err| anyhow!("failed to initialize Skia renderer: {err}"))?;
        return Ok(Box::new(renderer));
    }
    #[cfg(not(feature = "renderer-skia"))]
    {
        Err(anyhow!("renderer-skia feature is required"))
    }
}

#[derive(Debug, Clone, Default)]
pub struct RenderBackendOptions {
    pub media_root: Option<PathBuf>,
    pub video_encoder: Option<String>,
    pub encode_queue: Option<usize>,
    pub max_decoded_source_frames: Option<usize>,
    pub stream_cache_frames: Option<usize>,
}

pub struct WebAssetCache {
    root: PathBuf,
    index: Mutex<HashMap<String, PathBuf>>,
    client: Client,
    _temp_dir: Option<tempfile::TempDir>,
}

impl WebAssetCache {
    pub fn new_temp() -> anyhow::Result<Self> {
        let temp_dir = tempfile::tempdir().context("failed to create web asset cache")?;
        Self::new_with_root(temp_dir.path().to_path_buf(), Some(temp_dir))
    }

    fn new_with_root(root: PathBuf, temp_dir: Option<tempfile::TempDir>) -> anyhow::Result<Self> {
        std::fs::create_dir_all(&root).with_context(|| {
            format!("failed to create web asset cache dir `{}`", root.display())
        })?;
        let client = Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .context("failed to build web asset cache http client")?;
        Ok(Self {
            root,
            index: Mutex::new(HashMap::new()),
            client,
            _temp_dir: temp_dir,
        })
    }

    pub fn resolve(&self, url: &str) -> anyhow::Result<PathBuf> {
        if let Some(path) = self.cached_path(url)? {
            return Ok(path);
        }

        let target = self.cache_path(url)?;
        if target.exists() {
            self.insert_cached(url, &target)?;
            return Ok(target);
        }

        let response = self
            .client
            .get(url)
            .send()
            .with_context(|| format!("failed to download asset `{url}`"))?;
        let status = response.status();
        if !status.is_success() {
            return Err(anyhow!("failed to download asset `{url}`: HTTP {status}"));
        }
        let bytes = response
            .bytes()
            .with_context(|| format!("failed to read asset `{url}`"))?;

        let mut temp_file = NamedTempFile::new_in(&self.root)
            .with_context(|| format!("failed to create temp file for `{url}`"))?;
        temp_file
            .write_all(&bytes)
            .with_context(|| format!("failed to write asset `{url}`"))?;
        match temp_file.persist(&target) {
            Ok(_) => {}
            Err(err) => {
                if !target.exists() {
                    return Err(err).with_context(|| format!("failed to persist asset `{url}`"));
                }
            }
        }

        self.insert_cached(url, &target)?;
        Ok(target)
    }

    fn cached_path(&self, url: &str) -> anyhow::Result<Option<PathBuf>> {
        let lock = self
            .index
            .lock()
            .map_err(|_| anyhow!("web asset cache lock poisoned"))?;
        Ok(lock.get(url).cloned())
    }

    fn insert_cached(&self, url: &str, path: &Path) -> anyhow::Result<()> {
        let mut lock = self
            .index
            .lock()
            .map_err(|_| anyhow!("web asset cache lock poisoned"))?;
        lock.insert(url.to_string(), path.to_path_buf());
        Ok(())
    }

    fn cache_path(&self, url: &str) -> anyhow::Result<PathBuf> {
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        url.hash(&mut hasher);
        let hash = hasher.finish();
        let ext = extract_extension(url)
            .filter(|ext| !ext.is_empty())
            .unwrap_or("bin");
        Ok(self.root.join(format!("{hash}.{ext}")))
    }
}

#[derive(Default)]
pub struct FrameRequirements {
    pub images: HashSet<String>,
    pub videos: HashMap<String, BTreeSet<u64>>,
}

pub fn collect_requirements(
    timeline: &CompiledTimeline,
    frames: impl IntoIterator<Item = u64>,
) -> anyhow::Result<FrameRequirements> {
    let mut requirements = FrameRequirements::default();

    for frame in frames {
        let operation_indices = timeline
            .operation_indices_for_frame(frame)
            .map_err(|err| anyhow!(err.to_string()))?;

        for operation_index in operation_indices {
            let Some(operation) = timeline.operation(*operation_index) else {
                return Err(anyhow!("missing operation index {}", operation_index));
            };

            match &operation.kind {
                CompiledOperationKind::Image(image) => {
                    if let Some(source) = timeline.source(image.source_index) {
                        requirements.images.insert(source.id.clone());
                    }
                }
                CompiledOperationKind::Layout(layout) => {
                    collect_layout_image_requirements(
                        timeline,
                        &layout.root,
                        &mut requirements.images,
                    );
                }
                CompiledOperationKind::Video(video) => {
                    if let Some(source) = timeline.source(video.source_index)
                        && let Some(source_frame) =
                            operation.resolved_video_source_frame(frame, None)
                    {
                        requirements
                            .videos
                            .entry(source.id.clone())
                            .or_default()
                            .insert(source_frame);
                    }
                }
                CompiledOperationKind::Solid
                | CompiledOperationKind::Shape(_)
                | CompiledOperationKind::Text(_) => {}
            }
        }
    }

    Ok(requirements)
}

fn collect_layout_image_requirements(
    timeline: &CompiledTimeline,
    node: &CompiledLayoutNode,
    images: &mut HashSet<String>,
) {
    match &node.kind {
        CompiledLayoutNodeKind::Container { children } => {
            for child in children {
                collect_layout_image_requirements(timeline, child, images);
            }
        }
        CompiledLayoutNodeKind::Text { .. } => {}
        CompiledLayoutNodeKind::Image { source_index } => {
            if let Some(source) = timeline.source(*source_index) {
                images.insert(source.id.clone());
            }
        }
    }
}

#[derive(Default)]
pub struct PreparedAssets {
    pub images: HashMap<String, FrameImage>,
    pub videos: HashMap<String, BTreeMap<u64, FrameImage>>,
}

impl FrameProvider for PreparedAssets {
    fn image(&mut self, source_id: &str) -> Result<ProvidedFrame, ProviderError> {
        Ok(self
            .images
            .get(source_id)
            .cloned()
            .map(ProvidedFrame::Ready)
            .unwrap_or(ProvidedFrame::Missing))
    }

    fn video_frame(
        &mut self,
        source_id: &str,
        source_frame: u64,
    ) -> Result<ProvidedFrame, ProviderError> {
        let Some(frames) = self.videos.get(source_id) else {
            return Err(ProviderError::MissingSource(source_id.to_string()));
        };

        if let Some(frame) = frames.get(&source_frame) {
            return Ok(ProvidedFrame::Ready(frame.clone()));
        }

        let prev = frames.range(..=source_frame).next_back();
        let next = frames.range(source_frame..).next();

        Ok(prev
            .map(|(_, frame)| frame.clone())
            .or_else(|| next.map(|(_, frame)| frame.clone()))
            .map(ProvidedFrame::Ready)
            .unwrap_or(ProvidedFrame::Missing))
    }
}

pub fn decode_image_source(
    source: &Source,
    media_root: &Path,
    asset_cache: Option<&WebAssetCache>,
) -> anyhow::Result<FrameImage> {
    match &source.kind {
        SourceKind::File { path } => {
            let resolved = resolve_source_file_path(path, media_root, asset_cache)?;
            let image = image::ImageReader::open(&resolved)
                .with_context(|| format!("failed to open image `{}`", resolved.display()))?
                .decode()
                .with_context(|| format!("failed to decode image `{}`", resolved.display()))?;
            let rgba = image.into_rgba8();
            FrameImage::new(rgba.width(), rgba.height(), rgba.into_raw())
                .map_err(|err| anyhow!("failed to build image frame: {err}"))
        }
        SourceKind::Url { url } => {
            let resolved = resolve_source_file_path(url, media_root, asset_cache)?;
            let image = image::ImageReader::open(&resolved)
                .with_context(|| format!("failed to open image `{}`", resolved.display()))?
                .decode()
                .with_context(|| format!("failed to decode image `{}`", resolved.display()))?;
            let rgba = image.into_rgba8();
            FrameImage::new(rgba.width(), rgba.height(), rgba.into_raw())
                .map_err(|err| anyhow!("failed to build image frame: {err}"))
        }
    }
}

#[allow(dead_code)]
pub fn frame_size(width: u32, height: u32) -> anyhow::Result<usize> {
    let pixels = (width as usize)
        .checked_mul(height as usize)
        .ok_or_else(|| anyhow!("frame size overflow"))?;
    pixels
        .checked_mul(4)
        .ok_or_else(|| anyhow!("frame size overflow"))
}

pub fn choose_video_encoder(override_encoder: Option<&str>) -> String {
    if let Some(encoder) = override_encoder {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return encoder.to_string();
        }
    }

    if let Ok(encoder) = env::var("LUMEN_VIDEO_ENCODER") {
        let encoder = encoder.trim();
        if !encoder.is_empty() {
            return encoder.to_string();
        }
    }

    if cfg!(target_os = "macos") {
        return "h264_videotoolbox".to_string();
    }

    "libx264".to_string()
}

pub fn encode_rgba_stream(
    width: u32,
    height: u32,
    fps: Rational,
    encoder: String,
    rx: mpsc::Receiver<Vec<u8>>,
) -> anyhow::Result<Vec<u8>> {
    let tmp = tempfile::tempdir().context("failed to create temporary encode directory")?;
    let output_path = tmp.path().join("output.mp4");

    let mut command = Command::new("ffmpeg");
    command
        .arg("-y")
        .arg("-hide_banner")
        .arg("-loglevel")
        .arg("error")
        .arg("-nostdin")
        .arg("-f")
        .arg("rawvideo")
        .arg("-pix_fmt")
        .arg("rgba")
        .arg("-s:v")
        .arg(format!("{width}x{height}"))
        .arg("-r")
        .arg(format!("{}/{}", fps.num, fps.den))
        .arg("-i")
        .arg("pipe:0")
        .arg("-an")
        .arg("-c:v")
        .arg(&encoder)
        .arg("-pix_fmt")
        .arg("yuv420p")
        .arg("-movflags")
        .arg("+faststart")
        .arg(&output_path)
        .stdin(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = command.spawn().context("failed to spawn ffmpeg encoder")?;

    {
        let stdin = child
            .stdin
            .as_mut()
            .ok_or_else(|| anyhow!("ffmpeg stdin unavailable"))?;

        for frame in rx {
            stdin
                .write_all(&frame)
                .context("failed writing frame to ffmpeg stdin")?;
        }
    }

    let output = child
        .wait_with_output()
        .context("failed waiting for ffmpeg encode")?;

    if !output.status.success() {
        return Err(anyhow!(
            "ffmpeg encode failed with encoder `{encoder}`: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    std::fs::read(&output_path)
        .with_context(|| format!("failed to read encoded output `{}`", output_path.display()))
}

pub fn try_render_ffmpeg_fast_path(
    _timeline: &CompiledTimeline,
    _options: &RenderBackendOptions,
    _asset_cache: Option<&WebAssetCache>,
    _on_progress: &mut dyn FnMut(u64, u64),
) -> anyhow::Result<Option<Vec<u8>>> {
    Ok(None)
}

pub fn resolve_source_file_path(
    path: &str,
    root_override: &Path,
    asset_cache: Option<&WebAssetCache>,
) -> anyhow::Result<PathBuf> {
    let root = media_root(Some(root_override))?;
    if is_http_url(path) {
        let cache =
            asset_cache.ok_or_else(|| anyhow!("web asset cache unavailable for `{path}`"))?;
        return cache.resolve(path);
    }
    resolve_local_path_with_root(path, &root)
}

pub fn media_root(override_root: Option<&Path>) -> anyhow::Result<PathBuf> {
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

pub fn resolve_local_path_with_root(source: &str, root: &Path) -> anyhow::Result<PathBuf> {
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

fn extract_extension(url: &str) -> Option<&str> {
    let trimmed = url
        .split(['?', '#'])
        .next()
        .unwrap_or(url)
        .trim_end_matches('/');
    let name = trimmed.rsplit('/').next().unwrap_or("");
    let path = Path::new(name);
    path.extension().and_then(|ext| ext.to_str())
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::resolve_local_path_with_root;

    #[test]
    fn rejects_traversal_path() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_local_path_with_root("../secret.txt", tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn rejects_non_file_uri_scheme() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let result = resolve_local_path_with_root("https://example.com/video.mp4", tmp.path());
        assert!(result.is_err());
    }

    #[test]
    fn resolves_under_media_root() {
        let tmp = tempfile::tempdir().expect("tempdir");
        let file_path = tmp.path().join("clip.png");
        fs::write(&file_path, b"x").expect("write");

        let resolved = resolve_local_path_with_root("clip.png", tmp.path()).expect("resolve");
        assert_eq!(resolved, file_path.canonicalize().expect("canonicalize"));
    }
}
