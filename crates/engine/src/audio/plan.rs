//! What each audio clip plays where, in root samples, with its gains evaluated.
//!
//! A clip of the root's timeline plays at its own time. A template's clip plays wherever a `clip`
//! node reaching the root's output plays the template, at the template time that node shows, so
//! its start, duration, offset, speed, `hold` and `loop` place the sound as they place the
//! frames, through any depth of nested clips. Speed resamples, so pitch follows it. A `switch`
//! plays only the selected layer's audio, and a `time_remap` plays its source's audio at the
//! remapped time; where the remapped time holds or runs backwards it is silent, as a `hold` past
//! a clip's window is.

use std::{
    borrow::Cow,
    collections::{HashMap, HashSet},
    rc::Rc,
};

use serde::Serialize;

use super::{AUDIO_SAMPLE_RATE, AudioClip, AudioTimeline, AudioTrack, ms_to_sample};
use crate::{
    composition::{Composition, TemplateRef},
    error::{LumenError, MediaError},
    media::{MediaLocation, MediaStore},
    node::{
        ClipPropValue, DelegateEvalContext, DelegateEvaluable, NodeId, NodeKind,
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

/// Most clips and time remaps one control interval may be inside at once while finding what
/// sounds, counting each restart of a loop in it separately.
pub const MAX_ACTIVE_CLIPS: usize = 1024;

/// Most clips of an audio timeline one control interval may sound.
pub const MAX_ACTIVE_VOICES: usize = 1024;

/// Most times a looping clip or time remap may restart within one control interval.
pub const MAX_LOOP_RESTARTS: u64 = 32;

/// Most segments one call of [`plan`] may return, 36 MB of segments. The server plans 30 s at a
/// time, so this allows about 178 clips with animated gains sounding throughout.
pub const MAX_PLAN_SEGMENTS: usize = 500_000;

/// Most segments the browser preview's plan of the whole timeline may hold.
pub const MAX_PREVIEW_SEGMENTS: usize = 200_000;

/// Most control intervals the browser preview's plan may cover, about 3 hours.
pub const MAX_PREVIEW_INTERVALS: u64 = 1 << 20;

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
    /// Where the composition has the clip, for the failure of a source that can't be read.
    #[serde(skip)]
    pub at: MediaLocation,
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
/// what expressions that measure text use. A plan of more than [`MAX_PLAN_SEGMENTS`] segments
/// fails instead of growing; plan shorter ranges.
pub fn plan(
    composition: &Composition,
    media: Option<&dyn MediaStore>,
    start: u64,
    frames: u64,
) -> crate::Result<AudioPlan> {
    plan_within(composition, media, start, frames, MAX_PLAN_SEGMENTS)
}

pub(super) fn plan_within(
    composition: &Composition,
    media: Option<&dyn MediaStore>,
    start: u64,
    frames: u64,
    max_segments: usize,
) -> crate::Result<AudioPlan> {
    if frames == 0 || !composition.has_audio() {
        return Ok(AudioPlan::default());
    }
    let mut planner = Planner::new(composition, media);
    let last = start.saturating_add(frames - 1) / CONTROL_FRAMES;
    for interval in start / CONTROL_FRAMES..=last {
        planner.interval(interval * CONTROL_FRAMES)?;
        if planner.plan.segments.len() > max_segments {
            return Err(limit(format!(
                "planning samples {start} to {} makes more than {max_segments} segments by sample {}. Animated gains split the sound into a segment every {CONTROL_FRAMES} samples, so animate fewer clips or mix a shorter range",
                start.saturating_add(frames),
                interval * CONTROL_FRAMES,
            )));
        }
    }
    Ok(planner.plan)
}

#[cfg(test)]
pub(super) fn clips_examined(
    composition: &Composition,
    start: u64,
    frames: u64,
) -> crate::Result<usize> {
    let mut planner = Planner::new(composition, None);
    for interval in start / CONTROL_FRAMES..=start.saturating_add(frames - 1) / CONTROL_FRAMES {
        planner.interval(interval * CONTROL_FRAMES)?;
    }
    Ok(planner.examined)
}

/// What sounds over the root's whole timeline, which the browser preview plays and loops. A
/// timeline of more than [`MAX_PREVIEW_INTERVALS`] control intervals, or sounds that make more
/// than [`MAX_PREVIEW_SEGMENTS`] segments, fail instead of building a plan that large.
pub fn preview_plan(
    composition: &Composition,
    media: Option<&dyn MediaStore>,
) -> crate::Result<AudioPlan> {
    preview_plan_within(composition, media, MAX_PREVIEW_SEGMENTS)
}

pub(super) fn preview_plan_within(
    composition: &Composition,
    media: Option<&dyn MediaStore>,
    max_segments: usize,
) -> crate::Result<AudioPlan> {
    if !composition.has_audio() {
        return Ok(AudioPlan::default());
    }
    let timeline = composition.timeline();
    let seconds = f64::from(timeline.duration_frames) / f64::from(timeline.fps);
    let intervals = ((seconds * SAMPLE_RATE).ceil() as u64).div_ceil(CONTROL_FRAMES);
    if intervals > MAX_PREVIEW_INTERVALS {
        return Err(limit(format!(
            "the preview plans at most {MAX_PREVIEW_INTERVALS} control intervals of {CONTROL_FRAMES} samples ({:.0} s), and this timeline is {seconds:.0} s",
            (MAX_PREVIEW_INTERVALS * CONTROL_FRAMES) as f64 / SAMPLE_RATE,
        )));
    }
    let mut planner = Planner::new(composition, media);
    for interval in 0..intervals {
        planner.interval(interval * CONTROL_FRAMES)?;
        if planner.plan.segments.len() > max_segments {
            return Err(limit(format!(
                "the preview plan of the whole timeline has more than {max_segments} segments by {:.0} s. Animated gains split the sound into a segment every {CONTROL_FRAMES} samples, so animate fewer clips or shorten the composition",
                ((interval + 1) * CONTROL_FRAMES) as f64 / SAMPLE_RATE,
            )));
        }
    }
    Ok(planner.plan)
}

fn limit(details: String) -> LumenError {
    MediaError::AudioLimit { details }.into()
}

struct Planner<'a> {
    composition: &'a Composition,
    media: Option<&'a dyn MediaStore>,
    root_fps: f64,
    /// Templates with audio, or that play a template with audio.
    audible: HashSet<&'a str>,
    root_index: Option<Rc<TimelineIndex>>,
    template_indexes: HashMap<&'a str, Rc<TimelineIndex>>,
    voices: HashMap<(Vec<NodeId>, usize), usize>,
    /// The last segment of each voice, which the next may extend.
    last_segments: HashMap<usize, usize>,
    /// Clips sounding in the interval being planned.
    sounding: usize,
    /// Clips of audio timelines looked at so far, to check an interval's work follows what sounds.
    examined: usize,
    plan: AudioPlan,
}

/// What an audio timeline's clips need looked up quickly, built once for the timeline.
struct TimelineIndex {
    /// The track each clip is on, as an index into the timeline's tracks.
    tracks: Vec<Option<usize>>,
    has_solo: bool,
    /// Clip indices by start. A binary tree is laid over them with each range's middle as its
    /// node, so a query skips every range whose clips all end before it.
    by_start: Vec<usize>,
    /// The start and end sample of the clip at each place of `by_start`.
    spans: Vec<(f64, f64)>,
    /// The latest end among the clips of the range each place of `by_start` is the middle of.
    latest_end: Vec<f64>,
}

impl TimelineIndex {
    fn new(timeline: &AudioTimeline) -> Self {
        let mut ids: HashMap<&str, usize> = HashMap::new();
        for (index, track) in timeline.tracks.iter().enumerate() {
            ids.entry(track.id.as_str()).or_insert(index);
        }
        let tracks = timeline
            .clips
            .iter()
            .map(|clip| ids.get(clip.track_id.as_str()).copied())
            .collect();
        let start = |clip: &AudioClip| ms_to_sample(clip.start_ms);
        let mut by_start: Vec<usize> = (0..timeline.clips.len()).collect();
        by_start.sort_by_key(|&index| (start(&timeline.clips[index]), index));
        let spans: Vec<(f64, f64)> = by_start
            .iter()
            .map(|&index| {
                let clip = &timeline.clips[index];
                let begin = start(clip) as f64;
                (begin, begin + ms_to_sample(clip.duration_ms).max(1) as f64)
            })
            .collect();
        let mut latest_end = vec![0.0; spans.len()];
        fn fill(spans: &[(f64, f64)], latest_end: &mut [f64], from: usize, to: usize) -> f64 {
            if from >= to {
                return f64::NEG_INFINITY;
            }
            let middle = from + (to - from) / 2;
            let latest = spans[middle]
                .1
                .max(fill(spans, latest_end, from, middle))
                .max(fill(spans, latest_end, middle + 1, to));
            latest_end[middle] = latest;
            latest
        }
        fill(&spans, &mut latest_end, 0, spans.len());
        Self {
            tracks,
            has_solo: timeline.tracks.iter().any(|track| track.solo),
            by_start,
            spans,
            latest_end,
        }
    }

    /// The clips, in timeline order, that may play somewhere in `[low, high]`, and how many
    /// places of the tree the search looked at.
    fn overlapping(&self, low: f64, high: f64, looked_at: &mut usize) -> Vec<usize> {
        let mut found = Vec::new();
        let mut ranges = vec![(0, self.spans.len())];
        while let Some((from, to)) = ranges.pop() {
            if from >= to {
                continue;
            }
            let middle = from + (to - from) / 2;
            if self.latest_end[middle] < low {
                continue;
            }
            *looked_at += 1;
            let (start, end) = self.spans[middle];
            ranges.push((from, middle));
            if start <= high {
                if end >= low {
                    found.push(self.by_start[middle]);
                }
                ranges.push((middle + 1, to));
            }
        }
        found.sort_unstable();
        found
    }
}

/// A timeline and how root time reaches it.
struct Instance<'a> {
    /// The `clip` and `time_remap` nodes on the way from the root, which tell instances apart.
    key: Vec<NodeId>,
    chain: Vec<Link<'a>>,
    scope: Scope<'a>,
    timeline: &'a AudioTimeline,
    index: Rc<TimelineIndex>,
    /// Whether a clip on the way sets a prop by expression, so the scope's props change with time.
    animated: bool,
}

