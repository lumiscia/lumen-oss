//! What each audio clip plays where, in root samples, with its gains evaluated.
//!
//! A clip of the root's timeline plays at its own time. A template's clip plays wherever a `clip`
//! node reaching the root's output plays the template, at the template time that node shows, so
//! its start, duration, offset, speed, `hold` and `loop` place the sound as they place the
//! frames, through any depth of nested clips. Speed resamples, so pitch follows it. A `switch`
//! plays only the selected layer's audio, and a `time_remap` plays its source's audio at the
//! remapped time; where the remapped time holds or runs backwards it is silent, as a `hold` past
//! a clip's window is.

use std::collections::{HashMap, HashSet};

use serde::Serialize;

use super::{AUDIO_SAMPLE_RATE, AudioClip, AudioTimeline, AudioTrack, ms_to_sample};
use crate::{
    composition::Composition,
    error::{LumenError, MediaError},
    media::MediaStore,
    node::{
        DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind,
        compositing::clip::{Clip, ClipEnd, ClipWindow},
        processing::time_remap::{TimeRemap, remap_frame},
        required_input,
    },
    scope::{FrameTime, MAX_GRAPH_DEPTH, Scope, route},
};

/// Root samples between the points where gains and clip timing are evaluated. Gains move
/// linearly between them, so they change without zipper noise, and they are evaluated at least
/// once per frame up to 93 fps.
pub const CONTROL_FRAMES: u64 = 512;

const SAMPLE_RATE: f64 = AUDIO_SAMPLE_RATE as f64;

/// What the mixer plays: the voices, and the segments of root samples each plays.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioPlan {
    pub voices: Vec<AudioVoice>,
    pub segments: Vec<AudioSegment>,
}

/// An audio clip as one instance plays it: a clip of the root's timeline, or of a template's for
/// one path of `clip` nodes that plays the template.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioVoice {
    pub source_id: String,
    /// The source sample at the clip's start.
    pub source_start: f64,
    /// The clip's length and its fades, in samples of the composition the clip is in.
    pub duration: f64,
    pub fade_in: f64,
    pub fade_out: f64,
}

/// Root samples `start + i` for `i` in `0..frames`, where a voice plays its clip at
/// `position + step * i` samples from the clip's start, at a gain moving linearly from `gain[0]`
/// at `i = 0` to `gain[1]` at `i = frames`. Only positions in `[from, to)` sound: the clip, cut
/// by the windows of the `clip` nodes that play it.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioSegment {
    pub voice: usize,
    pub start: u64,
    pub frames: u64,
    pub position: f64,
    pub step: f64,
    pub from: f64,
    pub to: f64,
    pub gain: [f64; 2],
}

/// What sounds in root samples `[start, start + frames)`. Every control interval that overlaps
/// them is planned whole, so a sample is planned alike whatever range asks for it. `media` is
/// what expressions that measure text use.
pub fn plan(
    composition: &Composition,
    media: Option<&dyn MediaStore>,
    start: u64,
    frames: u64,
) -> crate::Result<AudioPlan> {
    let mut planner = Planner::new(composition, media);
    if frames == 0 || !composition.has_audio() {
        return Ok(AudioPlan::default());
    }
    let last = start.saturating_add(frames - 1) / CONTROL_FRAMES;
    for interval in start / CONTROL_FRAMES..=last {
        planner.interval(interval * CONTROL_FRAMES)?;
    }
    Ok(planner.plan)
}

/// What sounds over the root's whole timeline, which the browser preview plays and loops.
pub fn preview_plan(
    composition: &Composition,
    media: Option<&dyn MediaStore>,
) -> crate::Result<AudioPlan> {
    let timeline = composition.timeline();
    let seconds = f64::from(timeline.duration_frames) / f64::from(timeline.fps);
    plan(composition, media, 0, (seconds * SAMPLE_RATE).ceil() as u64)
}

struct Planner<'a> {
    composition: &'a Composition,
    media: Option<&'a dyn MediaStore>,
    root_fps: f64,
    /// Templates with audio, or that play a template with audio.
    audible: HashSet<&'a str>,
    voices: HashMap<(Vec<NodeId>, usize), usize>,
    /// The last segment of each voice, which the next may extend.
    last_segments: HashMap<usize, usize>,
    plan: AudioPlan,
}

