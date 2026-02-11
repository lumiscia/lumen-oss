use std::{
    collections::HashMap,
    fs::File,
    num::NonZero,
    path::{Path, PathBuf},
};

use ac_ffmpeg::{
    codec::video::{VideoFrame, VideoFrameScaler, frame},
    format::io::IO,
    time::Timestamp,
};
use lru::LruCache;
use lumen::{
    media::{MediaError, MediaProvider},
    sequence::{Asset, AssetKind},
    skia::{AlphaType, ColorType, Data, Image, ImageInfo, images},
    time::{FrameIndex, Rational},
};

use crate::video::decode::VideoDecoder;

const IMAGE_CACHE_SIZE: usize = 32;
const VIDEO_FRAME_CACHE_SIZE: usize = 96;
const VIDEO_DECODE_WINDOW_FRAMES: u64 = 6;

pub struct AssetMediaProvider {
    assets: HashMap<String, Asset>,
    fps: Rational,
    image_cache: LruCache<String, Image>,
    video_states: HashMap<String, VideoState>,
}

impl AssetMediaProvider {
    pub fn new(assets: Vec<Asset>, fps: Rational) -> Self {
        Self {
            assets: assets.into_iter().map(|asset| (asset.id.clone(), asset)).collect(),
            fps,
            image_cache: LruCache::new(NonZero::new(IMAGE_CACHE_SIZE).expect("non-zero")),
            video_states: HashMap::new(),
        }
    }

    fn asset(&self, asset_id: &str) -> Result<&Asset, MediaError> {
        self.assets
            .get(asset_id)
            .ok_or_else(|| MediaError::MissingAsset(asset_id.to_string()))
    }

    fn load_image(&mut self, asset_id: &str) -> Result<Option<Image>, MediaError> {
        let asset = self.asset(asset_id)?;
        if asset.kind != AssetKind::Image {
            return Err(MediaError::Decode(format!(
                "asset `{asset_id}` is not an image asset"
            )));
        }

        let bytes = read_asset_bytes(&asset.source)?;
        let data = Data::new_copy(&bytes);
        let image = Image::from_encoded(data)
            .ok_or_else(|| MediaError::Decode(format!("failed to decode image asset `{asset_id}`")))?;

        Ok(Some(image))
    }

    fn video_state(&mut self, asset_id: &str) -> Result<&mut VideoState, MediaError> {
        if self.video_states.contains_key(asset_id) {
            return Ok(self.video_states.get_mut(asset_id).expect("state exists"));
        }

        let asset = self.asset(asset_id)?;
        if asset.kind != AssetKind::Video {
            return Err(MediaError::Decode(format!(
                "asset `{asset_id}` is not a video asset"
            )));
        }

        let path = resolve_local_path(&asset.source)?;
        let file = File::open(&path)
            .map_err(|err| MediaError::Source(format!("failed to open `{}`: {err}", path.display())))?;

        let decoder = VideoDecoder::new(IO::from_seekable_read_stream(file))
            .map_err(|err| MediaError::Decode(err.to_string()))?;

        self.video_states
            .insert(asset_id.to_string(), VideoState::new(path, decoder));

        Ok(self.video_states.get_mut(asset_id).expect("video state inserted"))
    }

    fn decode_video_frame(&mut self, asset_id: &str, frame: FrameIndex) -> Result<Option<Image>, MediaError> {
        let fps = self.fps;
        let state = self.video_state(asset_id)?;
        if let Some(image) = resolve_frame(state, frame.0) {
            state.last_resolved = Some(image.clone());
            return Ok(Some(image));
        }

        let seek_to = if state
            .last_requested
            .map(|last| frame.0 == last.saturating_add(1))
            .unwrap_or(false)
        {
            None
        } else {
            Some(frame_to_timestamp(frame.0, fps)?)
        };

        let duration = frame_to_timestamp(VIDEO_DECODE_WINDOW_FRAMES.max(1), fps)?;
        let mut decoded = state
            .decoder
            .decode(seek_to, duration)
            .map_err(|err| MediaError::Decode(err.to_string()))?;

        if decoded.is_empty() && seek_to.is_some() {
            decoded = state
                .decoder
                .decode(Some(frame_to_timestamp(frame.0, fps)?), frame_to_timestamp(1, fps)?)
                .map_err(|err| MediaError::Decode(err.to_string()))?;
        }

        for decoded_frame in decoded {
            let image = frame_to_image(state, &decoded_frame)?;
            let decoded_idx = timestamp_to_frame(decoded_frame.pts(), fps);
            state.frame_cache.put(decoded_idx, image);
        }

        state.last_requested = Some(frame.0);
        if let Some(image) = resolve_frame(state, frame.0) {
            state.last_resolved = Some(image.clone());
            return Ok(Some(image));
        }

        Ok(state.last_resolved.clone())
    }
}

impl MediaProvider for AssetMediaProvider {
    fn image(&mut self, asset_id: &str) -> Result<Option<Image>, MediaError> {
        if let Some(image) = self.image_cache.get(asset_id) {
            return Ok(Some(image.clone()));
        }

        let image = self.load_image(asset_id)?;
        if let Some(image) = image {
            self.image_cache.put(asset_id.to_string(), image.clone());
            Ok(Some(image))
        } else {
            Ok(None)
        }
    }

