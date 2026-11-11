//! Plays an [`AudioPlan`] from its sources.
//!
//! `packages/lumen-preview/src/audio-worker.js` plays plans the same way in the browser; the
//! fixtures below keep the two in step.

use std::{
    collections::{HashMap, VecDeque},
    f64::consts::FRAC_PI_2,
    sync::Arc,
};

use super::{AUDIO_CHANNELS, AudioBuffer, AudioResolver, AudioSegment, AudioVoice, plan};
use crate::{composition::Composition, media::MediaStore};

/// Samples each clip ramps over at its start and end, and where a window cuts it, so it never
/// starts or stops on a click.
const CLIP_EDGE_FADE_SAMPLES: f64 = 384.0;
/// Sources are read in aligned blocks of this many samples, so consecutive reads continue each
/// other and a source position read twice comes from the same decode.
const BLOCK_FRAMES: u64 = 4_096;
/// Blocks kept between reads, a few seconds of audio.
const MAX_BLOCKS: usize = 64;
/// Most source samples read at once for one segment, so a fast clip reads in bounded pieces.
const MAX_SOURCE_SPAN: u64 = 1 << 16;

pub struct AudioMixer<'a, P: MediaStore> {
    composition: &'a Composition,
    provider: &'a P,
    resolvers: HashMap<String, Box<dyn AudioResolver>>,
    blocks: HashMap<(String, u64), Arc<AudioBuffer>>,
    /// The cached blocks, oldest first.
    order: VecDeque<(String, u64)>,
}

impl<'a, P: MediaStore> AudioMixer<'a, P> {
    pub fn new(composition: &'a Composition, provider: &'a P) -> Self {
        Self {
            composition,
            provider,
            resolvers: HashMap::new(),
            blocks: HashMap::new(),
            order: VecDeque::new(),
        }
    }

    /// The mix of root samples `[start_sample, start_sample + frames)`, clamped to `[-1, 1]`.
    pub fn mix_range(&mut self, start_sample: u64, frames: usize) -> crate::Result<AudioBuffer> {
        let media: &dyn MediaStore = self.provider;
        let plan = plan(self.composition, Some(media), start_sample, frames as u64)?;
        let mut output = vec![vec![0.0_f32; frames]; AUDIO_CHANNELS];
        let end = start_sample.saturating_add(frames as u64);
        for segment in &plan.segments {
            let first = start_sample.saturating_sub(segment.start);
            let last = end
                .min(segment.start.saturating_add(segment.frames))
                .saturating_sub(segment.start);
            let voice = &plan.voices[segment.voice];
            let per_piece = ((MAX_SOURCE_SPAN as f64 / segment.step) as u64).max(1);
            let mut piece = first;
            while piece < last {
                let piece_end = last.min(piece.saturating_add(per_piece));
                self.mix_piece(
                    voice,
                    segment,
                    piece..piece_end,
                    &mut output,
                    segment.start + piece - start_sample,
                )?;
                piece = piece_end;
            }
        }
        for sample in output.iter_mut().flatten() {
            *sample = if sample.is_nan() {
                0.0
            } else {
                sample.clamp(-1.0, 1.0)
            };
        }
        Ok(AudioBuffer::from_channels(output))
    }

    /// Adds samples `range` of `segment` to `output` from `offset` on.
    fn mix_piece(
        &mut self,
        voice: &AudioVoice,
        segment: &AudioSegment,
        range: std::ops::Range<u64>,
        output: &mut [Vec<f32>],
        offset: u64,
    ) -> crate::Result<()> {
        let source_position =
            |i: u64| voice.source_start + segment.position + segment.step * i as f64;
        let read_start = source_position(range.start).floor().max(0.0) as u64;
        let read_end = source_position(range.end - 1).floor().max(0.0) as u64 + 2;
        let source = self.read(&voice.source_id, read_start, read_end - read_start)?;
        for (index, i) in range.enumerate() {
            let Some((position, gain)) = voice_sample(voice, segment, i) else {
                continue;
            };
            let whole = position.floor();
            let fraction = position - whole;
            let at = (whole as u64).saturating_sub(read_start) as usize;
            for (channel, samples) in source.iter().enumerate() {
                let value = if fraction == 0.0 {
                    f64::from(samples[at])
                } else {
                    f64::from(samples[at]) * (1.0 - fraction)
                        + f64::from(samples[at + 1]) * fraction
                };
                output[channel][offset as usize + index] += (value * gain) as f32;
            }
        }
        Ok(())
    }

