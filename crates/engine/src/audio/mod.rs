//! Audio timelines, sources and mixing.
//!
//! The root composition and each template can hold an audio timeline of tracks and clips. The
//! root's plays as written, and a template's plays wherever a `clip` node plays the template,
//! through that clip's start, duration, offset, speed and end, as its frames do. [`plan`]
//! evaluates gains and clip timing every [`CONTROL_FRAMES`] samples into [`AudioPlan`] segments,
//! and [`AudioMixer`] plays them. The browser preview plays the same plan, so the two mix alike.

mod mix;
mod plan;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    sync::Arc,
};

pub use mix::AudioMixer;
pub use plan::{
    AudioPlan, AudioSegment, AudioVoice, CONTROL_FRAMES, MAX_ACTIVE_CLIPS, MAX_ACTIVE_VOICES,
    MAX_LOOP_RESTARTS, MAX_PREVIEW_INTERVALS, MAX_PREVIEW_SEGMENTS, plan, preview_plan,
};

use crate::{
    composition::{CompositionTemplate, PropDef, Scene},
    error::MediaError,
    expr::{BuiltinFn, ExprNode, Expression, ExpressionContext, GlobalVar},
    node::NodeKind,
};

pub const AUDIO_SAMPLE_RATE: u32 = 48_000;
pub const AUDIO_CHANNELS: usize = 2;

/// Most times the clips of a composition may play templates that have sound, counting the clips
/// inside the templates they play. A count is of places that can sound, not of those sounding at
/// once, which [`MAX_ACTIVE_CLIPS`] and [`MAX_ACTIVE_VOICES`] bound.
pub const MAX_PLAYED_TEMPLATES: u64 = 4096;

