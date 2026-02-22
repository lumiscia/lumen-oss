pub mod worker;

use std::{
    collections::HashMap,
    env,
    ffi::CString,
    hash::Hash,
    path::{Path, PathBuf},
    ptr,
    sync::{Arc, OnceLock},
};

use ffmpeg_next::{self as ffmpeg, format, media, software::scaling};
use thiserror::Error;

use crate::{render::backend::FrameImage, time::Rational};

const DEFAULT_CACHE_FRAMES: usize = 64;

#[cfg(target_os = "macos")]
const AUTO_HW_DEVICE_CANDIDATES: &[&str] = &["videotoolbox"];
#[cfg(target_os = "linux")]
const AUTO_HW_DEVICE_CANDIDATES: &[&str] = &["vaapi", "cuda", "qsv", "vulkan"];
#[cfg(target_os = "windows")]
const AUTO_HW_DEVICE_CANDIDATES: &[&str] = &["d3d11va", "dxva2", "qsv"];
#[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "windows")))]
const AUTO_HW_DEVICE_CANDIDATES: &[&str] = &[];

static FFMPEG_INIT: OnceLock<Result<(), String>> = OnceLock::new();

#[derive(Debug, Error, Clone)]
pub enum FfmpegError {
    #[error("ffmpeg initialization failed: {0}")]
    Init(String),
    #[error("failed to open input `{path}`: {reason}")]
    OpenInput { path: String, reason: String },
    #[error("source `{0}` does not contain a video stream")]
    MissingVideoStream(String),
    #[error("invalid frame rate {num}/{den}")]
    InvalidFrameRate { num: u32, den: u32 },
    #[error("invalid cache size: {0}")]
    InvalidCacheSize(usize),
    #[error("unsupported video dimensions {width}x{height}")]
    InvalidDimensions { width: u32, height: u32 },
    #[error("video frame byte size overflow")]
    FrameByteSizeOverflow,
    #[error("failed to create decoder: {0}")]
    DecoderOpen(String),
    #[error("failed to create scaler: {0}")]
    ScalerInit(String),
    #[error("failed to read packet: {0}")]
    PacketRead(String),
    #[error("failed to submit packet: {0}")]
    SendPacket(String),
    #[error("failed to flush decoder: {0}")]
    SendEof(String),
    #[error("decode failed: {0}")]
    Decode(String),
    #[error("swscale conversion failed: {0}")]
    Convert(String),
    #[error("seek failed: {0}")]
    Seek(String),
    #[error("worker for `{0}` is unavailable")]
    WorkerUnavailable(String),
    #[error("worker for `{0}` dropped decode response")]
    WorkerResponseDropped(String),
    #[error("encode queue closed")]
    EncodeChannelClosed,
    #[error("encode thread panicked")]
    EncodeThreadPanic,
}

pub(crate) fn ensure_ffmpeg_init() -> Result<(), FfmpegError> {
    let init = FFMPEG_INIT.get_or_init(|| {
        ffmpeg::init()
            .map_err(|err| format!("ffmpeg init failed: {err}"))
            .map(|_| {
                ffmpeg::log::set_level(ffmpeg::log::Level::Error);
            })
    });

    match init {
        Ok(()) => Ok(()),
        Err(err) => Err(FfmpegError::Init(err.clone())),
    }
}

#[derive(Debug)]
struct LruCache<K, V> {
    capacity: usize,
    clock: u64,
    entries: HashMap<K, (V, u64)>,
}