/// What finding the instances of one control interval has found and entered so far.
struct Found<'a> {
    /// The first root sample of the interval.
    start: u64,
    stack: Vec<Walk<'a>>,
    instances: Vec<Instance<'a>>,
    entered: usize,
}

impl Found<'_> {
    /// Counts a clip or time remap entered, in the one place that bounds how many there are.
    fn enter(&mut self) -> crate::Result<()> {
        self.entered += 1;
        if self.entered > MAX_ACTIVE_CLIPS {
            return Err(limit(format!(
                "more than {MAX_ACTIVE_CLIPS} clips and time remaps are playing sound at once around sample {}, counting each restart of a loop. Play fewer nested clips at a time",
                self.start
            )));
        }
        Ok(())
    }
}

/// A node reached from the root's output while finding instances.
#[derive(Clone)]
struct Walk<'a> {
    scope: Scope<'a>,
    node: NodeId,
    /// The frame of `scope` the node is evaluated at, for the first sample it plays.
    frame: f64,
    /// Root samples from there to where it stops playing within the interval, at least 1.
    samples: u64,
    /// Frames of `scope` per root sample.
    rate: f64,
    chain: Vec<Link<'a>>,
    /// The `clip` and `time_remap` nodes on the way and which restart of its loop each is in.
    path: Vec<(NodeId, i64)>,
    depth: u32,
}

