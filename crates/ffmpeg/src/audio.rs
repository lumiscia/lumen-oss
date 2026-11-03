use std::ptr;

use crate::{
    FfmpegError, Result,
    ffi::{self, AvFrame, CodecContextGuard, sys},
    format::{InputContext, Packet, Rational},
};
use sys::{AVRounding::AV_ROUND_UP, AVSampleFormat::AV_SAMPLE_FMT_FLT};

/// Interleaved f32 audio. `samples` counts frames, so `interleaved_f32` holds `samples * channels`
/// values.
#[derive(Debug, Clone)]
pub struct AudioFrame {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: usize,
    pub interleaved_f32: Vec<f32>,
}

#[derive(Debug, Clone, Copy)]
pub struct AudioResamplerConfig {
    pub sample_rate: u32,
    pub channels: u16,
}

impl Default for AudioResamplerConfig {
    fn default() -> Self {
        Self {
            sample_rate: 48_000,
            channels: 2,
        }
    }
}

pub struct AudioDecoder {
    stream_index: usize,
    time_base: Rational,
    context: *mut sys::AVCodecContext,
}

unsafe impl Send for AudioDecoder {}

impl AudioDecoder {
    pub fn open(input: &InputContext, stream_index: usize) -> Result<Self> {
        let parameters = input.stream_parameters(stream_index)?;
        let codec = unsafe { sys::avcodec_find_decoder((*parameters).codec_id) };
        if codec.is_null() {
            return Err(FfmpegError::new(
                "avcodec_find_decoder",
                "no audio decoder found",
            ));
        }
        let mut context = CodecContextGuard(unsafe { sys::avcodec_alloc_context3(codec) });
        if context.0.is_null() {
            return Err(FfmpegError::new(
                "avcodec_alloc_context3",
                "failed to allocate audio decoder context",
            ));
        }
        unsafe {
            ffi::check(
                sys::avcodec_parameters_to_context(context.0, parameters),
                "avcodec_parameters_to_context",
            )?;
            ffi::check(
                sys::avcodec_open2(context.0, codec, ptr::null_mut()),
                "avcodec_open2",
            )?;
        }
        Ok(Self {
            stream_index,
            time_base: input.stream_time_base(stream_index)?.into(),
            context: context.release(),
        })
    }

    pub fn stream_index(&self) -> usize {
        self.stream_index
    }

    pub fn time_base(&self) -> Rational {
        self.time_base
    }

    pub fn send_packet(&mut self, packet: &Packet) -> Result<()> {
        if packet.stream_index() != self.stream_index {
            return Ok(());
        }
        unsafe {
            ffi::check(
                sys::avcodec_send_packet(self.context, packet.inner.as_ptr()),
                "avcodec_send_packet",
            )
        }
    }

    pub fn send_eof(&mut self) -> Result<()> {
        unsafe {
            ffi::check(
                sys::avcodec_send_packet(self.context, ptr::null()),
                "avcodec_send_packet",
            )
        }
    }

    pub fn flush(&mut self) {
        unsafe { sys::avcodec_flush_buffers(self.context) };
    }

    pub fn receive_frame(&mut self) -> Result<Option<DecodedAudioFrame>> {
        let mut frame = AvFrame::new()?;
        let result = unsafe { sys::avcodec_receive_frame(self.context, frame.as_mut_ptr()) };
        if result == sys::AVERROR(libc::EAGAIN) || result == sys::AVERROR_EOF {
            return Ok(None);
        }
        if result < 0 {
            return Err(ffi::error_from_code("avcodec_receive_frame", result));
        }
        Ok(Some(DecodedAudioFrame { frame }))
    }
}

impl Drop for AudioDecoder {
    fn drop(&mut self) {
        unsafe { sys::avcodec_free_context(&mut self.context) };
    }
}

pub struct DecodedAudioFrame {
    frame: AvFrame,
}

impl DecodedAudioFrame {
    pub fn samples(&self) -> usize {
        self.frame.nb_samples()
    }

    pub fn pts(&self) -> Option<i64> {
        self.frame.pts()
    }
}

pub struct AudioResampler {
    ptr: *mut sys::SwrContext,
    config: AudioResamplerConfig,
}

unsafe impl Send for AudioResampler {}

impl AudioResampler {
    pub fn new(source: &DecodedAudioFrame, config: AudioResamplerConfig) -> Result<Self> {
        let mut dst_layout = unsafe { std::mem::zeroed() };
        let mut src_layout = unsafe { (*source.frame.as_ptr()).ch_layout };
        unsafe {
            sys::av_channel_layout_default(&mut dst_layout, config.channels as i32);
            if src_layout.nb_channels == 0 {
                sys::av_channel_layout_default(
                    &mut src_layout,
                    (*source.frame.as_ptr()).ch_layout.nb_channels,
                );
            }
        }
        // Owning the context before it is configured frees it if `swr_init` fails.
        let mut resampler = Self {
            ptr: ptr::null_mut(),
            config,
        };
        unsafe {
            ffi::check(
                sys::swr_alloc_set_opts2(
                    &mut resampler.ptr,
                    ptr::addr_of_mut!(dst_layout),
                    AV_SAMPLE_FMT_FLT,
                    config.sample_rate as i32,
                    ptr::addr_of_mut!(src_layout),
                    source.frame.sample_format(),
                    (*source.frame.as_ptr()).sample_rate,
                    0,
                    ptr::null_mut(),
                ),
                "swr_alloc_set_opts2",
            )?;
            ffi::check(sys::swr_init(resampler.ptr), "swr_init")?;
        }
        Ok(resampler)
    }

    pub fn convert(&mut self, source: &DecodedAudioFrame) -> Result<AudioFrame> {
        let input_samples = source.frame.nb_samples() as i64;
        let delay =
            unsafe { sys::swr_get_delay(self.ptr, (*source.frame.as_ptr()).sample_rate as i64) };
        let output_capacity = unsafe {
            sys::av_rescale_rnd(
                delay + input_samples,
                self.config.sample_rate as i64,
                (*source.frame.as_ptr()).sample_rate as i64,
                AV_ROUND_UP,
            )
        } as usize;
        let channels = self.config.channels as usize;
        let mut output = vec![0_f32; output_capacity.saturating_mul(channels)];
        let mut output_ptr = output.as_mut_ptr() as *mut u8;
        let converted = unsafe {
            sys::swr_convert(
                self.ptr,
                ptr::addr_of_mut!(output_ptr),
                output_capacity as i32,
                (*source.frame.as_ptr()).data.as_ptr() as *mut *const u8,
                source.frame.nb_samples() as i32,
            )
        };
        if converted < 0 {
            return Err(ffi::error_from_code("swr_convert", converted));
        }
        let samples = converted as usize;
        output.truncate(samples.saturating_mul(channels));
        Ok(AudioFrame {
            sample_rate: self.config.sample_rate,
            channels: self.config.channels,
            samples,
            interleaved_f32: output,
        })
    }
}

impl Drop for AudioResampler {
    fn drop(&mut self) {
        unsafe { sys::swr_free(&mut self.ptr) };
    }
}