impl<K, V> LruCache<K, V>
where
    K: Eq + Hash + Clone,
{
    fn new(capacity: usize) -> Self {
        Self {
            capacity,
            clock: 0,
            entries: HashMap::new(),
        }
    }

    fn get(&mut self, key: &K) -> Option<&V> {
        if let Some((value, stamp)) = self.entries.get_mut(key) {
            self.clock = self.clock.saturating_add(1);
            *stamp = self.clock;
            return Some(value);
        }
        None
    }

    fn put(&mut self, key: K, value: V) -> Option<V> {
        self.clock = self.clock.saturating_add(1);
        if let Some((existing, stamp)) = self.entries.get_mut(&key) {
            *stamp = self.clock;
            let old = std::mem::replace(existing, value);
            return Some(old);
        }

        let evicted = if self.entries.len() >= self.capacity {
            self.pop_lru()
        } else {
            None
        };

        self.entries.insert(key, (value, self.clock));
        evicted.map(|(_, value)| value)
    }

    fn pop_lru(&mut self) -> Option<(K, V)> {
        let lru_key = self
            .entries
            .iter()
            .min_by_key(|(_, (_, stamp))| *stamp)
            .map(|(key, _)| key.clone())?;
        let (value, _) = self.entries.remove(&lru_key)?;
        Some((lru_key, value))
    }

    fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(key, (value, _))| (key, value))
    }
}

pub struct LibavStreamDecoder {
    input_ctx: format::context::Input,
    video_stream_index: usize,
    decoder: ffmpeg::codec::decoder::Video,
    scaler: scaling::Context,
    width: u32,
    height: u32,
    frame_byte_size: usize,
    time_base: ffmpeg::Rational,
    timeline_time_base: ffmpeg::Rational,
    next_source_frame: u64,
    cache: LruCache<u64, FrameImage>,
    buffer_pool: Vec<Vec<u8>>,
    decoded_frame: ffmpeg::frame::Video,
    scratch_frame: ffmpeg::frame::Video,
    packet: ffmpeg::Packet,
    eof: bool,
    draining: bool,
    last_decoded_source_frame: Option<u64>,
    last_decoded_image: Option<FrameImage>,
    source_path: PathBuf,
}

/// Safe: decoder is owned exclusively by its worker thread and accessed only via the bounded
/// channel.
unsafe impl Send for LibavStreamDecoder {}

fn source_frame_to_pts_raw(
    frame: u64,
    time_base: ffmpeg::Rational,
    timeline_time_base: ffmpeg::Rational,
) -> i64 {
    let timestamp_secs = frame as f64 * timeline_time_base.0 as f64 / timeline_time_base.1 as f64;
    let pts = timestamp_secs * time_base.1 as f64 / time_base.0 as f64;
    pts.round() as i64
}

fn pts_to_source_frame_raw(
    pts: i64,
    time_base: ffmpeg::Rational,
    timeline_time_base: ffmpeg::Rational,
) -> u64 {
    let timestamp_secs = pts as f64 * time_base.0 as f64 / time_base.1 as f64;
    let frame = timestamp_secs * timeline_time_base.1 as f64 / timeline_time_base.0 as f64;
    frame.round().max(0.0) as u64
}

impl LibavStreamDecoder {
    pub fn new(
        path: impl AsRef<Path>,
        fps: Rational,
        cache_frames: usize,
    ) -> Result<Self, FfmpegError> {
        ensure_ffmpeg_init()?;

        if fps.num == 0 || fps.den == 0 {
            return Err(FfmpegError::InvalidFrameRate {
                num: fps.num,
                den: fps.den,
            });
        }

        let source_path = path.as_ref().to_path_buf();
        let source_path_str = source_path.to_string_lossy().into_owned();
        let input_ctx = format::input(&source_path).map_err(|err| FfmpegError::OpenInput {
            path: source_path_str.clone(),
            reason: err.to_string(),
        })?;

        let stream = input_ctx
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| FfmpegError::MissingVideoStream(source_path_str.clone()))?;
        let video_stream_index = stream.index();
        let time_base = stream.time_base();
        let timeline_time_base = ffmpeg::Rational::new(
            i32::try_from(fps.den).map_err(|_| FfmpegError::InvalidFrameRate {
                num: fps.num,
                den: fps.den,
            })?,
            i32::try_from(fps.num).map_err(|_| FfmpegError::InvalidFrameRate {
                num: fps.num,
                den: fps.den,
            })?,
        );

