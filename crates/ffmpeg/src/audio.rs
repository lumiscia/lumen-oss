use std::{collections::VecDeque, ptr};

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
        let time_base: Rational = input.stream_time_base(stream_index)?.into();
        unsafe {
            ffi::check(
                sys::avcodec_parameters_to_context(context.0, parameters),
                "avcodec_parameters_to_context",
            )?;
            // Without it the decoder trims priming samples but leaves the frame's timestamp where
            // the untrimmed frame began.
            (*context.0).pkt_timebase = sys::AVRational {
                num: time_base.numerator,
                den: time_base.denominator,
            };
            ffi::check(
                sys::avcodec_open2(context.0, codec, ptr::null_mut()),
                "avcodec_open2",
            )?;
        }
        Ok(Self {
            stream_index,
            time_base,
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

/// What a resampler is built for: a frame with any other of these needs a new one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SourceConfig {
    sample_rate: i32,
    format: i32,
    channels: i32,
    layout_order: i32,
    layout_mask: u64,
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

    fn config(&self) -> SourceConfig {
        let frame = self.frame.as_ptr();
        unsafe {
            SourceConfig {
                sample_rate: (*frame).sample_rate,
                format: (*frame).format,
                channels: (*frame).ch_layout.nb_channels,
                layout_order: (*frame).ch_layout.order as i32,
                layout_mask: (*frame).ch_layout.u.mask,
            }
        }
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
    source: SourceConfig,
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
            source: source.config(),
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
        let source_rate = i64::from(self.source.sample_rate);
        let delay = unsafe { sys::swr_get_delay(self.ptr, source_rate) };
        let capacity = unsafe {
            sys::av_rescale_rnd(
                delay + input_samples as i64,
                self.config.sample_rate as i64,
                source_rate,
                AV_ROUND_UP,
            )
        } as usize;
        // `extended_data` is `data` up to eight channels and the only pointer array past them.
        let input = unsafe { (*source.frame.as_ptr()).extended_data } as *mut *const u8;
        self.convert_raw(input, input_samples, capacity)
    }

    /// Whether `frame` is in the format this resampler was built for.
    fn accepts(&self, frame: &DecodedAudioFrame) -> bool {
        self.source == frame.config()
    }

    /// Feeds `samples` of silence ahead of the next frame, as if the stream began that much earlier.
    fn inject_silence(&mut self, samples: usize) -> Result<()> {
        unsafe {
            ffi::check(
                sys::swr_inject_silence(self.ptr, samples as i32),
                "swr_inject_silence",
            )
        }
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
/// cleanly (AAC's overlapped transform, MP3's bit reservoir) and the resampler's filter have them.
/// This is the back-off the `ffmpeg` command applies to `-ss` on inputs with B-frame video.
const SEEK_PREROLL_SAMPLES: i64 = 3 * 48_000 / 23;

/// How far a frame's timestamp may be from the samples before it, at the least, without being a
/// gap. Vorbis frames are stamped off by up to a quarter of their block.
const JITTER_MS: i64 = 15;

const CHANNELS: usize = 2;
const OUTPUT_RATE: i64 = 48_000;
const OUTPUT_TIME_BASE: sys::AVRational = sys::AVRational {
    num: 1,
    den: 48_000,
};

/// Reads ranges of one audio stream as 48 kHz stereo f32.
///
/// Positions count output samples from the container's first timestamp. Audio that starts after
/// the container does, or resumes after a gap in the timestamps, reads as silence until it begins.
/// Ranges that follow each other continue the decoder where it stopped and give the samples a
/// decode from the start gives. Any other range seeks, and lands on the timestamp of the first
/// frame it finds, so it can differ from a continuous read by the precision of the container's
/// timestamps (a millisecond in Matroska). Resampling always starts on a whole output sample, so
/// its phase doesn't depend on the seek.
pub struct AudioRangeReader {
    input: InputContext,
    decoder: AudioDecoder,
    resampler: Option<AudioResampler>,
    /// The container's first timestamp, in output samples.
    origin: i64,
    /// How far a timestamp may be from the sample clock, in output samples, before it is a gap or
    /// an overlap and not rounding.
    tolerance: i64,
    /// Where the resampler's next output sample goes, unknown until a run's first frame.
    clock: Option<i64>,
    /// Where the next frame starts in source samples if the timestamps hold.
    next_source: i64,
    /// Where the current read starts, for frames that carry no timestamp.
    hint: i64,
    /// The end of the last range read. Earlier samples are gone.
    cursor: i64,
    /// Decoded output not yet read, in order.
    blocks: VecDeque<AudioBlock>,
    input_ended: bool,
    finished: bool,
    decoded_any: bool,
    /// The first packet or frame that failed to decode, reported only if nothing ever decodes.
    failure: Option<FfmpegError>,
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

fn rescale(value: i64, from: sys::AVRational, to: sys::AVRational) -> i64 {
    unsafe { sys::av_rescale_q(value, from, to) }
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 { a } else { gcd(b, a % b) }
}

impl AudioRangeReader {
    /// Opens the best audio stream of `path`, which must pass the same input allowlists as
    /// [`InputContext::open`].
    pub fn open(path: impl Into<String>) -> Result<Self> {
        let input = InputContext::open(path)?;
        let stream_index = input.best_audio_stream()?.stream_index;
        let decoder = AudioDecoder::open(&input, stream_index)?;
        let time_base = decoder.time_base();
        let tolerance = rescale(
            1,
            sys::AVRational {
                num: time_base.numerator,
                den: time_base.denominator,
            },
            OUTPUT_TIME_BASE,
        )
        .max(OUTPUT_RATE * JITTER_MS / 1000);
        let origin = unsafe {
            let start = (*input.ptr).start_time;
            if start == sys::AV_NOPTS_VALUE {
                0
            } else {
                rescale(
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
            tolerance,
            clock: None,
            next_source: 0,
            hint: 0,
            cursor: 0,
            blocks: VecDeque::new(),
            input_ended: false,
            finished: false,
            decoded_any: false,
            failure: None,
        })
    }

    /// The start of the range that continues this reader without a seek.
    pub fn position(&self) -> u64 {
        self.cursor as u64
    }

    /// `frames` samples from `start_sample`, or fewer when the stream ends first. A range wholly
    /// past the end is empty. Damaged packets are skipped and their span reads as silence, unless
    /// no packet of the stream decodes at all, which is an error.
    pub fn read(&mut self, start_sample: u64, frames: usize) -> Result<AudioFrame> {
        let start = i64::try_from(start_sample)
            .map_err(|_| FfmpegError::new("AudioRangeReader::read", "start sample is too large"))?;
        let end = start.saturating_add(frames as i64);
        if start < self.cursor || start - self.cursor > FORWARD_SCAN_SAMPLES {
            self.seek(start)?;
        }
        self.hint = start;
        // A failed read leaves the decoder somewhere unknown, so the next one seeks.
        self.cursor = i64::MAX;

        let mut samples = vec![0.0; frames * CHANNELS];
        let mut covered = start;
        while let Some(block) = self.next_block()? {
            if block.position >= end {
                covered = end;
                self.blocks.push_front(block);
                break;
            }
            let from = block.position.max(start);
            let to = block.end().min(end);
            if from < to {
                let source = &block.samples[(from - block.position) as usize * CHANNELS..]
                    [..(to - from) as usize * CHANNELS];
                samples[(from - start) as usize * CHANNELS..][..source.len()]
                    .copy_from_slice(source);
                covered = covered.max(to);
            }
            if block.end() >= end {
                covered = end;
                if block.end() > end {
                    self.blocks.push_front(AudioBlock {
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
            let decoded_any = self.decoded_any;
            *self = Self::open(self.input.path())?;
            self.decoded_any = decoded_any;
            return Ok(());
        }
        let time_base = self.decoder.time_base();
        let timestamp = rescale(
            target + self.origin,
            OUTPUT_TIME_BASE,
            sys::AVRational {
                num: time_base.numerator,
                den: time_base.denominator,
            },
        );
        self.input
            .seek_stream(self.decoder.stream_index(), timestamp)?;
        self.decoder.flush();
        self.resampler = None;
        self.clock = None;
        self.blocks.clear();
        self.input_ended = false;
        self.finished = false;
        Ok(())
    }

    fn next_block(&mut self) -> Result<Option<AudioBlock>> {
        loop {
            if let Some(block) = self.blocks.pop_front() {
                return Ok(Some(block));
            }
            if self.finished {
                return match self.failure.take() {
                    Some(failure) if !self.decoded_any => Err(failure),
                    _ => Ok(None),
                };
            }
            match self.decoder.receive_frame() {
                Ok(Some(frame)) => {
                    self.decoded_any = true;
                    self.place_frame(frame)?;
                }
                Ok(None) if self.input_ended => self.finish()?,
                Ok(None) => self.feed_decoder()?,
                Err(error) => {
                    self.failure.get_or_insert(error);
                    if self.input_ended {
                        self.finish()?;
                    } else {
                        self.feed_decoder()?;
                    }
                }
            }
        }
    }

    fn finish(&mut self) -> Result<()> {
        self.finished = true;
        self.end_segment()
    }

    /// Drains the resampler into a block, ending the stretch of output it was producing.
    fn end_segment(&mut self) -> Result<()> {
        let (Some(mut resampler), Some(clock)) = (self.resampler.take(), self.clock) else {
            return Ok(());
        };
        let tail = resampler.flush()?;
        self.push_block(clock, tail);
        Ok(())
    }

    fn push_block(&mut self, position: i64, converted: AudioFrame) {
        let block = AudioBlock {
            position,
            samples: converted.interleaved_f32,
        };
        self.clock = Some(block.end());
        self.blocks.push_back(block);
    }

    /// Puts a decoded frame where its timestamp says, against the clock: the source samples already
    /// fed to the resampler, or the end of the output when the frame is in a new format. Within the
    /// tolerance of the clock the frame continues it, so timestamps rounded to the time base don't
    /// add up. Later leaves silence. Earlier drops the samples that overlap, all of them if the
    /// frame is wholly behind. A frame without a timestamp continues the clock.
    fn place_frame(&mut self, mut frame: DecodedAudioFrame) -> Result<()> {
        let rate = frame.sample_rate();
        let to_source = |output: i64| unsafe {
            sys::av_rescale_rnd(output, rate, OUTPUT_RATE, AVRounding::AV_ROUND_NEAR_INF)
        };
        let tolerance = to_source(self.tolerance);

        let mut continues = self
            .resampler
            .as_ref()
            .is_some_and(|resampler| resampler.accepts(&frame));
        if !continues {
            self.end_segment()?;
        }
        let mut expected = if continues {
            Some(self.next_source)
        } else {
            self.clock.map(to_source)
        };
        let start = frame.pts().map(|pts| self.source_position(pts, rate));
        let delta = start
            .zip(expected)
            .map_or(0, |(start, expected)| start - expected);
        if delta > tolerance {
            self.end_segment()?;
            continues = false;
            expected = None;
        } else if delta < -tolerance {
            if -delta >= frame.samples() as i64 {
                return Ok(());
            }
            frame.trim_start((-delta) as usize);
        }

        let position = if continues {
            self.clock.unwrap_or_default()
        } else if let (Some(clock), Some(expected)) = (self.clock, expected) {
            self.resampler = Some(AudioResampler::new(
                &frame,
                AudioResamplerConfig::default(),
            )?);
            self.next_source = expected;
            clock
        } else {
            // The resampler's output falls on whole output samples only from a source sample that
            // is one too, so a run starts at the last such sample and silence fills in.
            let source = start.unwrap_or_else(|| unsafe {
                sys::av_rescale_rnd(self.hint, rate, OUTPUT_RATE, AV_ROUND_UP)
            });
            let divisor = gcd(rate, OUTPUT_RATE);
            let period = rate / divisor;
            let padding = source.rem_euclid(period);
            let mut resampler = AudioResampler::new(&frame, AudioResamplerConfig::default())?;
            resampler.inject_silence(padding as usize)?;
            self.resampler = Some(resampler);
            self.next_source = source;
            (source - padding) / period * (OUTPUT_RATE / divisor)
        };
        self.next_source += frame.samples() as i64;

        let Some(resampler) = &mut self.resampler else {
            return Ok(());
        };
        let converted = resampler.convert(&frame)?;
        self.push_block(position, converted);
        Ok(())
    }

    /// A timestamp of the stream as source samples after the container's first timestamp.
    fn source_position(&self, pts: i64, rate: i64) -> i64 {
        let time_base = self.decoder.time_base();
        rescale(
            pts,
            sys::AVRational {
                num: time_base.numerator,
                den: time_base.denominator,
            },
            sys::AVRational {
                num: 1,
                den: rate as i32,
            },
        ) - unsafe { sys::av_rescale_rnd(self.origin, rate, OUTPUT_RATE, AV_ROUND_UP) }
    }

    fn feed_decoder(&mut self) -> Result<()> {
        let Some(packet) = self.input.read_packet()? else {
            self.input_ended = true;
            return self.decoder.send_eof();
        };
        if let Err(error) = self.decoder.send_packet(&packet) {
            // A damaged packet costs its own samples, not the rest of the stream.
            self.failure.get_or_insert(error);
        }
        Ok(())
    }
}
