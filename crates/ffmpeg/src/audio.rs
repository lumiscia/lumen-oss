use std::ptr;

use crate::{
    FfmpegError, Result,
    ffi::{self, AvFrame, CodecContextGuard, sys},
    format::{InputContext, Packet, Rational},
};
use sys::{
    AVRounding::{self, AV_ROUND_UP},
    AVSampleFormat::AV_SAMPLE_FMT_FLT,
};

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

    pub fn sample_rate(&self) -> i64 {
        i64::from(unsafe { (*self.frame.as_ptr()).sample_rate })
    }

    /// Drops the first `count` samples, which must be fewer than the frame holds.
    fn trim_start(&mut self, count: usize) {
        let frame = self.frame.as_mut_ptr();
        unsafe {
            let format = self.frame.sample_format();
            let channels = (*frame).ch_layout.nb_channels as usize;
            let planar = sys::av_sample_fmt_is_planar(format) != 0;
            let planes = if planar { channels } else { 1 };
            let stride =
                sys::av_get_bytes_per_sample(format) as usize * if planar { 1 } else { channels };
            for plane in 0..planes {
                let data = (*frame).extended_data.add(plane);
                *data = (*data).add(count * stride);
            }
            (*frame).nb_samples -= count as i32;
        }
    }
}

pub struct AudioResampler {
    ptr: *mut sys::SwrContext,
    config: AudioResamplerConfig,
    source_sample_rate: i64,
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
            source_sample_rate: i64::from(unsafe { (*source.frame.as_ptr()).sample_rate }),
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
        let input_samples = source.frame.nb_samples();
        let delay = unsafe { sys::swr_get_delay(self.ptr, self.source_sample_rate) };
        let capacity = unsafe {
            sys::av_rescale_rnd(
                delay + input_samples as i64,
                self.config.sample_rate as i64,
                self.source_sample_rate,
                AV_ROUND_UP,
            )
        } as usize;
        // `extended_data` is `data` up to eight channels and the only pointer array past them.
        let input = unsafe { (*source.frame.as_ptr()).extended_data } as *mut *const u8;
        self.convert_raw(input, input_samples, capacity)
    }

    /// Drains the samples the resampler still holds. Call it once, after the last frame.
    pub fn flush(&mut self) -> Result<AudioFrame> {
        let delay = unsafe { sys::swr_get_delay(self.ptr, i64::from(self.config.sample_rate)) };
        self.convert_raw(ptr::null_mut(), 0, delay.max(0) as usize)
    }

    fn convert_raw(
        &mut self,
        input: *mut *const u8,
        input_samples: usize,
        capacity: usize,
    ) -> Result<AudioFrame> {
        let channels = self.config.channels as usize;
        let mut output = vec![0_f32; capacity.saturating_mul(channels)];
        let mut output_ptr = output.as_mut_ptr() as *mut u8;
        let converted = unsafe {
            sys::swr_convert(
                self.ptr,
                ptr::addr_of_mut!(output_ptr),
                capacity as i32,
                input,
                input_samples as i32,
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

/// Samples a read may skip by decoding on before it seeks instead.
const FORWARD_SCAN_SAMPLES: i64 = 2 * 48_000;

/// How far before the requested start a seek lands, so codecs that need earlier packets to decode
/// cleanly (AAC's overlapped transform, MP3's bit reservoir) have them. This is the back-off the
/// `ffmpeg` command applies to `-ss` on inputs with B-frame video, and the same landing keeps
/// timestamps rounded to a coarse time base on the same samples.
const SEEK_PREROLL_SAMPLES: i64 = 3 * 48_000 / 23;

/// Reads ranges of one audio stream as 48 kHz stereo f32.
///
/// Positions count output samples from the container's first timestamp, so audio that starts after
/// the container does reads as silence until it begins. Ranges that follow each other continue the
/// decoder where it stopped; any other range seeks.
pub struct AudioRangeReader {
    input: InputContext,
    decoder: AudioDecoder,
    resampler: Option<AudioResampler>,
    /// The container's first timestamp, in output samples.
    origin: i64,
    /// Where the next decoded block starts, unknown until the first frame after a seek.
    next_position: Option<i64>,
    /// Where the first block read after a seek or open starts.
    anchor: i64,
    /// The end of the last range read. Earlier samples are gone.
    cursor: i64,
    /// The part of a block past the last range read.
    stash: Option<AudioBlock>,
    input_ended: bool,
    finished: bool,
}

/// Interleaved stereo samples and the position of the first one.
struct AudioBlock {
    position: i64,
    samples: Vec<f32>,
}

impl AudioBlock {
    fn end(&self) -> i64 {
        self.position + (self.samples.len() / CHANNELS) as i64
    }
}

const CHANNELS: usize = 2;
const OUTPUT_TIME_BASE: sys::AVRational = sys::AVRational {
    num: 1,
    den: 48_000,
};

impl AudioRangeReader {
    /// Opens the best audio stream of `path`, which must pass the same input allowlists as
    /// [`InputContext::open`].
    pub fn open(path: impl Into<String>) -> Result<Self> {
        let input = InputContext::open(path)?;
        let stream_index = input.best_audio_stream()?.stream_index;
        let decoder = AudioDecoder::open(&input, stream_index)?;
        let origin = unsafe {
            let start = (*input.ptr).start_time;
            if start == sys::AV_NOPTS_VALUE {
                0
            } else {
                sys::av_rescale_q(
                    start,
                    sys::AVRational {
                        num: 1,
                        den: sys::AV_TIME_BASE,
                    },
                    OUTPUT_TIME_BASE,
                )
            }
        };
        // Demuxers skip the streams marked discarded, so other streams' packets are never read.
        unsafe {
            for index in 0..(*input.ptr).nb_streams as usize {
                if index != stream_index {
                    (**(*input.ptr).streams.add(index)).discard = sys::AVDiscard::AVDISCARD_ALL;
                }
            }
        }
        Ok(Self {
            input,
            decoder,
            resampler: None,
            origin,
            next_position: None,
            anchor: 0,
            cursor: 0,
            stash: None,
            input_ended: false,
            finished: false,
        })
    }

    /// `frames` samples from `start_sample`, or fewer when the stream ends first. A range wholly
    /// past the end is empty.
    pub fn read(&mut self, start_sample: u64, frames: usize) -> Result<AudioFrame> {
        let start = i64::try_from(start_sample)
            .map_err(|_| FfmpegError::new("AudioRangeReader::read", "start sample is too large"))?;
        let end = start.saturating_add(frames as i64);
        if start < self.cursor || start - self.cursor > FORWARD_SCAN_SAMPLES {
            self.seek(start)?;
        }
        self.anchor = start;
        // A failed read leaves the decoder somewhere unknown, so the next one seeks.
        self.cursor = i64::MAX;

        let mut samples = vec![0.0; frames * CHANNELS];
        let mut covered = start;
        while let Some(block) = self.next_block()? {
            if block.position >= end {
                covered = end;
                self.stash = Some(block);
                break;
            }
            let from = block.position.max(start);
            let to = block.end().min(end);
            if from < to {
                let source = &block.samples[(from - block.position) as usize * CHANNELS..]
                    [..(to - from) as usize * CHANNELS];
                samples[(from - start) as usize * CHANNELS..][..source.len()]
                    .copy_from_slice(source);
                covered = to;
            }
            if block.end() >= end {
                covered = end;
                if block.end() > end {
                    self.stash = Some(AudioBlock {
                        position: end,
                        samples: block.samples[(end - block.position) as usize * CHANNELS..]
                            .to_vec(),
                    });
                }
                break;
            }
        }
        samples.truncate((covered - start) as usize * CHANNELS);
        self.cursor = end;
        Ok(AudioFrame {
            sample_rate: 48_000,
            channels: CHANNELS as u16,
            samples: samples.len() / CHANNELS,
            interleaved_f32: samples,
        })
    }

    fn seek(&mut self, start: i64) -> Result<()> {
        let target = start - SEEK_PREROLL_SAMPLES;
        // Seeking to the start skips the leading trim some containers apply on first read, so a
        // reader going back there starts over.
        if target <= 0 {
            *self = Self::open(self.input.path())?;
            return Ok(());
        }
        let target = target + self.origin;
        let time_base = self.decoder.time_base();
        let timestamp = unsafe {
            sys::av_rescale_q(
                target,
                OUTPUT_TIME_BASE,
                sys::AVRational {
                    num: time_base.numerator,
                    den: time_base.denominator,
                },
            )
        };
        self.input
            .seek_stream(self.decoder.stream_index(), timestamp)?;
        self.decoder.flush();
        self.resampler = None;
        self.next_position = None;
        self.stash = None;
        self.input_ended = false;
        self.finished = false;
        Ok(())
    }

    fn next_block(&mut self) -> Result<Option<AudioBlock>> {
        if let Some(block) = self.stash.take() {
            return Ok(Some(block));
        }
        while !self.finished {
            if let Some(mut frame) = self.decoder.receive_frame()? {
                let position = match self.next_position {
                    Some(position) => position,
                    None => match self.start_run(&mut frame) {
                        Some(position) => position,
                        None => continue,
                    },
                };
                let resampler = match &mut self.resampler {
                    Some(resampler) => resampler,
                    None => self.resampler.insert(AudioResampler::new(
                        &frame,
                        AudioResamplerConfig::default(),
                    )?),
                };
                let converted = resampler.convert(&frame)?;
                return Ok(Some(self.block_at(position, converted)));
            }
            if self.input_ended {
                self.finished = true;
                let (Some(resampler), Some(position)) = (&mut self.resampler, self.next_position)
                else {
                    break;
                };
                let tail = resampler.flush()?;
                return Ok(Some(self.block_at(position, tail)));
            }
            self.feed_decoder()?;
        }
        Ok(None)
    }

    fn block_at(&mut self, position: i64, converted: AudioFrame) -> AudioBlock {
        let block = AudioBlock {
            position,
            samples: converted.interleaved_f32,
        };
        self.next_position = Some(block.end());
        block
    }

    /// Positions the first frame of a run. Samples before the anchor go, as `-ss` drops them, so
    /// the resampler's output lands on whole samples from the anchor. Returns `None` when the
    /// whole frame goes.
    fn start_run(&mut self, frame: &mut DecodedAudioFrame) -> Option<i64> {
        let rate = frame.sample_rate();
        let wanted = unsafe { sys::av_rescale_rnd(self.anchor, rate, 48_000, AV_ROUND_UP) };
        let first = frame
            .pts()
            .map_or(wanted, |pts| self.source_position(pts, rate));
        if first >= wanted {
            return Some(if first == wanted {
                self.anchor
            } else {
                unsafe { sys::av_rescale_rnd(first, 48_000, rate, AVRounding::AV_ROUND_NEAR_INF) }
            });
        }
        let skipped = (wanted - first) as usize;
        if skipped >= frame.samples() {
            return None;
        }
        frame.trim_start(skipped);
        Some(self.anchor)
    }

    /// A timestamp of the stream as source samples after the container's first timestamp.
    fn source_position(&self, pts: i64, rate: i64) -> i64 {
        let time_base = self.decoder.time_base();
        unsafe {
            sys::av_rescale_q(
                pts,
                sys::AVRational {
                    num: time_base.numerator,
                    den: time_base.denominator,
                },
                sys::AVRational {
                    num: 1,
                    den: rate as i32,
                },
            ) - sys::av_rescale_rnd(self.origin, rate, 48_000, AV_ROUND_UP)
        }
    }

    fn feed_decoder(&mut self) -> Result<()> {
        let Some(packet) = self.input.read_packet()? else {
            self.input_ended = true;
            return self.decoder.send_eof();
        };
        match self.decoder.send_packet(&packet) {
            // A damaged packet costs its own samples, not the rest of the stream.
            Err(error) if error.code == Some(sys::AVERROR_INVALIDDATA) => Ok(()),
            result => result,
        }
    }
}
