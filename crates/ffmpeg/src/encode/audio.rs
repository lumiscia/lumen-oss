use std::ptr;

use crate::audio::{AudioFrame, SampleFormat};
use crate::ffi::{self, AvFrame, AvPacket, sys};
use crate::{FfmpegError, Result};
use sys::AVMediaType::AVMEDIA_TYPE_AUDIO;
use sys::AVSampleFormat::{AV_SAMPLE_FMT_FLT, AV_SAMPLE_FMT_FLTP};

use super::codec::{find_audio_encoder, select_audio_sample_format};
use super::common::refresh_stream_time_base;
use super::output::OutputContext;

#[derive(Debug, Clone)]
pub struct AudioEncoderConfig {
    pub sample_rate: u32,
    pub channels: u16,
    pub bit_rate: i64,
    pub encoder_name: Option<String>,
}

impl AudioEncoderConfig {
    pub fn aac(sample_rate: u32, channels: u16) -> Self {
        Self {
            sample_rate,
            channels,
            bit_rate: 192_000,
            encoder_name: None,
        }
    }
}

pub struct AudioEncoder {
    stream_index: usize,
    stream_time_base: sys::AVRational,
    context: *mut sys::AVCodecContext,
    sample_format: sys::AVSampleFormat,
    sample_rate: u32,
    channels: u16,
    frame_size: usize,
    pending: Vec<f32>,
    next_pts: i64,
}

unsafe impl Send for AudioEncoder {}

impl AudioEncoder {
    pub fn create(output: &mut OutputContext, config: AudioEncoderConfig) -> Result<Self> {
        if config.sample_rate == 0 || config.channels == 0 {
            return Err(FfmpegError::new(
                "AudioEncoder::create",
                "sample_rate and channels must be greater than zero",
            ));
        }
        let codec = find_audio_encoder(&config)?;
        let sample_format = select_audio_sample_format(codec)?;
        let stream = unsafe { sys::avformat_new_stream(output.ptr, ptr::null()) };
        if stream.is_null() {
            return Err(FfmpegError::new(
                "avformat_new_stream",
                "failed to allocate audio output stream",
            ));
        }
        let context = unsafe { sys::avcodec_alloc_context3(codec) };
        if context.is_null() {
            return Err(FfmpegError::new(
                "avcodec_alloc_context3",
                "failed to allocate audio encoder context",
            ));
        }
        let time_base = sys::AVRational {
            num: 1,
            den: config.sample_rate as i32,
        };
        unsafe {
            (*context).codec_id = sys::AVCodecID::AV_CODEC_ID_AAC;
            (*context).codec_type = AVMEDIA_TYPE_AUDIO;
            (*context).sample_rate = config.sample_rate as i32;
            (*context).sample_fmt = sample_format;
            (*context).bit_rate = config.bit_rate;
            (*context).time_base = time_base;
            sys::av_channel_layout_default(&mut (*context).ch_layout, config.channels as i32);
            if ((*(*output.ptr).oformat).flags & sys::AVFMT_GLOBALHEADER) != 0 {
                (*context).flags |= sys::AV_CODEC_FLAG_GLOBAL_HEADER as i32;
            }
            ffi::check(
                sys::avcodec_open2(context, codec, ptr::null_mut()),
                "avcodec_open2",
            )?;
            ffi::check(
                sys::avcodec_parameters_from_context((*stream).codecpar, context),
                "avcodec_parameters_from_context",
            )?;
            (*stream).time_base = time_base;
        }
        let frame_size = unsafe { (*context).frame_size.max(0) as usize };
        let frame_size = frame_size.max(1);
        Ok(Self {
            stream_index: unsafe { (*stream).index as usize },
            stream_time_base: time_base,
            context,
            sample_format,
            sample_rate: config.sample_rate,
            channels: config.channels,
            frame_size,
            pending: Vec::with_capacity(frame_size.saturating_mul(config.channels as usize)),
            next_pts: 0,
        })
    }

    pub(in crate::encode) fn refresh_stream_time_base(
        &mut self,
        output: &OutputContext,
    ) -> Result<()> {
        self.stream_time_base = refresh_stream_time_base(
            output,
            self.stream_index,
            "AudioEncoder::refresh_stream_time_base",
        )?;
        Ok(())
    }

    pub(in crate::encode) fn send_audio_frame(
        &mut self,
        output: &mut OutputContext,
        frame: &AudioFrame,
    ) -> Result<()> {
        self.validate_audio_frame(frame)?;
        self.pending.extend_from_slice(&frame.interleaved_f32);
        let samples_per_packet = self.frame_size.saturating_mul(self.channels as usize);
        while self.pending.len() >= samples_per_packet {
            let chunk: Vec<f32> = self.pending.drain(..samples_per_packet).collect();
            self.send_samples(output, &chunk, self.frame_size)?;
        }
        Ok(())
    }