        let mut decoder_ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|err| FfmpegError::DecoderOpen(err.to_string()))?;
        let mut threading = ffmpeg::codec::threading::Config::default();
        threading.kind = ffmpeg::codec::threading::Type::Frame;
        threading.count = 0;
        decoder_ctx.set_threading(threading);
        configure_hw_decode_if_requested(&mut decoder_ctx);
        let decoder = decoder_ctx
            .decoder()
            .video()
            .map_err(|err| FfmpegError::DecoderOpen(err.to_string()))?;

        let width = decoder.width();
        let height = decoder.height();
        if width == 0 || height == 0 {
            return Err(FfmpegError::InvalidDimensions { width, height });
        }
        let frame_byte_size = (width as usize)
            .checked_mul(height as usize)
            .and_then(|value| value.checked_mul(4))
            .ok_or(FfmpegError::FrameByteSizeOverflow)?;

        let scaler = scaling::Context::get(
            decoder.format(),
            width,
            height,
            ffmpeg::format::Pixel::RGBA,
            width,
            height,
            scaling::Flags::FAST_BILINEAR,
        )
        .map_err(|err| FfmpegError::ScalerInit(err.to_string()))?;

        let cache_capacity = if cache_frames == 0 {
            DEFAULT_CACHE_FRAMES
        } else {
            cache_frames
        };

        Ok(Self {
            input_ctx,
            video_stream_index,
            decoder,
            scaler,
            width,
            height,
            frame_byte_size,
            time_base,
            timeline_time_base,
            next_source_frame: 0,
            cache: LruCache::new(cache_capacity),
            buffer_pool: Vec::new(),
            decoded_frame: ffmpeg::frame::Video::empty(),
            scratch_frame: ffmpeg::frame::Video::empty(),
            packet: ffmpeg::Packet::empty(),
            eof: false,
            draining: false,
            last_decoded_source_frame: None,
            last_decoded_image: None,
            source_path,
        })
    }

    fn source_frame_to_pts(&self, frame: u64) -> i64 {
        source_frame_to_pts_raw(frame, self.time_base, self.timeline_time_base)
    }

    fn pts_to_source_frame(&self, pts: i64) -> u64 {
        pts_to_source_frame_raw(pts, self.time_base, self.timeline_time_base)
    }

    pub fn get_frame(&mut self, target: u64) -> Result<Option<FrameImage>, FfmpegError> {
        if let Some(frame) = self.cache.get(&target) {
            return Ok(Some(frame.clone()));
        }

        if target < self.next_source_frame {
            self.seek_or_reopen(target)?;
        }

        while self.next_source_frame <= target && !self.eof {
            match self.decode_next_frame()? {
                Some((idx, image)) => {
                    self.cache_decoded_frame(idx, image);
                    if idx >= target {
                        break;
                    }
                }
                None => break,
            }
        }

        if let Some(frame) = self.cache.get(&target) {
            return Ok(Some(frame.clone()));
        }

        Ok(self.nearest_cached_frame(target))
    }

    fn cache_decoded_frame(&mut self, frame_idx: u64, image: FrameImage) {
        let held_image = self.last_decoded_image.clone();
        if let (Some(last_idx), Some(last_image)) = (self.last_decoded_source_frame, held_image) {
            if frame_idx > last_idx.saturating_add(1) {
                for gap_idx in last_idx.saturating_add(1)..frame_idx {
                    self.cache_frame(gap_idx, last_image.clone());
                }
            }
        }

        self.cache_frame(frame_idx, image.clone());
        self.last_decoded_source_frame = Some(frame_idx);
        self.last_decoded_image = Some(image);
    }

    fn cache_frame(&mut self, frame_idx: u64, image: FrameImage) {
        if let Some(evicted) = self.cache.put(frame_idx, image) {
            self.recycle_buffer(evicted);
        }
    }

    fn recycle_buffer(&mut self, frame: FrameImage) {
        let Ok(mut pixels) = Arc::try_unwrap(frame.pixels_rgba) else {
            return;
        };
        if pixels.capacity() != self.frame_byte_size {
            return;
        }
        pixels.clear();
        self.buffer_pool.push(pixels);
    }

    fn take_buffer(&mut self) -> Vec<u8> {
        if let Some(mut pooled) = self.buffer_pool.pop() {
            pooled.clear();
            pooled
        } else {
            Vec::with_capacity(self.frame_byte_size)
        }
    }

    fn nearest_cached_frame(&self, target: u64) -> Option<FrameImage> {
        let mut prev: Option<(u64, &FrameImage)> = None;
        let mut next: Option<(u64, &FrameImage)> = None;

        for (idx, image) in self.cache.iter() {
            let idx = *idx;
            if idx <= target {
                if prev.as_ref().is_none_or(|(best, _)| idx > *best) {
                    prev = Some((idx, image));
                }
            } else if next.as_ref().is_none_or(|(best, _)| idx < *best) {
                next = Some((idx, image));
            }
        }

        prev.map(|(_, image)| image.clone())
            .or_else(|| next.map(|(_, image)| image.clone()))
    }

    fn seek_or_reopen(&mut self, target_frame: u64) -> Result<(), FfmpegError> {
        let target_pts = self.source_frame_to_pts(target_frame);
        if self.input_ctx.seek(target_pts, ..target_pts).is_ok() {
            self.decoder.flush();
            self.eof = false;
            self.draining = false;
            self.next_source_frame = 0;
            self.last_decoded_source_frame = None;
            self.last_decoded_image = None;
            return Ok(());
        }

        self.reopen_and_skip(target_frame)
    }

    fn reopen_and_skip(&mut self, target_frame: u64) -> Result<(), FfmpegError> {
        let source_path = self.source_path.to_string_lossy().into_owned();
        let input_ctx = format::input(&self.source_path).map_err(|err| FfmpegError::OpenInput {
            path: source_path.clone(),
            reason: err.to_string(),
        })?;

        let stream = input_ctx
            .streams()
            .best(media::Type::Video)
            .ok_or_else(|| FfmpegError::MissingVideoStream(source_path.clone()))?;
        let video_stream_index = stream.index();
        let time_base = stream.time_base();

        let mut decoder_ctx = ffmpeg::codec::context::Context::from_parameters(stream.parameters())
            .map_err(|err| FfmpegError::DecoderOpen(err.to_string()))?;
        let mut threading = ffmpeg::codec::threading::Config::default();
        threading.kind = ffmpeg::codec::threading::Type::Frame;
        threading.count = 0;
        decoder_ctx.set_threading(threading);
        configure_hw_decode_if_requested(&mut decoder_ctx);
        let decoder = decoder_ctx
            .decoder()
            .video()
            .map_err(|err| FfmpegError::DecoderOpen(err.to_string()))?;

        self.input_ctx = input_ctx;
        self.video_stream_index = video_stream_index;
        self.time_base = time_base;
        self.decoder = decoder;
        self.eof = false;
        self.draining = false;
        self.next_source_frame = 0;
        self.last_decoded_source_frame = None;
        self.last_decoded_image = None;

        while self.next_source_frame <= target_frame && !self.eof {
            match self.decode_next_frame()? {
                Some((idx, image)) => {
                    self.cache_decoded_frame(idx, image);
                    if idx >= target_frame {
                        break;
                    }
                }
                None => break,
            }
        }

        Ok(())
    }

    fn decode_next_frame(&mut self) -> Result<Option<(u64, FrameImage)>, FfmpegError> {
        let decoded = self.decode_next_raw()?;
        if let Some((frame_idx, _)) = decoded.as_ref() {
            self.next_source_frame = frame_idx.saturating_add(1);
        }
        Ok(decoded)
    }

    fn decode_next_raw(&mut self) -> Result<Option<(u64, FrameImage)>, FfmpegError> {
        loop {
            match self.decoder.receive_frame(&mut self.decoded_frame) {
                Ok(()) => {
                    if let Some(decoded) = self.convert_decoded()? {
                        return Ok(Some(decoded));
                    }
                }
                Err(ffmpeg::Error::Other { errno }) if errno == ffmpeg::error::EAGAIN => {
                    if self.draining {
                        self.eof = true;
                        return Ok(None);
                    }

                    if !self.feed_next_packet()? {
                        self.decoder
                            .send_eof()
                            .map_err(|err| FfmpegError::SendEof(err.to_string()))?;
                        self.draining = true;
                    }
                }
                Err(ffmpeg::Error::Eof) => {
                    self.eof = true;
                    return Ok(None);
                }
                Err(err) => return Err(FfmpegError::Decode(err.to_string())),
            }
        }
    }

    fn feed_next_packet(&mut self) -> Result<bool, FfmpegError> {
        loop {
            match self.packet.read(&mut self.input_ctx) {
                Ok(()) => {
                    if self.packet.stream() != self.video_stream_index {
                        continue;
                    }

                    self.decoder
                        .send_packet(&self.packet)
                        .map_err(|err| FfmpegError::SendPacket(err.to_string()))?;
                    return Ok(true);
                }
                Err(ffmpeg::Error::Eof) => return Ok(false),
                Err(err) => return Err(FfmpegError::PacketRead(err.to_string())),
            }
        }
    }

    fn convert_decoded(&mut self) -> Result<Option<(u64, FrameImage)>, FfmpegError> {
        let Some(pts) = self.decoded_frame.pts() else {
            return Ok(None);
        };
        let source_frame = self.pts_to_source_frame(pts);

        self.scaler
            .run(&self.decoded_frame, &mut self.scratch_frame)
            .map_err(|err| FfmpegError::Convert(err.to_string()))?;

        let row_bytes = self.width as usize * 4;
        let mut rgba = self.take_buffer();
        let stride = self.scratch_frame.stride(0);
        let data = self.scratch_frame.data(0);
        if stride == row_bytes {
            rgba.extend_from_slice(&data[..row_bytes * self.height as usize]);
        } else {
            for row in 0..self.height as usize {
                let start = row * stride;
                let end = start + row_bytes;
                rgba.extend_from_slice(&data[start..end]);
            }
        }

        let image = FrameImage {
            width: self.width,
            height: self.height,
            pixels_rgba: Arc::new(rgba),
        };
        Ok(Some((source_frame, image)))
    }
}