/// A timeline and how root time reaches it.
struct Instance<'a> {
    /// The `clip` and `time_remap` nodes on the way from the root, which tell instances apart.
    key: Vec<NodeId>,
    chain: Vec<Link>,
    scope: Scope<'a>,
    timeline: &'a AudioTimeline,
}

/// A node reached from the root's output while finding instances.
#[derive(Clone)]
struct Walk<'a> {
    scope: Scope<'a>,
    node: NodeId,
    /// The frame of `scope` the node is evaluated at, for the interval's first sample.
    frame: f64,
    /// Frames of `scope` per root sample.
    rate: f64,
    chain: Vec<Link>,
    key: Vec<NodeId>,
    depth: u32,
}

/// How one step of the way maps the frames of the composition above to the frames below.
#[derive(Debug, Clone, Copy)]
enum Link {
    Window(ClipWindow),
    /// A `time_remap`'s target, linear around where it was evaluated: `frame + rate * (t - at)`.
    Remap {
        at: f64,
        frame: f64,
        rate: f64,
    },
}

/// A link around one frame: `out = base_out + (in - base_in) * rate`, with the content it can
/// show in `[from, to)`.
struct Piece {
    base_in: f64,
    base_out: f64,
    rate: f64,
    from: f64,
    to: f64,
}

impl Piece {
    fn map(&self, frame: f64) -> f64 {
        self.base_out + (frame - self.base_in) * self.rate
    }
}

impl Link {
    /// The piece that plays at `frame`, or `None` where nothing sounds.
    fn piece(&self, frame: f64) -> Option<Piece> {
        match *self {
            Self::Window(window) => {
                let elapsed = frame - window.start;
                if !(elapsed >= 0.0 && window.duration > 0.0) {
                    return None;
                }
                let repeat = if elapsed < window.duration {
                    0.0
                } else if window.end == ClipEnd::Loop {
                    (elapsed / window.duration).floor()
                } else {
                    return None;
                };
                Some(Piece {
                    base_in: window.start + repeat * window.duration,
                    base_out: window.offset,
                    rate: window.rate(),
                    from: window.offset,
                    to: window.offset + window.duration * window.rate(),
                })
            }
            Self::Remap { at, frame, rate } => Some(Piece {
                base_in: at,
                base_out: frame,
                rate,
                from: f64::NEG_INFINITY,
                to: f64::INFINITY,
            }),
        }
    }
}

/// Where root time lands in an instance at one sample: a position in the instance's samples, how
/// far it moves per root sample, and the positions the windows on the way let through.
#[derive(Debug, Clone, Copy, PartialEq)]
struct Trace {
    position: f64,
    step: f64,
    from: f64,
    to: f64,
}

impl Trace {
    /// Whether `next`, `offset` samples on, is on the same line as this trace.
    fn continues_to(&self, next: &Self, offset: u64) -> bool {
        let expected = self.position + self.step * offset as f64;
        self.step == next.step
            && self.from == next.from
            && self.to == next.to
            && (expected - next.position).abs() <= 1e-3 + 1e-9 * next.position.abs()
    }
}

/// Root samples `[start, start + frames)` on one line of an instance, traced at `start`.
struct Run {
    start: u64,
    frames: u64,
    trace: Trace,
}

impl<'a> Planner<'a> {
    fn new(composition: &'a Composition, media: Option<&'a dyn MediaStore>) -> Self {
        Self {
            composition,
            media,
            root_fps: f64::from(composition.timeline().fps),
            audible: audible_templates(composition),
            voices: HashMap::new(),
            last_segments: HashMap::new(),
            plan: AudioPlan::default(),
        }
    }

    /// Plans root samples `[start, start + CONTROL_FRAMES)`.
    fn interval(&mut self, start: u64) -> crate::Result<()> {
        for instance in self.instances(start)? {
            let runs = self.runs(&instance, start, start + CONTROL_FRAMES);
            if !runs.is_empty() {
                self.play(&instance, &runs)?;
            }
        }
        Ok(())
    }