    fn validate_audio_frame(&self, frame: &AudioFrame) -> Result<()> {
        if frame.sample_rate != self.sample_rate
            || frame.channels != self.channels
            || frame.sample_format != SampleFormat::F32
        {
            return Err(FfmpegError::new(
                "AudioEncoder::send_audio_frame",
                format!(
                    "expected {} Hz, {} channel interleaved f32 audio; got {} Hz, {} channel {:?}",
                    self.sample_rate,
                    self.channels,
                    frame.sample_rate,
                    frame.channels,
                    frame.sample_format
                ),
            ));
        }
        let channels = self.channels as usize;
        if frame.interleaved_f32.len() != frame.samples.saturating_mul(channels) {
            return Err(FfmpegError::new(
                "AudioEncoder::send_audio_frame",
                "audio sample count does not match interleaved buffer length",
            ));
        }
        Ok(())
    }

    pub(in crate::encode) fn flush(&mut self, output: &mut OutputContext) -> Result<()> {
        if !self.pending.is_empty() {
            let channels = self.channels as usize;
            let samples = self.pending.len().div_ceil(channels);
            let mut chunk = std::mem::take(&mut self.pending);
            chunk.resize(samples.saturating_mul(channels), 0.0);
            self.send_samples(output, &chunk, samples)?;
        }
        self.send_frame(output, ptr::null())
    }

    fn send_samples(
        &mut self,
        output: &mut OutputContext,
        samples: &[f32],
        sample_count: usize,
    ) -> Result<()> {
        let mut frame = AvFrame::new()?;
        unsafe {
            (*frame.as_mut_ptr()).format = self.sample_format as i32;
            (*frame.as_mut_ptr()).nb_samples = sample_count as i32;
            (*frame.as_mut_ptr()).sample_rate = self.sample_rate as i32;
            sys::av_channel_layout_default(
                &mut (*frame.as_mut_ptr()).ch_layout,
                self.channels as i32,
            );
            ffi::check(
                sys::av_frame_get_buffer(frame.as_mut_ptr(), 0),
                "av_frame_get_buffer",
            )?;
            ffi::check(
                sys::av_frame_make_writable(frame.as_mut_ptr()),
                "av_frame_make_writable",
            )?;
            fill_audio_frame(frame.as_mut_ptr(), samples, sample_count, self.channels)?;
            (*frame.as_mut_ptr()).pts = self.next_pts;
        }
        self.next_pts = self.next_pts.saturating_add(sample_count as i64);
        self.send_frame(output, frame.as_ptr())
    }

    fn send_frame(&mut self, output: &mut OutputContext, frame: *const sys::AVFrame) -> Result<()> {
        unsafe {
            ffi::check(
                sys::avcodec_send_frame(self.context, frame),
                "avcodec_send_frame",
            )?;
        }
        loop {
            let mut packet = AvPacket::new()?;
            let result = unsafe { sys::avcodec_receive_packet(self.context, packet.as_mut_ptr()) };
            if result == sys::AVERROR(libc::EAGAIN) || result == sys::AVERROR_EOF {
                break;
            }
            if result < 0 {
                return Err(ffi::error_from_code("avcodec_receive_packet", result));
            }
            unsafe {
                (*packet.as_mut_ptr()).stream_index = self.stream_index as i32;
                sys::av_packet_rescale_ts(
                    packet.as_mut_ptr(),
                    (*self.context).time_base,
                    self.stream_time_base,
                );
                ffi::check(
                    sys::av_interleaved_write_frame(output.ptr, packet.as_mut_ptr()),
                    "av_interleaved_write_frame",
                )
                .map_err(|error| error.with_path(output.path().to_string()))?;
            }
        }
        Ok(())
    }
}

impl Drop for AudioEncoder {
    fn drop(&mut self) {
        unsafe {
            sys::avcodec_free_context(&mut self.context);
        }
    }
}

unsafe fn fill_audio_frame(
    frame: *mut sys::AVFrame,
    samples: &[f32],
    sample_count: usize,
    channels: u16,
) -> Result<()> {
    let channels = channels as usize;
    let format = unsafe { std::mem::transmute::<i32, sys::AVSampleFormat>((*frame).format) };
    match format {
        AV_SAMPLE_FMT_FLTP => {
            for channel in 0..channels {
                let plane = unsafe { (*frame).data[channel] }.cast::<f32>();
                if plane.is_null() {
                    return Err(FfmpegError::new(
                        "AudioEncoder::fill_audio_frame",
                        "audio frame plane is null",
                    ));
                }
                for sample in 0..sample_count {
                    unsafe {
                        *plane.add(sample) = samples[sample.saturating_mul(channels) + channel];
                    }
                }
            }
            Ok(())
        }
        AV_SAMPLE_FMT_FLT => {
            let data = unsafe { (*frame).data[0] }.cast::<f32>();
            if data.is_null() {
                return Err(FfmpegError::new(
                    "AudioEncoder::fill_audio_frame",
                    "audio frame data is null",
                ));
            }
            unsafe {
                std::ptr::copy_nonoverlapping(samples.as_ptr(), data, samples.len());
            }
            Ok(())
        }
        _ => Err(FfmpegError::new(
            "AudioEncoder::fill_audio_frame",
            "unsupported audio encoder sample format",
        )),
    }
}
