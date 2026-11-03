use std::{
    ffi::{CStr, OsStr},
    path::Path,
    ptr,
};

use crate::{
    PixelFormat, Result, VideoCodec,
    ffi::{self, AvPacket, sys},
};
use sys::AVMediaType::{AVMEDIA_TYPE_AUDIO, AVMEDIA_TYPE_VIDEO};

#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct Rational {
    pub numerator: i32,
    pub denominator: i32,
}

impl Rational {
    pub const fn new(numerator: i32, denominator: i32) -> Self {
        Self {
            numerator,
            denominator,
        }
    }

    /// Converts a floating-point frame rate to an exact rational.
    ///
    /// Whole rates map to `n/1`, NTSC rates (23.976, 29.97, 59.94, ...) map to `n*1000/1001`, and
    /// any other rate is rounded to the nearest 1/1000 and reduced.
    pub fn from_fps(fps: f64) -> Result<Self> {
        const OPERATION: &str = "Rational::from_fps";
        const WHOLE_TOLERANCE: f64 = 1e-6;
        const NTSC_TOLERANCE: f64 = 1e-3;
        if !fps.is_finite() || fps <= 0.0 || fps > 1_000_000.0 {
            return Err(crate::FfmpegError::new(
                OPERATION,
                format!("frame rate {fps} must be a finite value between 0 and 1,000,000"),
            ));
        }
        let whole = fps.round();
        if whole >= 1.0 && (fps - whole).abs() < WHOLE_TOLERANCE {
            return Ok(Self::new(whole as i32, 1));
        }
        let ntsc = (fps * 1.001).round();
        if ntsc >= 1.0 && (fps * 1.001 - ntsc).abs() < NTSC_TOLERANCE {
            return Ok(Self::new(ntsc as i32 * 1000, 1001));
        }
        let millis = (fps * 1000.0).round() as i64;
        if millis == 0 {
            return Err(crate::FfmpegError::new(
                OPERATION,
                format!("frame rate {fps} rounds to zero at millisecond precision"),
            ));
        }
        let divisor = gcd(millis, 1000);
        Ok(Self::new(
            (millis / divisor) as i32,
            (1000 / divisor) as i32,
        ))
    }

    pub fn as_f64(self) -> Option<f64> {
        (self.denominator != 0).then_some(self.numerator as f64 / self.denominator as f64)
    }
}

fn gcd(mut a: i64, mut b: i64) -> i64 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

impl From<u32> for Rational {
    /// Interprets the value as a whole number of frames per second.
    fn from(value: u32) -> Self {
        Self::new(i32::try_from(value).unwrap_or(i32::MAX), 1)
    }
}