    /// The root's timeline, and every template instance with audio that the root's output reaches
    /// in the interval from `start`.
    fn instances(&self, start: u64) -> crate::Result<Vec<Instance<'a>>> {
        let root = Scope::root(self.composition);
        let mut instances = Vec::new();
        if let Some(timeline) = self.composition.audio() {
            instances.push(Instance {
                key: Vec::new(),
                chain: Vec::new(),
                scope: root.clone(),
                timeline,
            });
        }
        if self.audible.is_empty() {
            return Ok(instances);
        }
        let Some(source) = output_source(&root) else {
            return Ok(instances);
        };
        let rate = self.root_fps / SAMPLE_RATE;
        let mut stack = vec![Walk {
            scope: root,
            node: source,
            frame: start as f64 * rate,
            rate,
            chain: Vec::new(),
            key: Vec::new(),
            depth: 1,
        }];
        let mut seen = HashSet::new();
        while let Some(walk) = stack.pop() {
            if walk.depth > MAX_GRAPH_DEPTH {
                return Err(walk.scope.too_deep(walk.node));
            }
            if !seen.insert((walk.key.clone(), walk.node)) {
                continue;
            }
            match walk.scope.node(walk.node) {
                None => {}
                Some(NodeKind::Clip(clip)) => {
                    self.enter_clip(clip, &walk, &mut stack, &mut instances)?;
                }
                Some(NodeKind::TimeRemap(remap)) => {
                    self.enter_time_remap(remap, &walk, &mut stack)?
                }
                Some(node) => {
                    let inputs = route(&walk.scope, node, walk.frame.into(), self.media, true)?;
                    stack.extend(inputs.into_iter().map(|input| Walk {
                        node: input.node,
                        depth: walk.depth + 1,
                        ..walk.clone()
                    }));
                }
            }
        }
        Ok(instances)
    }

    /// Follows a clip into what it plays, when its window overlaps the interval.
    fn enter_clip(
        &self,
        clip: &Clip,
        walk: &Walk<'a>,
        stack: &mut Vec<Walk<'a>>,
        instances: &mut Vec<Instance<'a>>,
    ) -> crate::Result<()> {
        let ctx = walk.scope.expr_context(walk.frame.into(), self.media);
        let window = clip.window(&walk.scope, &ctx)?;
        let interval_end = walk.frame + walk.rate * CONTROL_FRAMES as f64;
        let reaches = window.duration > 0.0
            && interval_end >= window.start
            && (window.end == ClipEnd::Loop || walk.frame < window.start + window.duration);
        if !reaches {
            return Ok(());
        }
        let mut elapsed = (walk.frame - window.start).max(0.0);
        if elapsed >= window.duration && window.end == ClipEnd::Loop {
            elapsed = elapsed.rem_euclid(window.duration);
        }
        let mut next = Walk {
            scope: walk.scope.clone(),
            node: walk.node,
            frame: window.offset + elapsed * window.rate(),
            rate: walk.rate * window.rate(),
            chain: [walk.chain.as_slice(), &[Link::Window(window)]].concat(),
            key: [walk.key.as_slice(), &[clip.id]].concat(),
            depth: walk.depth + 1,
        };
        let Some((template, output)) = clip.template(&walk.scope)? else {
            if let Some(source) = clip.source {
                stack.push(Walk {
                    node: source,
                    ..next
                });
            }
            return Ok(());
        };
        if !clip
            .template_name()
            .is_some_and(|name| self.audible.contains(name))
        {
            return Ok(());
        }
        let props = clip.instance_props(template, &ctx)?;
        next.scope = Scope::template(self.composition, template, output, props);
        if let Some(timeline) = &template.audio {
            instances.push(Instance {
                key: next.key.clone(),
                chain: next.chain.clone(),
                scope: next.scope.clone(),
                timeline,
            });
        }
        if let Some(source) = output_source(&next.scope) {
            stack.push(Walk {
                node: source,
                ..next
            });
        }
        Ok(())
    }

    /// Follows a time remap into its source, where the remapped time moves forward.
    fn enter_time_remap(
        &self,
        remap: &TimeRemap,
        walk: &Walk<'a>,
        stack: &mut Vec<Walk<'a>>,
    ) -> crate::Result<()> {
        let target = |frame: f64| {
            let params = remap.params.eval(&DelegateEvalContext {
                node_id: remap.id,
                expr: &walk.scope.expr_context(frame.into(), self.media),
            })?;
            remap_frame(remap.id, &params)
        };
        let span = walk.rate * CONTROL_FRAMES as f64;
        let frame = target(walk.frame)?;
        let rate = (target(walk.frame + span)? - frame) / span;
        if rate <= 0.0 {
            return Ok(());
        }
        stack.push(Walk {
            scope: walk.scope.clone(),
            node: required_input(remap.id, "source", remap.source)?,
            frame,
            rate: walk.rate * rate,
            chain: [
                walk.chain.as_slice(),
                &[Link::Remap {
                    at: walk.frame,
                    frame,
                    rate,
                }],
            ]
            .concat(),
            key: [walk.key.as_slice(), &[remap.id]].concat(),
            depth: walk.depth + 1,
        });
        Ok(())
    }

    /// Where root sample `sample` lands in `instance`, or `None` where nothing of it sounds.
    fn trace(&self, instance: &Instance<'_>, sample: u64) -> Option<Trace> {
        if instance.chain.is_empty() {
            return Some(Trace {
                position: sample as f64,
                step: 1.0,
                from: f64::NEG_INFINITY,
                to: f64::INFINITY,
            });
        }
        let mut frame = sample as f64 * self.root_fps / SAMPLE_RATE;
        let (mut from, mut to, mut rate) = (f64::NEG_INFINITY, f64::INFINITY, 1.0);
        for link in &instance.chain {
            let piece = link.piece(frame)?;
            frame = piece.map(frame);
            from = piece.map(from).max(piece.from);
            to = piece.map(to).min(piece.to);
            rate *= piece.rate;
        }
        let content_fps = f64::from(instance.scope.scene.timeline.fps);
        let samples = SAMPLE_RATE / content_fps;
        Some(Trace {
            position: frame * samples,
            step: rate * self.root_fps / content_fps,
            from: from * samples,
            to: to * samples,
        })
    }

    /// The lines root samples `[start, end)` follow in `instance`. A window starting, ending or
    /// looping within them starts a new one.
    fn runs(&self, instance: &Instance<'_>, start: u64, end: u64) -> Vec<Run> {
        let last = end - 1;
        if let (Some(first), Some(last_trace)) =
            (self.trace(instance, start), self.trace(instance, last))
            && first.continues_to(&last_trace, last - start)
        {
            return vec![Run {
                start,
                frames: end - start,
                trace: first,
            }];
        }
        let mut runs: Vec<Run> = Vec::new();
        let mut open = false;
        for sample in start..end {
            let Some(trace) = self.trace(instance, sample) else {
                open = false;
                continue;
            };
            match runs.last_mut() {
                Some(run) if open && run.trace.continues_to(&trace, sample - run.start) => {
                    run.frames += 1;
                }
                _ => {
                    runs.push(Run {
                        start: sample,
                        frames: 1,
                        trace,
                    });
                    open = true;
                }
            }
        }
        runs
    }

    /// Adds the segments each audible clip of `instance` plays along `runs`.
    fn play(&mut self, instance: &Instance<'a>, runs: &[Run]) -> crate::Result<()> {
        let timeline = instance.timeline;
        let has_solo = timeline.tracks.iter().any(|track| track.solo);
        for (index, clip) in timeline.clips.iter().enumerate() {
            let track = timeline
                .tracks
                .iter()
                .find(|track| track.id == clip.track_id)
                .ok_or_else(|| MediaError::UnknownAudioTrack {
                    clip: clip.id.clone(),
                    track: clip.track_id.clone(),
                })?;
            if track.muted
                || (has_solo && !track.solo)
                || track.volume.is_silent()
                || clip.volume.is_silent()
            {
                continue;
            }
            let clip_start = ms_to_sample(clip.start_ms) as f64;
            let duration = ms_to_sample(clip.duration_ms).max(1) as f64;
            for run in runs {
                let Trace {
                    position,
                    step,
                    from,
                    to,
                } = run.trace;
                let position = position - clip_start;
                let from = (from - clip_start).max(0.0);
                let to = (to - clip_start).min(duration);
                // The samples of the run whose positions are in `[from, to)`.
                let first = ((from - position) / step).ceil().max(0.0);
                let end = ((to - position) / step).ceil().min(run.frames as f64);
                if !(from < to && first < end) {
                    continue;
                }
                let frames = (end - first) as u64;
                let position = position + step * first;
                let gain = [
                    self.gain(instance, track, clip, clip_start + position)?,
                    self.gain(
                        instance,
                        track,
                        clip,
                        clip_start + position + step * frames as f64,
                    )?,
                ];
                let voice = self.voice(instance, index, clip);
                self.push(AudioSegment {
                    voice,
                    start: run.start + first as u64,
                    frames,
                    position,
                    step,
                    from,
                    to,
                    gain,
                });
            }
        }
        Ok(())
    }

    /// The track's and the clip's gain multiplied, at sample `sample` of the instance.
    fn gain(
        &self,
        instance: &Instance<'_>,
        track: &AudioTrack,
        clip: &AudioClip,
        sample: f64,
    ) -> crate::Result<f64> {
        let frame = sample * f64::from(instance.scope.scene.timeline.fps) / SAMPLE_RATE;
        let ctx = instance
            .scope
            .expr_context(FrameTime::from(frame), self.media);
        let located = |what: String| {
            move |error: LumenError| {
                let error = LumenError::evaluate(format!("audio {what} volume: {error}"));
                match instance.scope.template_name() {
                    Some(template) => {
                        LumenError::evaluate(format!("composition `{template}`: {error}"))
                    }
                    None => error,
                }
            }
        };
        let track_gain = track
            .volume
            .eval(&ctx)
            .map_err(located(format!("track `{}`", track.id)))?;
        let clip_gain = clip
            .volume
            .eval(&ctx)
            .map_err(located(format!("clip `{}`", clip.id)))?;
        Ok(track_gain * clip_gain)
    }

    fn voice(&mut self, instance: &Instance<'_>, index: usize, clip: &AudioClip) -> usize {
        let voices = &mut self.plan.voices;
        *self
            .voices
            .entry((instance.key.clone(), index))
            .or_insert_with(|| {
                let duration = ms_to_sample(clip.duration_ms).max(1) as f64;
                voices.push(AudioVoice {
                    source_id: clip.source_id.clone(),
                    source_start: ms_to_sample(clip.source_start_ms) as f64,
                    duration,
                    fade_in: (ms_to_sample(clip.fade_in_ms) as f64).min(duration),
                    fade_out: (ms_to_sample(clip.fade_out_ms) as f64).min(duration),
                });
                voices.len() - 1
            })
    }

    /// Adds `segment`, extending the voice's last segment instead when it continues it at the
    /// same fixed gain.
    fn push(&mut self, segment: AudioSegment) {
        if let Some(&last) = self.last_segments.get(&segment.voice) {
            let previous = &mut self.plan.segments[last];
            let continues = previous.start + previous.frames == segment.start
                && previous.step == segment.step
                && previous.from == segment.from
                && previous.to == segment.to
                && previous.gain[0] == previous.gain[1]
                && previous.gain == segment.gain
                && segment.gain[0] == segment.gain[1]
                && (previous.position + previous.step * previous.frames as f64 - segment.position)
                    .abs()
                    <= 1e-6;
            if continues {
                previous.frames += segment.frames;
                return;
            }
        }
        self.last_segments
            .insert(segment.voice, self.plan.segments.len());
        self.plan.segments.push(segment);
    }
}

/// The node a scope's `media_output` shows.
fn output_source(scope: &Scope<'_>) -> Option<NodeId> {
    match scope.node(scope.output) {
        Some(NodeKind::MediaOutput(output)) => output.source,
        _ => None,
    }
}

/// Templates with audio clips, and templates with a `clip` that plays one of them.
fn audible_templates(composition: &Composition) -> HashSet<&str> {
    let templates = composition.compositions();
    let mut audible: HashSet<&str> = templates
        .iter()
        .filter(|(_, template)| {
            template
                .audio
                .as_ref()
                .is_some_and(|audio| !audio.clips.is_empty())
        })
        .map(|(name, _)| name.as_str())
        .collect();
    loop {
        let found = templates
            .iter()
            .filter(|(name, template)| {
                !audible.contains(name.as_str())
                    && template.scene.graph.nodes.values().any(|node| match node {
                        NodeKind::Clip(clip) => clip
                            .template_name()
                            .is_some_and(|name| audible.contains(name)),
                        _ => false,
                    })
            })
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>();
        if found.is_empty() {
            return audible;
        }
        audible.extend(found);
    }
}