    fn video_frame(&mut self, asset_id: &str, frame: FrameIndex) -> Result<Option<Image>, MediaError> {
        self.decode_video_frame(asset_id, frame)
    }
}

struct VideoState {
    #[allow(dead_code)]
    path: PathBuf,
    decoder: VideoDecoder<File>,
    scaler: Option<VideoFrameScaler>,
    scaler_source: Option<(usize, usize, frame::PixelFormat)>,
    frame_cache: LruCache<u64, Image>,
    last_requested: Option<u64>,
    last_resolved: Option<Image>,
}

impl VideoState {
    fn new(path: PathBuf, decoder: VideoDecoder<File>) -> Self {
        Self {
            path,
            decoder,
            scaler: None,
            scaler_source: None,
            frame_cache: LruCache::new(NonZero::new(VIDEO_FRAME_CACHE_SIZE).expect("non-zero")),
            last_requested: None,
            last_resolved: None,
        }
    }
}

fn frame_to_image(state: &mut VideoState, frame: &VideoFrame) -> Result<Image, MediaError> {
    let source_desc = (frame.width(), frame.height(), frame.pixel_format());
    let needs_scaler = state
        .scaler_source
        .map(|current| current != source_desc)
        .unwrap_or(true);

    if needs_scaler {
        state.scaler = Some(
            VideoFrameScaler::builder()
                .source_width(frame.width())
                .source_height(frame.height())
                .source_pixel_format(frame.pixel_format())
                .target_width(frame.width())
                .target_height(frame.height())
                .target_pixel_format(frame::get_pixel_format("rgba"))
                .build()
                .map_err(|err| MediaError::Decode(err.to_string()))?,
        );
        state.scaler_source = Some(source_desc);
    }

    let scaler = state
        .scaler
        .as_mut()
        .ok_or_else(|| MediaError::Decode("missing scaler".to_string()))?;

    let rgba_frame = scaler
        .scale(frame)
        .map_err(|err| MediaError::Decode(err.to_string()))?;

    let width = rgba_frame.width();
    let height = rgba_frame.height();
    let planes = rgba_frame.planes();
    let plane = planes.first().ok_or_else(|| {
        MediaError::Decode("scaled frame did not contain rgba plane".to_string())
    })?;

    let mut rgba = vec![0u8; width * height * 4];
    let line_size = plane.line_size();
    for (line_index, line) in plane.lines().enumerate().take(height) {
        let source = &line[..(width * 4).min(line_size)];
        let dst_start = line_index * width * 4;
        let dst_end = dst_start + source.len();
        rgba[dst_start..dst_end].copy_from_slice(source);
    }

    let image_info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Unpremul,
        None,
    );
    let data = Data::new_copy(&rgba);
    images::raster_from_data(&image_info, data, width * 4).ok_or_else(|| {
        MediaError::Decode("failed to create skia image from decoded frame".to_string())
    })
}

fn frame_to_timestamp(frame_idx: u64, fps: Rational) -> Result<Timestamp, MediaError> {
    if fps.num == 0 {
        return Err(MediaError::Decode("fps numerator must be > 0".to_string()));
    }

    let micros = (frame_idx as u128)
        .saturating_mul(1_000_000u128)
        .saturating_mul(fps.den as u128)
        / fps.num as u128;

    Ok(Timestamp::from_micros(micros.min(i64::MAX as u128) as i64))
}

fn resolve_frame(state: &mut VideoState, requested: u64) -> Option<Image> {
    if let Some(image) = state.frame_cache.get(&requested).cloned() {
        return Some(image);
    }

    let mut best_prev: Option<(u64, Image)> = None;
    let mut best_next: Option<(u64, Image)> = None;

    for (key, image) in state.frame_cache.iter() {
        let idx = *key;
        if idx <= requested {
            if best_prev.as_ref().map(|(best, _)| idx > *best).unwrap_or(true) {
                best_prev = Some((idx, image.clone()));
            }
        } else if best_next
            .as_ref()
            .map(|(best, _)| idx < *best)
            .unwrap_or(true)
        {
            best_next = Some((idx, image.clone()));
        }
    }

    best_prev
        .map(|(_, image)| image)
        .or_else(|| best_next.map(|(_, image)| image))
}

fn timestamp_to_frame(timestamp: Timestamp, fps: Rational) -> u64 {
    let micros = timestamp.as_micros().unwrap_or(0).max(0) as u128;
    let numerator = micros
        .saturating_mul(fps.num as u128)
        .checked_div(1_000_000u128.saturating_mul(fps.den as u128))
        .unwrap_or(0);
    numerator.min(u64::MAX as u128) as u64
}

fn read_asset_bytes(source: &str) -> Result<Vec<u8>, MediaError> {
    let path = resolve_local_path(source)?;
    std::fs::read(&path)
        .map_err(|err| MediaError::Source(format!("failed to read `{}`: {err}", path.display())))
}

fn resolve_local_path(source: &str) -> Result<PathBuf, MediaError> {
    if let Some(path) = source.strip_prefix("file://") {
        return Ok(PathBuf::from(path));
    }

    let path = Path::new(source);
    if path.is_absolute() || path.exists() {
        return Ok(path.to_path_buf());
    }

    Err(MediaError::Source(format!(
        "only local file paths are currently supported: `{source}`"
    )))
}
