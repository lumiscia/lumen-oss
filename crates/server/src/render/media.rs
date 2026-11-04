use std::{
    collections::HashMap,
    env,
    path::{Component, Path, PathBuf},
    sync::{Arc, RwLock},
};

use anyhow::{Context, anyhow};
use lumen_engine::{
    audio::AudioResolver,
    error::MediaError,
    ffmpeg::{FfmpegAudioResolver, FfmpegResolverOptions, FfmpegVideoResolver},
    image::ImageFileResolver,
    media::{FontResolver, ImageResolver, MediaStore, TextSession, VideoFrameResolver},
};

use super::fonts::{FontCatalog, ResolvedFont};

pub const MEDIA_ROOT_ENV: &str = "LUMEN_MEDIA_ROOT";
pub const HARDWARE_DECODE_ENV: &str = "LUMEN_HARDWARE_DECODE";

/// Serves one render's media from under `root`, with its fonts.
pub struct LocalMediaStore {
    root: PathBuf,
    explain_missing_sources: bool,
    pub(super) video_options: FfmpegResolverOptions,
    audios: RwLock<HashMap<String, Arc<FfmpegAudioResolver>>>,
    images: RwLock<HashMap<String, Arc<ImageFileResolver>>>,
    videos: RwLock<HashMap<String, Arc<FfmpegVideoResolver>>>,
    fonts: FontCatalog,
    text: TextSession,
}

impl LocalMediaStore {
    pub fn new(root: PathBuf, font_dir: Option<&Path>) -> Self {
        let video_options = video_resolver_options_from_env();
        let fonts = FontCatalog::new(font_dir, &root);
        tracing::info!(
            media_root = %root.display(),
            prefer_hardware_decode = video_options.prefer_hardware_decode,
            hardware_decode_env = env::var(HARDWARE_DECODE_ENV).ok().as_deref().unwrap_or("<unset>"),
            "created local media store"
        );
        Self {
            root,
            explain_missing_sources: false,
            video_options,
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

    fn stored_source(&self, source: &str) -> Result<String, MediaError> {
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

    fn resolved_font(&self, font_family: &str) -> Option<ResolvedFont> {
        self.fonts.resolve(font_family, || {
            resolve_local_path_with_root(font_family, &self.root).ok()
        })
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

impl MediaStore for LocalMediaStore {
    fn get_image_resolver(&self, source: &str) -> Result<Box<dyn ImageResolver>, MediaError> {
        let resolved = self.stored_source(source)?;
        Ok(Box::new(self.image_resolver(&resolved)?))
    }

    fn get_video_resolver(
        &self,
        stream_id: &str,
    ) -> Result<Box<dyn VideoFrameResolver>, MediaError> {
        let resolved = self.stored_source(stream_id)?;
        Ok(Box::new(self.video_resolver(&resolved)?))
    }

    fn get_audio_resolver(&self, source_id: &str) -> Result<Box<dyn AudioResolver>, MediaError> {
        let resolved = self.stored_source(source_id)?;
        Ok(Box::new(self.audio_resolver(&resolved)?))
    }

    fn font_resolver_id(&self, font_family: &str) -> Option<String> {
        Some(self.resolved_font(font_family)?.id().to_string())
    }

    fn get_font_resolver(&self, font_family: &str) -> Option<Box<dyn FontResolver>> {
        let font = self.resolved_font(font_family)?;
        tracing::info!(
            font_family,
            sources = font.files().len(),
            "opened font media resolver"
        );
        Some(Box::new(font))
    }

    fn text_session(&self) -> Option<&TextSession> {
        Some(&self.text)
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
        let store = LocalMediaStore::new(root.path().canonicalize().unwrap(), None);
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
        let store = LocalMediaStore::new(root.clone(), None).explaining_missing_sources();
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
        let store = LocalMediaStore::new(root.path().canonicalize().unwrap(), None);
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
    fn a_changed_font_file_changes_the_resolver_id() {
        let (root, store) = job_with_roboto(ROBOTO);
        let before = store.font_resolver_id("Roboto").unwrap();
        let changed = [ROBOTO, &[0]].concat();
        std::fs::write(root.path().join("assets/fonts/Roboto.ttf"), changed).unwrap();

        let next_job = LocalMediaStore::new(root.path().canonicalize().unwrap(), None);
        assert_ne!(next_job.font_resolver_id("Roboto").unwrap(), before);
    }

    #[test]
    fn a_font_staged_under_its_alias_serves_the_family_of_that_name() {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("brand"), ROBOTO).unwrap();
        let store = LocalMediaStore::new(root.path().canonicalize().unwrap(), None);

        let resolver = store.get_font_resolver("brand").unwrap();
        assert_eq!(resolver.data().unwrap(), vec![ROBOTO.to_vec()]);
        assert!(store.get_font_resolver("other").is_none());
    }
}
