use std::{ffi::CStr, ptr};

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
    pub fn as_f64(self) -> Option<f64> {
        (self.denominator != 0).then_some(self.numerator as f64 / self.denominator as f64)
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
    pub width: u32,
    pub height: u32,
    pub pixel_format: PixelFormat,
    pub time_base: Rational,
    pub avg_frame_rate: Rational,
    pub frame_count: Option<u64>,
    pub duration_ts: Option<i64>,
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

pub struct InputContext {
    path: String,
    ptr: *mut sys::AVFormatContext,
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
        let mut ptr = ptr::null_mut();
        unsafe {
            ffi::check(
                sys::avformat_open_input(
                    &mut ptr,
                    c_path.as_ptr(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                ),
                "avformat_open_input",
            )
            .map_err(|error| error.with_path(path.clone()))?;
            ffi::check(
                sys::avformat_find_stream_info(ptr, ptr::null_mut()),
                "avformat_find_stream_info",
            )
            .map_err(|error| error.with_path(path.clone()))?;
        }
        Ok(Self { path, ptr })
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
                let params = (*stream).codecpar;
                match (*params).codec_type {
                    AVMEDIA_TYPE_VIDEO => {
                        video_streams.push(VideoStreamInfo {
                            stream_index: index,
                            codec: VideoCodec::from_av_codec_id((*params).codec_id),
                            width: (*params).width.max(0) as u32,
                            height: (*params).height.max(0) as u32,
                            pixel_format: PixelFormat::from_av_pixel_format(std::mem::transmute::<
                                i32,
                                sys::AVPixelFormat,
                            >(
                                (*params).format
                            )),
                            time_base: (*stream).time_base.into(),
                            avg_frame_rate: (*stream).avg_frame_rate.into(),
                            frame_count: ((*stream).nb_frames > 0)
                                .then_some((*stream).nb_frames as u64),
                            duration_ts: ((*stream).duration > 0).then_some((*stream).duration),
                        });
                    }
                    AVMEDIA_TYPE_AUDIO => {
                        audio_streams.push(AudioStreamInfo {
                            stream_index: index,
                            sample_rate: (*params).sample_rate.max(0) as u32,
                            channels: (*params).ch_layout.nb_channels.max(0) as u16,
                            time_base: (*stream).time_base.into(),
                            frame_count: ((*stream).nb_frames > 0)
                                .then_some((*stream).nb_frames as u64),
                            duration_ts: ((*stream).duration > 0).then_some((*stream).duration),
                        });
                    }
                    _ => {}
                }
            }
            MediaInfo {
                path: self.path.clone(),
                duration_us: ((*self.ptr).duration > 0).then_some((*self.ptr).duration),
                bit_rate: ((*self.ptr).bit_rate > 0).then_some((*self.ptr).bit_rate),
                video_streams,
                audio_streams,
            }
        }
    }

    pub fn best_video_stream(&self) -> Result<VideoStreamInfo> {
        self.media_info()
            .video_streams
            .into_iter()
            .next()
            .ok_or_else(|| crate::FfmpegError::new("best_video_stream", "no video stream found"))
    }

    pub fn best_audio_stream(&self) -> Result<AudioStreamInfo> {
        self.media_info()
            .audio_streams
            .into_iter()
            .next()
            .ok_or_else(|| crate::FfmpegError::new("best_audio_stream", "no audio stream found"))
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
}