/// How many times the root's clips play a template with sound, counting every clip node on the
/// way down: a template with sound counts once for each clip that plays it, plus what the clips
/// in it play. Templates must not play themselves. Counts saturate.
pub(crate) fn played_templates(
    root: &Scene,
    templates: &BTreeMap<String, CompositionTemplate>,
) -> u64 {
    fn plays(scene: &Scene) -> impl Iterator<Item = &str> {
        scene.graph.nodes.values().filter_map(|node| match node {
            NodeKind::Clip(clip) => clip.template_name(),
            _ => None,
        })
    }
    // The count for each template, found children first so a template shared by many clips is
    // counted once.
    let mut counts: HashMap<&str, u64> = HashMap::new();
    let mut stack: Vec<&str> = plays(root).collect();
    while let Some(&name) = stack.last() {
        if counts.contains_key(name) {
            stack.pop();
            continue;
        }
        let Some(template) = templates.get(name) else {
            counts.insert(name, 0);
            stack.pop();
            continue;
        };
        let pending: Vec<&str> = plays(&template.scene)
            .filter(|name| !counts.contains_key(name))
            .collect();
        if !pending.is_empty() {
            stack.extend(pending);
            continue;
        }
        let inside = plays(&template.scene)
            .map(|name| counts[name])
            .fold(0, u64::saturating_add);
        let sounds = template
            .audio
            .as_ref()
            .is_some_and(|audio| !audio.clips.is_empty());
        counts.insert(
            name,
            if sounds || inside > 0 {
                inside.saturating_add(1)
            } else {
                0
            },
        );
        stack.pop();
    }
    plays(root)
        .map(|name| counts[name])
        .fold(0, u64::saturating_add)
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct AudioTimeline {
    pub tracks: Vec<AudioTrack>,
    pub clips: Vec<AudioClip>,
}

impl AudioTimeline {
    /// What stops the timeline from playing as written: repeated ids, clips on tracks it doesn't
    /// declare, fixed gains that aren't finite and at least 0, gain expressions that read what
    /// audio has no value for, and clips without a duration. `props` are the props of the
    /// template the timeline is in, or `None` at the root.
    pub(crate) fn problems(&self, props: Option<&BTreeMap<String, PropDef>>) -> Vec<String> {
        let mut problems = Vec::new();
        let mut tracks = HashSet::new();
        for track in &self.tracks {
            if !tracks.insert(track.id.as_str()) {
                problems.push(format!("duplicate track id `{}`", track.id));
            }
            if let Some(problem) = track.volume.problem(props) {
                problems.push(format!("track `{}` volume {problem}", track.id));
            }
        }
        let declared = match self.tracks.as_slice() {
            [] => "none".to_string(),
            tracks => tracks
                .iter()
                .map(|track| format!("`{}`", track.id))
                .collect::<Vec<_>>()
                .join(", "),
        };
        let mut clips = HashSet::new();
        for clip in &self.clips {
            if !clips.insert(clip.id.as_str()) {
                problems.push(format!("duplicate clip id `{}`", clip.id));
            }
            if !tracks.contains(clip.track_id.as_str()) {
                problems.push(format!(
                    "clip `{}` names unknown track `{}`; declared tracks: {declared}",
                    clip.id, clip.track_id
                ));
            }
            if let Some(problem) = clip.volume.problem(props) {
                problems.push(format!("clip `{}` volume {problem}", clip.id));
            }
            if clip.duration_ms == 0 {
                problems.push(format!(
                    "clip `{}` duration must be greater than zero",
                    clip.id
                ));
            }
        }
        problems
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioTrack {
    pub id: String,
    pub muted: bool,
    pub solo: bool,
    /// Scales every clip on the track.
    pub volume: AudioGain,
}

#[derive(Debug, Clone, PartialEq, lumen_macros::MediaSources)]
pub struct AudioClip {
    pub id: String,
    #[media(audio)]
    pub source_id: String,
    pub track_id: String,
    pub start_ms: u64,
    pub duration_ms: u64,
    pub source_start_ms: u64,
    pub volume: AudioGain,
    /// How long the clip takes to rise from silence at its start, and to fall to silence at its
    /// end, on an equal-power curve. Each is at most the clip's duration, and where the two
    /// overlap their gains multiply.
    pub fade_in_ms: u64,
    pub fade_out_ms: u64,
}

/// A volume: a fixed gain, or an expression evaluated at the frame of the composition the
/// timeline is in. An expression's value below 0, or NaN, is silence.
#[derive(Debug, Clone, PartialEq)]
pub enum AudioGain {
    Value(f32),
    Expr(Expression),
}

impl From<f32> for AudioGain {
    fn from(gain: f32) -> Self {
        Self::Value(gain)
    }
}

impl AudioGain {
    /// Whether this is a fixed gain of 0 or less, which silences what it scales.
    pub(crate) fn is_silent(&self) -> bool {
        matches!(self, Self::Value(gain) if *gain <= 0.0)
    }

    pub(crate) fn eval(&self, ctx: &ExpressionContext<'_>) -> crate::Result<f64> {
        let expression = match self {
            Self::Value(gain) => return Ok(f64::from(*gain)),
            Self::Expr(expression) => expression,
        };
        let value = expression.evaluate(ctx)?;
        let gain = value.as_f64().ok_or_else(|| {
            crate::error::LumenError::evaluate(format!(
                "`={}` must be a number, got {} `{}`",
                expression.source,
                value.type_name(),
                value.as_string()
            ))
        })?;
        Ok(if gain.is_nan() {
            0.0
        } else {
            gain.clamp(0.0, f64::from(f32::MAX))
        })
    }

    /// Why this gain can't play: a fixed gain that isn't finite and at least 0, or an expression
    /// that reads a node, a glyph or text, or a prop the template doesn't declare.
    fn problem(&self, props: Option<&BTreeMap<String, PropDef>>) -> Option<String> {
        let expression = match self {
            Self::Value(gain) if gain.is_finite() && *gain >= 0.0 => return None,
            Self::Value(_) => return Some("must be a finite number of at least 0".to_string()),
            Self::Expr(expression) => expression,
        };
        let mut problem = None;
        crate::expr::functions::visit(expression.ast(), &mut |node| {
            let read = match node {
                ExprNode::Node(id) | ExprNode::PropertyValue(id, _) => format!("reads node {id}"),
                ExprNode::Global(
                    global @ (GlobalVar::Glyph
                    | GlobalVar::Word
                    | GlobalVar::Line
                    | GlobalVar::Span
                    | GlobalVar::GlyphCount
                    | GlobalVar::WordCount
                    | GlobalVar::LineCount
                    | GlobalVar::SpanCount),
                ) => format!(
                    "reads `{}`, which only text glyph params have",
                    global.name()
                ),
                ExprNode::Builtin(builtin @ (BuiltinFn::TextWidth | BuiltinFn::TextHeight), _) => {
                    format!("measures text with `{}`", builtin.name())
                }
                _ => return,
            };
            problem.get_or_insert(read);
        });
        let source = &expression.source;
        match problem {
            Some(read) => Some(format!(
                "`={source}` {read}; an audio volume reads only `frame`, `time`, `fps`, `width`, `height` and props"
            )),
            None => crate::composition::prop_problem(expression, props),
        }
    }
}

/// Samples at [`AUDIO_SAMPLE_RATE`], one list per channel, all the same length.
#[derive(Debug, Clone, PartialEq)]
pub struct AudioBuffer {
    channels: Vec<Vec<f32>>,
}

impl AudioBuffer {
    /// Panics if the channels differ in length.
    pub fn from_channels(channels: Vec<Vec<f32>>) -> Self {
        if let Some(first) = channels.first() {
            assert!(
                channels.iter().all(|channel| channel.len() == first.len()),
                "audio channels must be the same length"
            );
        }
        Self { channels }
    }

    pub fn channel_count(&self) -> usize {
        self.channels.len()
    }

    pub fn frames(&self) -> usize {
        self.channels.first().map_or(0, Vec::len)
    }

    pub fn channels(&self) -> &[Vec<f32>] {
        &self.channels
    }

    pub fn interleaved_f32(&self) -> Vec<f32> {
        let frames = self.frames();
        let channels = self.channel_count();
        let mut output = Vec::with_capacity(frames.saturating_mul(channels));
        for frame in 0..frames {
            for channel in 0..channels {
                output.push(self.channels[channel][frame]);
            }
        }
        output
    }
}

pub trait AudioResolver: Send + Sync {
    /// `frames` frames from `start_sample`, at [`AUDIO_SAMPLE_RATE`]. A source that ends early
    /// returns fewer, or pads with silence.
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError>;
}

impl<T: AudioResolver + ?Sized> AudioResolver for Arc<T> {
    fn resolve_range(
        &self,
        start_sample: u64,
        frames: usize,
    ) -> Result<Arc<AudioBuffer>, MediaError> {
        (**self).resolve_range(start_sample, frames)
    }
}

pub fn ms_to_sample(ms: u64) -> u64 {
    ((ms as u128 * u128::from(AUDIO_SAMPLE_RATE)) / 1_000) as u64
}

pub fn duration_samples(duration_frames: u32, fps: f32) -> u64 {
    ((duration_frames as f64 / fps as f64) * f64::from(AUDIO_SAMPLE_RATE)).round() as u64
}

#[cfg(test)]
mod tests;