impl From<sys::AVRational> for Rational {
    fn from(value: sys::AVRational) -> Self {
        Self {
            numerator: value.num,
            denominator: value.den,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MediaInfo {
    pub path: String,
    pub duration_us: Option<i64>,
    pub bit_rate: Option<i64>,
    pub video_streams: Vec<VideoStreamInfo>,
    pub audio_streams: Vec<AudioStreamInfo>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoStreamInfo {
    pub stream_index: usize,
    pub codec: VideoCodec,
    /// Display width, with `rotation` applied.
    pub width: u32,
    /// Display height, with `rotation` applied.
    pub height: u32,
    /// Clockwise rotation in degrees (0, 90, 180 or 270) that RGBA decode applies to each frame.
    pub rotation: u32,
    pub pixel_format: PixelFormat,
    /// Whether the stream carries alpha, in its pixel format or in a VP8 or VP9 alpha layer.
    pub has_alpha: bool,
    pub time_base: Rational,
    pub avg_frame_rate: Rational,
    /// `avg_frame_rate`, or FFmpeg's guess when the stream has no average.
    pub frame_rate: Option<Rational>,
    pub frame_count: Option<u64>,
    pub duration_ts: Option<i64>,
    /// First presentation timestamp in `time_base` units, or 0 when the stream doesn't report one.
    pub start_time: i64,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioStreamInfo {
    pub stream_index: usize,
    pub sample_rate: u32,
    pub channels: u16,
    pub time_base: Rational,
    pub frame_count: Option<u64>,
    pub duration_ts: Option<i64>,
}

pub struct Packet {
    pub(crate) inner: AvPacket,
}

impl Packet {
    pub fn stream_index(&self) -> usize {
        self.inner.stream_index()
    }

    pub fn pts(&self) -> Option<i64> {
        self.inner.pts()
    }
}

/// Demuxers media can be read with. Everything here keeps a file's data inside that file. Demuxers
/// that open other files or hosts named by the input (`hls`, `dash`, `concat`, `image2`, `imf`,
/// `mlv`, `vobsub`, `rtsp`, `sap`, `sdp`) must stay out. `mov` data references are also external,
/// but FFmpeg only follows them with `enable_drefs`, which is never set. `InputContext::open` makes
/// one exception for `image2`, in `input_formats`.
pub const ALLOWED_INPUT_FORMATS: &str = "mov,matroska,mpegts,mpeg,avi,flv,asf,ogg,mxf,ivf,h264,hevc,\
gif,apng,png_pipe,jpeg_pipe,webp_pipe,bmp_pipe,tiff_pipe,\
mp3,aac,wav,flac,aiff,caf,ac3,eac3,dts,amr,w64,wv";

/// Protocols media can be read through.
pub const ALLOWED_INPUT_PROTOCOLS: &str = "file";

/// Extensions read with `image2`, because FFmpeg's probe picks it over `jpeg_pipe` and `bmp_pipe` for
/// JPEGs past 2 KB and for BMPs with non-zero reserved bytes. Other extensions stay off it because
/// `.y` makes it open `.U` and `.V` siblings, and `.svg` and `.raw` pick decoders by name alone.
const IMAGE2_EXTENSIONS: [&str; 3] = ["jpg", "jpeg", "bmp"];

/// Codecs `image2` may read. It picks one by probing the file's bytes, without consulting
/// `format_whitelist`, so a renamed file could otherwise reach any image decoder FFmpeg has.
const IMAGE2_CODECS: [sys::AVCodecID; 2] = [
    sys::AVCodecID::AV_CODEC_ID_MJPEG,
    sys::AVCodecID::AV_CODEC_ID_BMP,
];

const UNSUPPORTED_INPUT: &str = "container format or protocol is not supported";

/// The demuxers `path` can be opened with. `image2` is only safe with `pattern_type=none`, which makes
/// it read `path` itself instead of expanding `%d` and globs. Given a name with no file behind it,
/// that option leaves an empty stream that opens successfully, so `image2` also needs a real file.
fn input_formats(path: &str) -> String {
    let path = Path::new(path);
    let is_image2_still = path
        .extension()
        .and_then(OsStr::to_str)
        .is_some_and(|extension| {
            IMAGE2_EXTENSIONS
                .iter()
                .any(|known| extension.eq_ignore_ascii_case(known))
        });
    if is_image2_still && path.is_file() {
        format!("{ALLOWED_INPUT_FORMATS},image2")
    } else {
        ALLOWED_INPUT_FORMATS.to_string()
    }
}

/// Whether an opened input is `image2` holding a codec outside `IMAGE2_CODECS`.
unsafe fn is_refused_image2(input: *const sys::AVFormatContext) -> bool {
    unsafe {
        if CStr::from_ptr((*(*input).iformat).name).to_bytes() != b"image2" {
            return false;
        }
        (0..(*input).nb_streams as usize).any(|index| {
            let params = (**(*input).streams.add(index)).codecpar;
            !IMAGE2_CODECS.contains(&(*params).codec_id)
        })
    }
}

pub struct InputContext {
    path: String,
    pub(crate) ptr: *mut sys::AVFormatContext,
}

unsafe impl Send for InputContext {}

impl std::fmt::Debug for InputContext {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("InputContext")
            .field("path", &self.path)
            .finish_non_exhaustive()
    }
}

impl InputContext {
    pub fn open(path: impl Into<String>) -> Result<Self> {
        ffi::init();
        let path = path.into();
        let c_path = ffi::cstring("avformat_open_input", &path)?;
        let formats = ffi::cstring("av_dict_set", &input_formats(&path))?;
        let protocols = ffi::cstring("av_dict_set", ALLOWED_INPUT_PROTOCOLS)?;
        let mut options = ptr::null_mut();
        let mut ptr = ptr::null_mut();
        unsafe {
            let opened = [
                (c"format_whitelist", formats.as_c_str()),
                (c"protocol_whitelist", protocols.as_c_str()),
                (c"pattern_type", c"none"),
            ]
            .into_iter()
            .try_for_each(|(key, value)| {
                ffi::check(
                    sys::av_dict_set(&mut options, key.as_ptr(), value.as_ptr(), 0),
                    "av_dict_set",
                )
            })
            .and_then(|()| {
                ffi::check(
                    sys::avformat_open_input(
                        &mut ptr,
                        c_path.as_ptr(),
                        ptr::null_mut(),
                        &mut options,
                    ),
                    "avformat_open_input",
                )
            });
            sys::av_dict_free(&mut options);
            opened.map_err(|mut error| {
                // FFmpeg reports a format or protocol outside the allowlists as EINVAL.
                if error.code == Some(-libc::EINVAL) {
                    error.message = UNSUPPORTED_INPUT.to_string();
                }
                error.with_path(path.clone())
            })?;
            if is_refused_image2(ptr) {
                sys::avformat_close_input(&mut ptr);
                return Err(
                    crate::FfmpegError::new("avformat_open_input", UNSUPPORTED_INPUT)
                        .with_path(path),
                );
            }
        }
        // Owning the context from here closes it if reading the stream info fails.
        let input = Self { path, ptr };
        unsafe {
            ffi::check(
                sys::avformat_find_stream_info(input.ptr, ptr::null_mut()),
                "avformat_find_stream_info",
            )
            .map_err(|error| error.with_path(input.path.clone()))?;
        }
        Ok(input)
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub fn media_info(&self) -> MediaInfo {
        let mut video_streams = Vec::new();
        let mut audio_streams = Vec::new();
        unsafe {
            for index in 0..(*self.ptr).nb_streams as usize {
                let stream = *(*self.ptr).streams.add(index);
                match (*(*stream).codecpar).codec_type {
                    AVMEDIA_TYPE_VIDEO => video_streams.push(self.video_stream_info(index)),
                    AVMEDIA_TYPE_AUDIO => audio_streams.push(self.audio_stream_info(index)),
                    _ => {}
                }
            }
        }
        MediaInfo {
            path: self.path.clone(),
            duration_us: self.duration_us(),
            bit_rate: unsafe { ((*self.ptr).bit_rate > 0).then_some((*self.ptr).bit_rate) },
            video_streams,
            audio_streams,
        }
    }

    pub fn duration_us(&self) -> Option<i64> {
        let duration = unsafe { (*self.ptr).duration };
        (duration > 0).then_some(duration)
    }

    /// Container duration measured from the first presentation timestamp. Matroska stores its end
    /// time as the duration, so the start offset is removed there; other formats already exclude it.
    pub fn presented_duration_us(&self) -> Option<i64> {
        let duration = self.duration_us()?;
        let start = unsafe {
            let start = (*self.ptr).start_time;
            let format_name = CStr::from_ptr((*(*self.ptr).iformat).name).to_string_lossy();
            if start == sys::AV_NOPTS_VALUE || !format_name.contains("matroska") {
                0
            } else {
                start.max(0)
            }
        };
        (duration > start).then_some(duration - start)
    }

    /// Whether an image demuxer read this input. They report no frame count or duration.
    pub fn is_image(&self) -> bool {
        let name = unsafe { CStr::from_ptr((*(*self.ptr).iformat).name) }.to_string_lossy();
        name.ends_with("_pipe") || name == "apng"
    }

    pub fn best_video_stream(&self) -> Result<VideoStreamInfo> {
        self.best_stream_index(AVMEDIA_TYPE_VIDEO)
            .map(|index| self.video_stream_info(index))
            .ok_or_else(|| crate::FfmpegError::new("best_video_stream", "no video stream found"))
    }

    pub fn best_audio_stream(&self) -> Result<AudioStreamInfo> {
        self.best_stream_index(AVMEDIA_TYPE_AUDIO)
            .map(|index| self.audio_stream_info(index))
            .ok_or_else(|| crate::FfmpegError::new("best_audio_stream", "no audio stream found"))
    }

    fn best_stream_index(&self, media_type: sys::AVMediaType) -> Option<usize> {
        let index =
            unsafe { sys::av_find_best_stream(self.ptr, media_type, -1, -1, ptr::null_mut(), 0) };
        usize::try_from(index).ok()
    }

    fn video_stream_info(&self, index: usize) -> VideoStreamInfo {
        unsafe {
            let stream = *(*self.ptr).streams.add(index);
            let params = (*stream).codecpar;
            let rotation = display_rotation(params);
            let (coded_width, coded_height) = (
                (*params).width.max(0) as u32,
                (*params).height.max(0) as u32,
            );
            let (width, height) = if rotation.is_multiple_of(180) {
                (coded_width, coded_height)
            } else {
                (coded_height, coded_width)
            };
            let format = std::mem::transmute::<i32, sys::AVPixelFormat>((*params).format);
            let descriptor = sys::av_pix_fmt_desc_get(format);
            VideoStreamInfo {
                stream_index: index,
                codec: VideoCodec::from_av_codec_id((*params).codec_id),
                width,
                height,
                rotation,
                pixel_format: PixelFormat::from_av_pixel_format(format),
                has_alpha: self.has_alpha_layer(index)
                    || (!descriptor.is_null()
                        && (*descriptor).flags & sys::AV_PIX_FMT_FLAG_ALPHA as u64 != 0),
                time_base: (*stream).time_base.into(),
                avg_frame_rate: (*stream).avg_frame_rate.into(),
                frame_rate: stream_frame_rate(self.ptr, stream),
                frame_count: ((*stream).nb_frames > 0).then_some((*stream).nb_frames as u64),
                duration_ts: ((*stream).duration > 0).then_some((*stream).duration),
                start_time: if (*stream).start_time == sys::AV_NOPTS_VALUE {
                    0
                } else {
                    (*stream).start_time
                },
            }
        }
    }

    fn audio_stream_info(&self, index: usize) -> AudioStreamInfo {
        unsafe {
            let stream = *(*self.ptr).streams.add(index);
            let params = (*stream).codecpar;
            AudioStreamInfo {
                stream_index: index,
                sample_rate: (*params).sample_rate.max(0) as u32,
                channels: (*params).ch_layout.nb_channels.max(0) as u16,
                time_base: (*stream).time_base.into(),
                frame_count: ((*stream).nb_frames > 0).then_some((*stream).nb_frames as u64),
                duration_ts: ((*stream).duration > 0).then_some((*stream).duration),
            }
        }
    }

    pub fn read_packet(&mut self) -> Result<Option<Packet>> {
        let mut packet = AvPacket::new()?;
        let result = unsafe { sys::av_read_frame(self.ptr, packet.as_mut_ptr()) };
        if result == sys::AVERROR_EOF {
            return Ok(None);
        }
        if result < 0 {
            return Err(ffi::error_from_code("av_read_frame", result).with_path(self.path.clone()));
        }
        Ok(Some(Packet { inner: packet }))
    }

    pub fn seek(&mut self, timestamp: i64) -> Result<()> {
        unsafe {
            ffi::check(
                sys::av_seek_frame(self.ptr, -1, timestamp, sys::AVSEEK_FLAG_BACKWARD),
                "av_seek_frame",
            )
            .map_err(|error| error.with_path(self.path.clone()))
        }
    }

    pub fn seek_stream(&mut self, stream_index: usize, timestamp: i64) -> Result<()> {
        unsafe {
            if stream_index >= (*self.ptr).nb_streams as usize {
                return Err(crate::FfmpegError::new(
                    "av_seek_frame",
                    "stream index out of range",
                ));
            }
            ffi::check(
                sys::av_seek_frame(
                    self.ptr,
                    stream_index as i32,
                    timestamp,
                    sys::AVSEEK_FLAG_BACKWARD,
                ),
                "av_seek_frame",
            )
            .map_err(|error| error.with_path(self.path.clone()))
        }
    }

    /// Whether the stream has a separate VP8 or VP9 alpha layer. Its pixel format doesn't show one,
    /// so this reads the tag the Matroska demuxer sets from the track header.
    pub(crate) fn has_alpha_layer(&self, stream_index: usize) -> bool {
        unsafe {
            if stream_index >= (*self.ptr).nb_streams as usize {
                return false;
            }
            let stream = *(*self.ptr).streams.add(stream_index);
            let entry =
                sys::av_dict_get((*stream).metadata, c"alpha_mode".as_ptr(), ptr::null(), 0);
            !entry.is_null() && CStr::from_ptr((*entry).value).to_bytes() == b"1"
        }
    }

    pub(crate) fn stream_parameters(
        &self,
        stream_index: usize,
    ) -> Result<*const sys::AVCodecParameters> {
        unsafe {
            if stream_index >= (*self.ptr).nb_streams as usize {
                return Err(crate::FfmpegError::new(
                    "stream_parameters",
                    "stream index out of range",
                ));
            }
            Ok((**(*self.ptr).streams.add(stream_index)).codecpar)
        }
    }

    pub(crate) fn stream_time_base(&self, stream_index: usize) -> Result<sys::AVRational> {
        unsafe {
            if stream_index >= (*self.ptr).nb_streams as usize {
                return Err(crate::FfmpegError::new(
                    "stream_time_base",
                    "stream index out of range",
                ));
            }
            Ok((**(*self.ptr).streams.add(stream_index)).time_base)
        }
    }
}

impl Drop for InputContext {
    fn drop(&mut self) {
        unsafe {
            sys::avformat_close_input(&mut self.ptr);
        }
    }
}

/// Prefers the measured average: on variable frame rate files FFmpeg's guess can be a peak rate that
/// disagrees with the stream's frame count.
unsafe fn stream_frame_rate(
    context: *mut sys::AVFormatContext,
    stream: *mut sys::AVStream,
) -> Option<Rational> {
    let average = unsafe { (*stream).avg_frame_rate };
    if average.num > 0 && average.den > 0 {
        return Some(average.into());
    }
    let guess = unsafe { sys::av_guess_frame_rate(context, stream, ptr::null_mut()) };
    let time_base = unsafe { (*stream).time_base };
    // With nothing better to go on, FFmpeg reports the inverted time base, which is not a frame rate.
    let inverted_time_base = guess.num == time_base.den && guess.den == time_base.num;
    (guess.num > 0 && guess.den > 0 && !inverted_time_base).then(|| guess.into())
}

/// Reads the clockwise quarter turn a stream's display matrix asks for. Any other matrix reads as 0.
pub(crate) fn display_rotation(params: *const sys::AVCodecParameters) -> u32 {
    const DISPLAY_MATRIX_BYTES: usize = 9 * size_of::<i32>();
    unsafe {
        let side_data = sys::av_packet_side_data_get(
            (*params).coded_side_data,
            (*params).nb_coded_side_data,
            sys::AVPacketSideDataType::AV_PKT_DATA_DISPLAYMATRIX,
        );
        if side_data.is_null() || (*side_data).size < DISPLAY_MATRIX_BYTES {
            return 0;
        }
        // Returns counterclockwise degrees, or NaN for a singular matrix, which casts to 0.
        let counterclockwise = sys::av_display_rotation_get((*side_data).data.cast::<i32>());
        let clockwise = (-counterclockwise.round() as i32).rem_euclid(360) as u32;
        if clockwise.is_multiple_of(90) {
            clockwise
        } else {
            0
        }
    }
}

pub(crate) fn codec_name(codec_id: sys::AVCodecID) -> String {
    unsafe {
        let name = sys::avcodec_get_name(codec_id);
        if name.is_null() {
            "unknown".to_string()
        } else {
            CStr::from_ptr(name).to_string_lossy().into_owned()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rational_handles_zero_denominator() {
        assert_eq!(
            Rational {
                numerator: 1,
                denominator: 0
            }
            .as_f64(),
            None
        );
        assert_eq!(
            Rational {
                numerator: 1,
                denominator: 2
            }
            .as_f64(),
            Some(0.5)
        );
    }

    #[test]
    fn maps_common_frame_rates_exactly() {
        assert_eq!(Rational::from_fps(30.0).unwrap(), Rational::new(30, 1));
        assert_eq!(
            Rational::from_fps(23.976).unwrap(),
            Rational::new(24_000, 1001)
        );
        assert_eq!(
            Rational::from_fps(f64::from(29.97_f32)).unwrap(),
            Rational::new(30_000, 1001)
        );
        assert_eq!(
            Rational::from_fps(59.94).unwrap(),
            Rational::new(60_000, 1001)
        );
        assert_eq!(Rational::from_fps(12.5).unwrap(), Rational::new(25, 2));
        assert_eq!(
            Rational::from_fps(24.0009).unwrap(),
            Rational::new(24_001, 1000)
        );
        assert_eq!(Rational::from_fps(0.0009).unwrap(), Rational::new(1, 1000));
    }

    #[test]
    fn rejects_invalid_frame_rates() {
        for fps in [0.0, 0.0004, -1.0, f64::NAN, f64::INFINITY] {
            assert!(Rational::from_fps(fps).is_err());
        }
    }
}
