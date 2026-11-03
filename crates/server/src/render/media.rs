use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::{AudioBuffer, AudioResolver},
    error::MediaError,
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    image::ImageFileResolver,
    media::{FontResolver, ImageResolver, MediaFrame, MediaStore, TextSession, VideoFrameResolver},
};

pub const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";
pub const HARDWARE_DECODE_ENV: &str = "LUMEN_HARDWARE_DECODE";

pub(super) struct LocalMediaStore {
    root: PathBuf,
    font_root: Option<PathBuf>,
    pub(super) video_options: FfmpegResolverOptions,
    audios: RwLock<HashMap<String, Arc<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Arc<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Arc<FfmpegVideoResolver>>>,
    /// What each family asked for resolved to, so a job probes the filesystem once per family.
    fonts: RwLock<HashMap<String, Option<Arc<ResolvedFont>>>>,
    text: TextSession,
}

/// The files a font family resolved to and the identity of their current contents.
#[derive(Debug)]
struct ResolvedFont {
    id: String,
    paths: Vec<String>,
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
            fonts: RwLock::new(HashMap::new()),
            text: TextSession::default(),
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

    /// `source` under the media root. The failure doesn't say why, as telling a missing file from
    /// one outside the root would let a composition probe the server's disk.
    fn stored_source(&self, source: &str) -> Result<String, MediaError> {
        self.resolve_source(source)
            .ok_or_else(|| MediaError::source_not_found(source, "no such file in the media root"))
    }