fn preferred_hw_device_names() -> Option<Vec<String>> {
    let raw = env::var("LUMEN_LIBAV_HW_DEVICE").ok()?;
    let value = raw.trim();
    if value.is_empty()
        || value.eq_ignore_ascii_case("none")
        || value.eq_ignore_ascii_case("off")
        || value.eq_ignore_ascii_case("disabled")
    {
        return None;
    }

    if value.eq_ignore_ascii_case("auto") {
        if AUTO_HW_DEVICE_CANDIDATES.is_empty() {
            return None;
        }
        return Some(
            AUTO_HW_DEVICE_CANDIDATES
                .iter()
                .map(|name| (*name).to_owned())
                .collect(),
        );
    }

    let explicit = value
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect::<Vec<_>>();
    if explicit.is_empty() {
        None
    } else {
        Some(explicit)
    }
}

fn configure_hw_decode_if_requested(decoder_ctx: &mut ffmpeg::codec::context::Context) {
    let Some(device_names) = preferred_hw_device_names() else {
        return;
    };

    let mut failures = Vec::new();
    for device_name in device_names {
        match try_attach_hw_device(decoder_ctx, &device_name) {
            Ok(()) => return,
            Err(reason) => failures.push(format!("{device_name}: {reason}")),
        }
    }

    eprintln!(
        "libav hardware decode unavailable, continuing with software decode ({})",
        failures.join("; ")
    );
}