/// The template a `clip` plays, and the clip that sets its props.
#[derive(Clone, Copy)]
struct Plays<'a> {
    clip: &'a Clip,
    template: TemplateRef<'a>,
}

/// A looping `time_remap`: frames `[start + repeat * length, start + (repeat + 1) * length)` of
/// its target are this restart.
#[derive(Clone, Copy)]
struct RemapLoop {
    start: f64,
    length: f64,
    repeat: i64,
}

impl RemapLoop {
    fn repeat_of(&self, target: f64) -> i64 {
        ((target.floor() - self.start) / self.length).floor() as i64
    }
}

/// How one step of the way maps the frames of the composition above to the frames below. A
/// looping step is for one restart of its loop, and holds where another restart is.
#[derive(Clone, Copy)]
enum Link<'a> {
    Window {
        window: ClipWindow,
        repeat: i64,
        /// The template the clip plays, or `None` when it plays a `source` of its own scope.
        plays: Option<Plays<'a>>,
    },
    /// A `time_remap`'s target, linear around where it was evaluated: `target + rate * (t - at)`
    /// before the loop folds it into the loop's frames.
    Remap {
        at: f64,
        target: f64,
        rate: f64,
        looping: Option<RemapLoop>,
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

impl Link<'_> {
    /// The piece that plays at `frame`, or `None` where nothing sounds.
    fn piece(&self, frame: f64) -> Option<Piece> {
        match *self {
            Self::Window { window, repeat, .. } => {
                let elapsed = frame - window.start;
                if !(elapsed >= 0.0 && window.duration > 0.0) {
                    return None;
                }
                let current = if elapsed < window.duration {
                    0.0
                } else if window.end == ClipEnd::Loop {
                    (elapsed / window.duration).floor()
                } else {
                    return None;
                };
                if current != repeat as f64 {
                    return None;
                }
                Some(Piece {
                    base_in: window.start + current * window.duration,
                    base_out: window.offset,
                    rate: window.rate(),
                    from: window.offset,
                    to: window.offset + window.duration * window.rate(),
                })
            }
            Self::Remap {
                at,
                target,
                rate,
                looping,
            } => {
                let value = target + (frame - at) * rate;
                // The remap holds frame 0 below it.
                if value < 0.0 {
                    return None;
                }
                let folded = match looping {
                    Some(looping) if looping.repeat_of(value) != looping.repeat => return None,
                    Some(looping) => looping.repeat as f64 * looping.length,
                    None => 0.0,
                };
                Some(Piece {
                    base_in: at,
                    base_out: target - folded,
                    rate,
                    from: f64::NEG_INFINITY,
                    to: f64::INFINITY,
                })
            }
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
            root_index: composition
                .audio()
                .map(|timeline| Rc::new(TimelineIndex::new(timeline))),
            template_indexes: composition
                .compositions()
                .iter()
                .filter_map(|(name, template)| {
                    let timeline = template.audio.as_ref()?;
                    Some((name.as_str(), Rc::new(TimelineIndex::new(timeline))))
                })
                .collect(),
            voices: HashMap::new(),
            last_segments: HashMap::new(),
            sounding: 0,
            examined: 0,
            plan: AudioPlan::default(),
        }
    }

    /// Plans root samples `[start, start + CONTROL_FRAMES)`.
    fn interval(&mut self, start: u64) -> crate::Result<()> {
        self.sounding = 0;
        for instance in self.instances(start)? {
            let runs = self.runs(&instance, start, start.saturating_add(CONTROL_FRAMES));
            if !runs.is_empty() {
                self.play(&instance, &runs)?;
            }
        }
        Ok(())
    }

    /// The root's timeline, and every template instance with audio that the root's output reaches
    /// in the interval from `start`. A looping clip or time remap that restarts in the interval
    /// is one instance for each restart, which only sounds in its own part of the interval.
    fn instances(&self, start: u64) -> crate::Result<Vec<Instance<'a>>> {
        let root = Scope::root(self.composition);
        let mut found = Found {
            start,
            stack: Vec::new(),
            instances: Vec::new(),
            entered: 0,
        };
        if let (Some(timeline), Some(index)) = (self.composition.audio(), &self.root_index) {
            found.instances.push(Instance::new(
                Vec::new(),
                Vec::new(),
                root.clone(),
                timeline,
                Rc::clone(index),
            ));
        }
        if self.audible.is_empty() {
            return Ok(found.instances);
        }
        let Some(source) = output_source(&root) else {
            return Ok(found.instances);
        };
        let rate = self.root_fps / SAMPLE_RATE;
        found.stack.push(Walk {
            scope: root,
            node: source,
            frame: start as f64 * rate,
            samples: CONTROL_FRAMES,
            rate,
            chain: Vec::new(),
            path: Vec::new(),
            depth: 1,
        });
        let mut seen = HashSet::new();
        while let Some(walk) = found.stack.pop() {
            if walk.depth > MAX_GRAPH_DEPTH {
                return Err(walk.scope.too_deep(walk.node));
            }
            if !seen.insert((walk.path.clone(), walk.node)) {
                continue;
            }
            match walk.scope.node(walk.node) {
                None => {}
                Some(NodeKind::Clip(clip)) => self.enter_clip(clip, &walk, &mut found)?,
                Some(NodeKind::TimeRemap(remap)) => {
                    self.enter_time_remap(remap, &walk, &mut found)?
                }
                Some(node) => {
                    let inputs = route(&walk.scope, node, walk.frame.into(), self.media, true)?;
                    found.stack.extend(inputs.into_iter().map(|input| Walk {
                        node: input.node,
                        depth: walk.depth + 1,
                        ..walk.clone()
                    }));
                }
            }
        }
        Ok(found.instances)
    }

    /// Follows a clip into what it plays, for each restart of its loop that the interval is in.
    fn enter_clip(
        &self,
        clip: &'a Clip,
        walk: &Walk<'a>,
        found: &mut Found<'a>,
    ) -> crate::Result<()> {
        let ctx = walk.scope.expr_context(walk.frame.into(), self.media);
        let window = clip.window(&walk.scope, &ctx)?;
        let samples = walk.samples as f64;
        let elapsed = walk.frame - window.start;
        let reaches = window.duration > 0.0
            && elapsed + walk.rate * samples >= 0.0
            && (window.end == ClipEnd::Loop || elapsed < window.duration);
        if !reaches {
            return Ok(());
        }
        let template = clip.template(&walk.scope)?;
        match template {
            Some(_)
                if !clip
                    .template_name()
                    .is_some_and(|name| self.audible.contains(name)) =>
            {
                return Ok(());
            }
            None if clip.source.is_none() => return Ok(()),
            _ => {}
        }
        let (first, last) = if window.end == ClipEnd::Loop {
            let first = (elapsed / window.duration).floor().max(0.0);
            let at_last = elapsed + walk.rate * (samples - 1.0);
            (first, (at_last / window.duration).floor().max(first))
        } else {
            (0.0, 0.0)
        };
        if last - first >= MAX_LOOP_RESTARTS as f64 {
            return Err(limit(format!(
                "clip {} restarts its loop more than {MAX_LOOP_RESTARTS} times within {CONTROL_FRAMES} samples of audio around sample {}. Loop a longer window",
                clip.id, found.start
            )));
        }
        // A restart reads the clip's props where it begins.
        let props_at = |frame: f64| {
            template
                .map(|template| {
                    let ctx = walk.scope.expr_context(frame.into(), self.media);
                    clip.instance_props(template.template, &ctx)
                })
                .transpose()
        };
        // The sample of the walk where a restart begins, or after its last if it begins later.
        let begins = |repeat: i64| {
            let ahead = repeat as f64 * window.duration - elapsed;
            if ahead > 0.0 && walk.rate > 0.0 {
                (ahead / walk.rate).floor().min(samples)
            } else {
                0.0
            }
        };
        for repeat in first as i64..=last as i64 {
            let begin = begins(repeat);
            if begin >= samples {
                continue;
            }
            let end = if repeat < last as i64 {
                (begins(repeat + 1) + 1.0).min(samples)
            } else {
                samples
            };
            found.enter()?;
            let into = (walk.frame - (window.start + repeat as f64 * window.duration)).max(0.0);
            let mut next = Walk {
                scope: walk.scope.clone(),
                node: walk.node,
                frame: window.offset + into * window.rate(),
                samples: (end - begin).max(1.0) as u64,
                rate: walk.rate * window.rate(),
                chain: [
                    walk.chain.as_slice(),
                    &[Link::Window {
                        window,
                        repeat,
                        plays: template.map(|template| Plays { clip, template }),
                    }],
                ]
                .concat(),
                path: [walk.path.as_slice(), &[(clip.id, repeat)]].concat(),
                depth: walk.depth + 1,
            };
            let restart = window.start + repeat as f64 * window.duration;
            let props = props_at(if begin > 0.0 { restart } else { walk.frame })?;
            let (Some(template), Some(props)) = (template, props) else {
                if let Some(source) = clip.source {
                    found.stack.push(Walk {
                        node: source,
                        ..next
                    });
                }
                continue;
            };
            next.scope = Scope::template(self.composition, template, props);
            if let (Some(timeline), Some(index)) = (
                &template.template.audio,
                clip.template_name()
                    .and_then(|name| self.template_indexes.get(name)),
            ) {
                found.instances.push(Instance::new(
                    next.path.iter().map(|(id, _)| *id).collect(),
                    next.chain.clone(),
                    next.scope.clone(),
                    timeline,
                    Rc::clone(index),
                ));
            }
            if let Some(source) = output_source(&next.scope) {
                found.stack.push(Walk {
                    node: source,
                    ..next
                });
            }
        }
        Ok(())
    }

    /// Follows a time remap into its source, for each restart of its loop that the interval is
    /// in, where the remapped time moves forward.
    fn enter_time_remap(
        &self,
        remap: &'a TimeRemap,
        walk: &Walk<'a>,
        found: &mut Found<'a>,
    ) -> crate::Result<()> {
        let params_at = |frame: f64| {
            let params = remap.params.eval(&DelegateEvalContext {
                node_id: remap.id,
                expr: &walk.scope.expr_context(frame.into(), self.media),
            })?;
            remap_frame(remap.id, &params)?;
            crate::Result::Ok(params)
        };
        let samples = walk.samples as f64;
        let span = walk.rate * samples;
        let first = params_at(walk.frame)?;
        let rate = (params_at(walk.frame + span)?.frame - first.frame) / span;
        let at_last = first.frame + rate * walk.rate * (samples - 1.0);
        if !(rate > 0.0 && at_last >= 0.0) {
            return Ok(());
        }
        let looping = first.loop_range().map(|(start, length)| RemapLoop {
            start: start as f64,
            length: length as f64,
            repeat: 0,
        });
        let (from, to) = looping.map_or((0, 0), |looping| {
            (
                looping.repeat_of(first.frame.max(0.0)),
                looping.repeat_of(at_last),
            )
        });
        if to.saturating_sub(from) >= MAX_LOOP_RESTARTS as i64 {
            return Err(limit(format!(
                "time_remap node {} restarts its loop more than {MAX_LOOP_RESTARTS} times within {CONTROL_FRAMES} samples of audio around sample {}. Loop a longer range or play it slower",
                remap.id, found.start
            )));
        }
        let source = required_input(remap.id, "source", remap.source)?;
        // The target frame where a restart's content begins, which is where it is evaluated.
        let content_start = |repeat: i64| {
            let loop_start = looping.map_or(0.0, |looping| {
                looping.start + repeat as f64 * looping.length
            });
            loop_start.max(0.0).max(first.frame)
        };
        // The sample of the walk where a restart begins.
        let begins = |repeat: i64| {
            ((content_start(repeat) - first.frame) / (rate * walk.rate))
                .floor()
                .min(samples)
        };
        for repeat in from..=to {
            let begin = begins(repeat);
            if begin >= samples {
                continue;
            }
            let end = if repeat < to {
                (begins(repeat + 1) + 1.0).min(samples)
            } else {
                samples
            };
            found.enter()?;
            let looping = looping.map(|looping| RemapLoop { repeat, ..looping });
            let folded = looping.map_or(0.0, |looping| repeat as f64 * looping.length);
            found.stack.push(Walk {
                scope: walk.scope.clone(),
                node: source,
                frame: content_start(repeat) - folded,
                samples: (end - begin).max(1.0) as u64,
                rate: walk.rate * rate,
                chain: [
                    walk.chain.as_slice(),
                    &[Link::Remap {
                        at: walk.frame,
                        target: first.frame,
                        rate,
                        looping,
                    }],
                ]
                .concat(),
                path: [walk.path.as_slice(), &[(remap.id, repeat)]].concat(),
                depth: walk.depth + 1,
            });
        }
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
        let lookup = &instance.index;
        // The positions of the instance that the runs sound at, which only clips there can play.
        let (low, high) = runs
            .iter()
            .filter_map(|run| {
                let end = run.trace.position + run.trace.step * run.frames as f64;
                let low = run.trace.position.min(end).max(run.trace.from);
                let high = run.trace.position.max(end).min(run.trace.to);
                (low <= high).then_some((low, high))
            })
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(low, high), run| {
                (low.min(run.0), high.max(run.1))
            });
        for index in lookup.overlapping(low, high, &mut self.examined) {
            let clip = &timeline.clips[index];
            let track = lookup.tracks[index]
                .map(|track| &timeline.tracks[track])
                .ok_or_else(|| MediaError::UnknownAudioTrack {
                    clip: clip.id.clone(),
                    track: clip.track_id.clone(),
                })?;
            if track.muted
                || (lookup.has_solo && !track.solo)
                || track.gain.is_silent()
                || clip.gain.is_silent()
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
                let start = run.start + first as u64;
                let scope_at_start = self.scope_at(instance, start, 0)?;
                let scope_at_end = self.scope_at(instance, start, frames)?;
                let gain = [
                    self.gain(&scope_at_start, track, clip, clip_start + position)?,
                    self.gain(
                        &scope_at_end,
                        track,
                        clip,
                        clip_start + position + step * frames as f64,
                    )?,
                ];
                let voice = self.voice(instance, index, clip);
                self.push(AudioSegment {
                    voice,
                    start,
                    frames,
                    position,
                    step,
                    from,
                    to,
                    gain,
                })?;
            }
        }
        Ok(())
    }

    /// The scope of `instance` as it stands `ahead` root samples after root sample `sample`: its
    /// props are what each clip on the way sets at the time that clip is at then.
    fn scope_at<'i>(
        &self,
        instance: &'i Instance<'a>,
        sample: u64,
        ahead: u64,
    ) -> crate::Result<Cow<'i, Scope<'a>>> {
        if !instance.animated {
            return Ok(Cow::Borrowed(&instance.scope));
        }
        let mut rate = self.root_fps / SAMPLE_RATE;
        let mut frame = sample as f64 * rate;
        let mut scope = Scope::root(self.composition);
        for link in &instance.chain {
            let Some(piece) = link.piece(frame) else {
                return Ok(Cow::Borrowed(&instance.scope));
            };
            if let Link::Window {
                plays: Some(plays), ..
            } = link
            {
                let at = FrameTime::from(frame + rate * ahead as f64);
                let props = plays
                    .clip
                    .instance_props(plays.template.template, &scope.expr_context(at, self.media))?;
                scope = Scope::template(self.composition, plays.template, props);
            }
            frame = piece.map(frame);
            rate *= piece.rate;
        }
        Ok(Cow::Owned(scope))
    }

    /// The track's and the clip's gain multiplied, at sample `sample` of the composition `scope`
    /// is the instance of.
    fn gain(
        &self,
        scope: &Scope<'_>,
        track: &AudioTrack,
        clip: &AudioClip,
        sample: f64,
    ) -> crate::Result<f64> {
        let frame = sample * f64::from(scope.scene.timeline.fps) / SAMPLE_RATE;
        let ctx = scope.expr_context(FrameTime::from(frame), self.media);
        let located = |what: String| {
            move |error: LumenError| {
                let error = LumenError::evaluate(format!("audio {what} gain: {error}"));
                match scope.template_name() {
                    Some(template) => {
                        LumenError::evaluate(format!("composition `{template}`: {error}"))
                    }
                    None => error,
                }
            }
        };
        let track_gain = track
            .gain
            .eval(&ctx)
            .map_err(located(format!("track `{}`", track.id)))?;
        let clip_gain = clip
            .gain
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
                    at: MediaLocation::AudioClip {
                        template: instance.scope.template_name().map(str::to_string),
                        track: clip.track_id.clone(),
                        clip: clip.id.clone(),
                        index,
                    },
                });
                voices.len() - 1
            })
    }

    /// Adds `segment`, extending the voice's last segment instead when it continues it at the
    /// same fixed gain.
    fn push(&mut self, segment: AudioSegment) -> crate::Result<()> {
        self.sounding += 1;
        if self.sounding > MAX_ACTIVE_VOICES {
            return Err(limit(format!(
                "more than {MAX_ACTIVE_VOICES} clips are sounding at once around sample {}",
                segment.start
            )));
        }
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
                return Ok(());
            }
        }
        self.last_segments
            .insert(segment.voice, self.plan.segments.len());
        self.plan.segments.push(segment);
        Ok(())
    }
}

impl<'a> Instance<'a> {
    fn new(
        key: Vec<NodeId>,
        chain: Vec<Link<'a>>,
        scope: Scope<'a>,
        timeline: &'a AudioTimeline,
        index: Rc<TimelineIndex>,
    ) -> Self {
        let animated = chain.iter().any(|link| match link {
            Link::Window {
                plays: Some(plays), ..
            } => plays
                .clip
                .props()
                .0
                .values()
                .any(|value| matches!(value, ClipPropValue::Expr(_))),
            _ => false,
        });
        Self {
            key,
            chain,
            scope,
            timeline,
            index,
            animated,
        }
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