    fn resolved_font(&self, font_family: &str) -> Option<Arc<ResolvedFont>> {
        if let Ok(cache) = self.fonts.read()
            && let Some(resolved) = cache.get(font_family)
        {
            return resolved.clone();
        }

        let resolved = self.resolve_font_family(font_family).map(|paths| {
            Arc::new(ResolvedFont {
                id: font_file_identity(&paths),
                paths,
            })
        });
        if let Ok(mut cache) = self.fonts.write() {
            cache
                .entry(font_family.to_string())
                .or_insert_with(|| resolved.clone());
        }
        resolved
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

    fn image_resolver(&self, source: &str) -> Result<Arc<ImageFileResolver>, MediaError> {
        if let Ok(cache) = self.images.read()
            && let Some(resolver) = cache.get(source)
        {
            return Ok(Arc::clone(resolver));
        }

        let resolver = Arc::new(ImageFileResolver::open(source.to_string())?);
        if let Ok(mut cache) = self.images.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Ok(resolver)
    }

    fn video_resolver(&self, source: &str) -> Result<Arc<FfmpegVideoResolver>, MediaError> {
        if let Ok(cache) = self.videos.read()
            && let Some(resolver) = cache.get(source)
        {
            return Ok(Arc::clone(resolver));
        }

        let resolver = match FfmpegVideoResolver::open_with_options(
            source.to_string(),
            self.video_options.clone(),
        ) {
            Ok(resolver) => Arc::new(resolver),
            Err(error) => {
                tracing::warn!(source, %error, "failed opening video media resolver");
                return Err(error);
            }
        };
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
        Ok(resolver)
    }

    fn audio_resolver(&self, source: &str) -> Result<Arc<FfmpegAudioResolver>, MediaError> {
        if let Ok(cache) = self.audios.read()
            && let Some(resolver) = cache.get(source)
        {
            return Ok(Arc::clone(resolver));
        }

        let resolver = Arc::new(FfmpegAudioResolver::open(source.to_string())?);
        if let Ok(mut cache) = self.audios.write() {
            cache
                .entry(source.to_string())
                .or_insert_with(|| Arc::clone(&resolver));
        }
        Ok(resolver)
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
        Ok(value) => FfmpegResolverOptions::with_hardware_decode(matches_env_flag(&value)),
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

    fn frame(&self) -> Result<MediaFrame, MediaError> {
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

    fn enqueue_frame(&self, frame: u32) -> Result<(), MediaError> {
        self.0.enqueue_frame(frame)
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
        self.0.frame(frame)
    }

    fn retain_frames(&self, frames: &[u32]) {
        self.0.retain_frames(frames);
    }
}

#[derive(Clone)]
struct SharedAudioResolver(Arc<FfmpegAudioResolver>);

impl AudioResolver for SharedAudioResolver {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError> {
        self.0.resolve_range(start_sample, frames)
    }
}

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Result<Box<dyn ImageResolver>, MediaError> {
        let resolved = self.stored_source(source)?;
        let resolver = self.image_resolver(&resolved)?;
        Ok(Box::new(SharedImageResolver(resolver)))
    }

    fn get_video_resolver(
        &self,
        stream_id: &str,
    ) -> Result<Box<dyn VideoFrameResolver>, MediaError> {
        let resolved = self.stored_source(stream_id)?;
        let resolver = self.video_resolver(&resolved)?;
        Ok(Box::new(SharedVideoResolver(resolver)))
    }

    fn get_audio_resolver(&self, source_id: &str) -> Result<Box<dyn AudioResolver>, MediaError> {
        let resolved = self.stored_source(source_id)?;
        let resolver = self.audio_resolver(&resolved)?;
        Ok(Box::new(SharedAudioResolver(resolver)))
    }

    fn font_resolver_id(&self, font_family: &str) -> Option<String> {
        Some(self.resolved_font(font_family)?.id.clone())
    }

    fn get_font_resolver(&self, font_family: &str) -> Option<Box<dyn FontResolver>> {
        let font = self.resolved_font(font_family)?;
        tracing::info!(
            font_family,
            sources = font.paths.len(),
            "opened font media resolver"
        );
        Some(Box::new(LocalFontResolver {
            family: font_family.to_string(),
            font,
        }))
    }

    fn text_session(&self) -> Option<&TextSession> {
        Some(&self.text)
    }
}

#[derive(Debug)]
struct LocalFontResolver {
    family: String,
    font: Arc<ResolvedFont>,
}

impl FontResolver for LocalFontResolver {
    fn id(&self) -> &str {
        &self.font.id
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        self.font
            .paths
            .iter()
            .map(|path| {
                std::fs::read(path).map_err(|err| MediaError::Decode {
                    media_source: self.family.clone(),
                    details: format!("failed reading font data `{path}`: {err}"),
                })
            })
            .collect()
    }
}

/// Names the current contents of `paths`, so that an ID stops matching once a file changes.
fn font_file_identity(paths: &[String]) -> String {
    let files: Vec<String> = paths
        .iter()
        .map(|path| {
            let metadata = std::fs::metadata(path).ok();
            let len = metadata.as_ref().map(std::fs::Metadata::len);
            let modified = metadata.and_then(|metadata| metadata.modified().ok());
            format!("{path}:{len:?}:{modified:?}")
        })
        .collect();
    format!("local-font:{}", files.join(";"))
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

#[cfg(test)]
mod tests {
    use lumen_engine::expr::{Expression, ExpressionContext};

    use super::*;

    const ROBOTO: &[u8] = include_bytes!("../../../text/assets/roboto/Roboto-Regular.ttf");

    #[test]
    fn a_source_outside_the_root_fails_like_a_missing_one() {
        let outside = tempfile::NamedTempFile::new().unwrap();
        let root = tempfile::tempdir().unwrap();
        let store = LocalMediaStore::new(root.path().canonicalize().unwrap());
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

    /// A media root holding `font` as the job's `Roboto`.
    fn job_with_roboto(font: &[u8]) -> (tempfile::TempDir, LocalMediaStore) {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("fonts")).unwrap();
        std::fs::write(root.path().join("fonts/Roboto.ttf"), font).unwrap();
        let store = LocalMediaStore::new(root.path().canonicalize().unwrap());
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
    fn a_job_resolves_each_font_family_once() {
        let (root, store) = job_with_roboto(ROBOTO);
        assert_eq!(store.font_resolver_id("Late"), None);
        std::fs::write(root.path().join("fonts/Late.ttf"), ROBOTO).unwrap();
        assert_eq!(store.font_resolver_id("Late"), None);

        let next_job = LocalMediaStore::new(root.path().canonicalize().unwrap());
        assert!(next_job.font_resolver_id("Late").is_some());
    }

    #[test]
    fn a_changed_font_file_changes_the_resolver_id() {
        let (root, store) = job_with_roboto(ROBOTO);
        let before = store.font_resolver_id("Roboto").unwrap();
        std::fs::write(root.path().join("fonts/Roboto.ttf"), &ROBOTO[..1024]).unwrap();

        let next_job = LocalMediaStore::new(root.path().canonicalize().unwrap());
        assert_ne!(next_job.font_resolver_id("Roboto").unwrap(), before);
    }
}