fn try_attach_hw_device(
    decoder_ctx: &mut ffmpeg::codec::context::Context,
    device_name: &str,
) -> Result<(), String> {
    unsafe {
        let cname = CString::new(device_name).map_err(|_| "device name contains NUL".to_owned())?;
        let device_type = ffmpeg::ffi::av_hwdevice_find_type_by_name(cname.as_ptr());
        if device_type == ffmpeg::ffi::AVHWDeviceType::AV_HWDEVICE_TYPE_NONE {
            return Err("unknown device type".to_owned());
        }

        let mut device_ctx: *mut ffmpeg::ffi::AVBufferRef = ptr::null_mut();
        let result = ffmpeg::ffi::av_hwdevice_ctx_create(
            &mut device_ctx,
            device_type,
            ptr::null(),
            ptr::null_mut(),
            0,
        );
        if result < 0 || device_ctx.is_null() {
            return Err(format!(
                "device init failed: {}",
                ffmpeg::Error::from(result)
            ));
        }

        let codec_ptr = decoder_ctx.as_mut_ptr();
        if !(*codec_ptr).hw_device_ctx.is_null() {
            ffmpeg::ffi::av_buffer_unref(&mut (*codec_ptr).hw_device_ctx);
        }
        (*codec_ptr).hw_device_ctx = device_ctx;
        (*codec_ptr).extra_hw_frames = 8;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{LibavStreamDecoder, LruCache, pts_to_source_frame_raw, source_frame_to_pts_raw};
    use crate::{render::backend::FrameImage, time::Rational};
    use std::{env, path::PathBuf, sync::Arc};

    fn frame(byte: u8) -> FrameImage {
        FrameImage {
            width: 1,
            height: 1,
            pixels_rgba: Arc::new(vec![byte, byte, byte, 255]),
        }
    }

    #[test]
    fn pts_round_trip_known_frames() {
        let time_base = ffmpeg_next::Rational::new(1, 90_000);
        let timeline = ffmpeg_next::Rational::new(1, 30);
        for frame in [0_u64, 1, 5, 30, 97, 150] {
            let pts = source_frame_to_pts_raw(frame, time_base, timeline);
            assert_eq!(pts_to_source_frame_raw(pts, time_base, timeline), frame);
        }
    }

    #[test]
    fn lru_cache_hit_and_eviction_order() {
        let mut cache = LruCache::new(2);
        assert!(cache.put(1_u64, frame(1)).is_none());
        assert!(cache.put(2_u64, frame(2)).is_none());
        assert!(cache.get(&1).is_some());

        let evicted = cache.put(3_u64, frame(3));
        assert!(evicted.is_some());
        assert!(cache.get(&2).is_none());
        assert!(cache.get(&1).is_some());
        assert!(cache.get(&3).is_some());
    }

    #[test]
    fn gap_fill_equivalent_indices_point_to_same_buffer() {
        let held = frame(7);
        let mut cache = LruCache::new(8);
        cache.put(1, held.clone());
        for idx in 2..5 {
            cache.put(idx, held.clone());
        }

        for idx in 1..5 {
            let got = cache.get(&idx).expect("cached frame");
            assert_eq!(got.pixels_rgba[0], 7);
        }
    }

    #[test]
    fn decode_smoke_from_optional_test_asset() {
        let Ok(path) = env::var("LUMEN_FFMPEG_TEST_VIDEO") else {
            return;
        };
        let mut decoder = LibavStreamDecoder::new(PathBuf::from(path), Rational::new(30, 1), 8)
            .expect("decoder should open test asset");

        let mut forward = Vec::new();
        for frame in 0..4 {
            forward.push(decoder.get_frame(frame).expect("decode sequential frame"));
        }
        assert!(forward.iter().all(Option::is_some));

        let cached = decoder.get_frame(3).expect("decode cached frame");
        assert!(cached.is_some());
        assert!(Arc::ptr_eq(
            &forward[3].as_ref().expect("frame 3").pixels_rgba,
            &cached.as_ref().expect("cached frame 3").pixels_rgba
        ));

        let seek_back = decoder.get_frame(1).expect("seek back to frame 1");
        assert!(seek_back.is_some());
    }
}