    /// Source samples `[start, start + frames)` of `source` per channel, silent past its end.
    fn read(&mut self, source: &str, start: u64, frames: u64) -> crate::Result<Vec<Vec<f32>>> {
        let mut channels = vec![vec![0.0; frames as usize]; AUDIO_CHANNELS];
        let end = start + frames;
        for block in start / BLOCK_FRAMES..=(end - 1) / BLOCK_FRAMES {
            let buffer = self.block(source, block)?;
            if buffer.channel_count() == 0 {
                continue;
            }
            let block_start = block * BLOCK_FRAMES;
            let from = start.max(block_start);
            let to = end.min(block_start + buffer.frames() as u64);
            if from >= to {
                continue;
            }
            let (from_block, to_block) =
                ((from - block_start) as usize, (to - block_start) as usize);
            for (channel, samples) in channels.iter_mut().enumerate() {
                let source = &buffer.channels()[channel.min(buffer.channel_count() - 1)];
                samples[(from - start) as usize..(to - start) as usize]
                    .copy_from_slice(&source[from_block..to_block]);
            }
        }
        Ok(channels)
    }

    fn block(&mut self, source: &str, block: u64) -> crate::Result<Arc<AudioBuffer>> {
        let key = (source.to_string(), block);
        if let Some(buffer) = self.blocks.get(&key) {
            return Ok(Arc::clone(buffer));
        }
        let resolver = match self.resolvers.get(source) {
            Some(resolver) => resolver,
            None => {
                let resolver = self.provider.get_audio_resolver(source)?;
                self.resolvers.entry(source.to_string()).or_insert(resolver)
            }
        };
        let buffer = resolver.resolve_range(block * BLOCK_FRAMES, BLOCK_FRAMES as usize)?;
        if self.order.len() >= MAX_BLOCKS
            && let Some(oldest) = self.order.pop_front()
        {
            self.blocks.remove(&oldest);
        }
        self.order.push_back(key.clone());
        self.blocks.insert(key, Arc::clone(&buffer));
        Ok(buffer)
    }
}

/// The source position sample `i` of `segment` plays and its gain, or `None` where it is silent.
fn voice_sample(voice: &AudioVoice, segment: &AudioSegment, i: u64) -> Option<(f64, f64)> {
    let position = segment.position + segment.step * i as f64;
    if !(position >= segment.from && position < segment.to) {
        return None;
    }
    let along = i as f64 / segment.frames as f64;
    let gain = segment.gain[0] + (segment.gain[1] - segment.gain[0]) * along;
    let envelope = edge_fade(position - segment.from, segment.to - segment.from)
        * fade(position, voice.fade_in)
        * fade(voice.duration - position, voice.fade_out);
    Some((voice.source_start + position, gain * envelope))
}

/// A linear ramp over the first and last [`CLIP_EDGE_FADE_SAMPLES`] of `length`, or half of it.
fn edge_fade(offset: f64, length: f64) -> f64 {
    let fade = CLIP_EDGE_FADE_SAMPLES.min((length / 2.0).floor());
    if fade <= 0.0 {
        return 1.0;
    }
    if offset < fade {
        return offset / fade;
    }
    let remaining = length - offset;
    if remaining < fade {
        remaining / fade
    } else {
        1.0
    }
}

/// An equal-power fade, `sin(π/2 · distance / length)`, at `distance` samples from the silent end.
fn fade(distance: f64, length: f64) -> f64 {
    if length > 0.0 && distance < length {
        (distance.max(0.0) / length * FRAC_PI_2).sin()
    } else {
        1.0
    }
}
