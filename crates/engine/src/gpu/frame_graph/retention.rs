//! Retention: which outputs the walk keeps across frames, for how long, and what drops them.
//!
//! [`Retention`] holds what the walk is inside while it records: static nodes, and nodes kept while
//! their models hold a pose or their params' values hold. The `FrameCx` methods here read a node
//! through what is kept and keep what it records.

use std::sync::Arc;

use foldhash::HashSet;
use lumen_gpu::TextureDomain;

use super::{
    FrameCx, MemoKey, Raster, RecordNode, Staticness,
    state::{Retained, Slot},
};
use crate::{
    composition::SceneId,
    error::RenderError,
    expr::ExpressionValue,
    media::{AssetKind, MediaIdentity, MediaStore},
    node::{Node, NodeId, NodeKind, required_input},
    scope::{FrameTime, MAX_GRAPH_DEPTH},
};

/// The frames a kept output stays correct for, and what it drew.
#[derive(Debug, Clone)]
pub(super) struct HoldTrack {
    /// Both ends included.
    frames: (f64, f64),
    models: Vec<String>,
}

impl HoldTrack {
    fn new() -> Self {
        Self {
            frames: (f64::NEG_INFINITY, f64::INFINITY),
            models: Vec::new(),
        }
    }

    fn narrow(&mut self, frames: (f64, f64)) {
        self.frames = (self.frames.0.max(frames.0), self.frames.1.min(frames.1));
    }

    fn contains(&self, frame: f64) -> bool {
        self.frames.0 <= frame && frame <= self.frames.1
    }

    /// Whether the output this tracked, recorded at `frame`, is kept: when the walk is not inside
    /// a static node, and it stays correct on this frame and on one either side of it, which
    /// `same_at` says of the params' values there.
    fn keeps(&self, frame: f64, at_root: bool, mut same_at: impl FnMut(f64) -> bool) -> bool {
        at_root
            && self.contains(frame)
            && neighbouring_frames(frame).any(|other| self.contains(other) && same_at(other))
    }
}

/// What the walk is inside that keeps outputs, as it records.
#[derive(Debug, Default)]
pub(super) struct Retention {
    /// How many static nodes the walk is inside, whose readers retain them rather than this.
    static_depth: u32,
    /// What the nodes that are static while their models hold a pose or their params' values hold,
    /// which the walk is inside, have found out so far, innermost last.
    holds: Vec<HoldTrack>,
}

impl Retention {
    /// Enters a node being recorded, static when `is_static`, and tracked while it holds when
    /// `tracked`.
    fn enter(&mut self, is_static: bool, tracked: bool) {
        self.static_depth += u32::from(is_static);
        if tracked {
            self.holds.push(HoldTrack::new());
        }
    }

    /// Leaves the node `enter` entered, with what was found while it was tracked.
    fn leave(&mut self, is_static: bool, tracked: bool) -> Option<HoldTrack> {
        self.static_depth -= u32::from(is_static);
        tracked.then(|| self.holds.pop()).flatten()
    }

    /// Whether the walk is inside no static node, so a static node recorded now keeps its own
    /// output.
    fn at_root(&self) -> bool {
        self.static_depth == 0
    }

    /// Whether what is recorded now ends up in a kept output.
    fn keeping(&self) -> bool {
        self.static_depth > 0 || !self.holds.is_empty()
    }

    /// Notes in the output being recorded that it reads one that holds on `track`'s frames.
    fn absorb(&mut self, track: &HoldTrack) {
        if let Some(outer) = self.holds.last_mut() {
            outer.narrow(track.frames);
            outer.models.extend(track.models.iter().cloned());
        }
    }

    /// Notes in the output being recorded that it reads the kept output `key`, which holds only
    /// on the frames the slot is correct on.
    fn hold_slot(&mut self, retained: &Retained, key: (SceneId, NodeId)) {
        let Some(track) = self.holds.last_mut() else {
            return;
        };
        if let Some(slot) = retained.slots.get(&key) {
            track.narrow(slot.frames);
            track.models.extend(slot.models.iter().cloned());
        }
    }

    #[cfg(feature = "3d")]
    fn drew_model(&mut self, retained: &mut Retained, id: &str) {
        if self.static_depth > 0 {
            retained.static_models.push(id.to_string());
        }
        if let Some(track) = self.holds.last_mut() {
            track.models.push(id.to_string());
        }
    }

    #[cfg(feature = "3d")]
    fn held_on(&mut self, frames: (f64, f64)) {
        if let Some(track) = self.holds.last_mut() {
            track.narrow(frames);
        }
    }
}

/// Whether `a` and `b` are the same values to every param, which tells `0` from `-0` and holds for
/// two NaNs of the same bits, as the same inputs give the same output.
fn same_values(a: &[ExpressionValue], b: &[ExpressionValue]) -> bool {
    fn same_bits(a: &[f64], b: &[f64]) -> bool {
        a.iter().zip(b).all(|(a, b)| a.to_bits() == b.to_bits())
    }
    a.len() == b.len()
        && a.iter().zip(b).all(|pair| match pair {
            (ExpressionValue::Number(a), ExpressionValue::Number(b)) => a.to_bits() == b.to_bits(),
            (ExpressionValue::Vec2((ax, ay)), ExpressionValue::Vec2((bx, by))) => {
                same_bits(&[*ax, *ay], &[*bx, *by])
            }
            (ExpressionValue::Vec3(a), ExpressionValue::Vec3(b)) => same_bits(a, b),
            (ExpressionValue::Color(a), ExpressionValue::Color(b)) => same_bits(a, b),
            (a, b) => a == b,
        })
}

/// The frames either side of `frame` among those a composition has.
fn neighbouring_frames(frame: f64) -> impl Iterator<Item = f64> {
    [frame.floor() + 1.0, frame.ceil() - 1.0]
        .into_iter()
        .filter(|other| *other >= 0.0)
}

/// The identity of what `media` serves for `source` as `kind`, if it serves anything.
fn source_identity(
    media: Option<&dyn MediaStore>,
    kind: AssetKind,
    source: &str,
) -> Option<MediaIdentity> {
    Some(media?.resolve(source, kind).ok()?.identity)
}

/// Whether what `media` serves for `source` as `kind` is no longer what was `seen`: another
/// identity, or there where it was missing or the other way round. A source that fails for any
/// other reason than being missing counts as changed, so that reading it again reports why.
pub(super) fn source_changed(
    media: Option<&dyn MediaStore>,
    kind: AssetKind,
    source: &str,
    seen: Option<&MediaIdentity>,
) -> bool {
    let Some(media) = media else {
        return seen.is_some();
    };
    match media.resolve(source, kind) {
        Ok(resolved) => Some(&resolved.identity) != seen,
        Err(error) => {
            let missing = error.failure().is_some_and(|failure| {
                matches!(failure.reason, crate::error::MediaReason::NotFound { .. })
            });
            !missing || seen.is_some()
        }
    }
}

impl FrameCx<'_> {
    /// Notes that the model, environment map or text mesh `id` was drawn, so that a kept output
    /// recording now keeps it.
    #[cfg(feature = "3d")]
    pub(crate) fn drew_model(&mut self, id: &str) {
        self.retention.drew_model(&mut self.st.retained, id);
    }

    /// Notes that the model being drawn holds one pose on `frames`, both ends included, which a
    /// kept output recording now can only be read on.
    #[cfg(feature = "3d")]
    pub(crate) fn held_on(&mut self, frames: (f64, f64)) {
        self.retention.held_on(frames);
    }

    /// What keeps `raster`, the output of `node`, the same from frame to frame, if anything.
    pub(crate) fn stable_raster(
        &self,
        node: NodeId,
        raster: Raster,
    ) -> Option<crate::gpu::mips::Stable> {
        use crate::gpu::mips::Stable;
        let graph = self.scope.id;
        if let Some(slot) = self.st.retained.slots.get(&(graph, node))
            && (slot.raster.tex == raster.tex || slot.full == Some(raster.tex))
        {
            return Some(Stable::Slot {
                graph,
                node,
                generation: slot.generation,
            });
        }
        self.st
            .media
            .textures
            .values()
            .find(|(tex, ..)| *tex == raster.tex)
            .and_then(|(.., key)| key.clone())
            .map(Stable::Image)
    }

    /// Records the required input `port` of `node_id` as is.
    pub(crate) fn required(
        &mut self,
        node_id: NodeId,
        port: &str,
        input: Option<NodeId>,
    ) -> crate::Result<Raster> {
        self.input(required_input(node_id, port, input)?)
    }

    /// Records the required input `port` of `node_id` at its full display size.
    pub(crate) fn required_full(
        &mut self,
        node_id: NodeId,
        port: &str,
        input: Option<NodeId>,
    ) -> crate::Result<Raster> {
        self.input_full(required_input(node_id, port, input)?)
    }

    /// Records `node_id` as is, for readers that read a raster through its domain.
    pub(crate) fn input(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        let node = self.scope.node(node_id).ok_or(RenderError::MissingNode {
            at: crate::error::At::node(node_id),
        })?;
        let graph = self.scope.id;
        let staticness = if self.settings.retain_static {
            self.staticness(node)
        } else {
            Staticness::Never
        };
        // A value that fails to evaluate fails recording, which reports it.
        let values = match staticness {
            Staticness::WhileEqual => self.frame_values(node_id, self.frame),
            _ => Some(Vec::new()),
        };
        let (staticness, values) = match values {
            Some(values) => (staticness, values),
            None => (Staticness::Never, Vec::new()),
        };
        let is_static = staticness == Staticness::Always;
        let tracked = matches!(staticness, Staticness::WhileHeld | Staticness::WhileEqual);
        let frame = self.frame.value();
        if staticness != Staticness::Never
            && let Some(slot) = self.st.retained.slots.get_mut(&(graph, node_id))
            && slot.frames.0 <= frame
            && frame <= slot.frames.1
            && same_values(&slot.values, &values)
        {
            slot.last_used = self.st.tick;
            self.st.retained.skipped_passes += slot.passes;
            let raster = slot.raster;
            self.retention
                .hold_slot(&self.st.retained, (graph, node_id));
            return Ok(raster);
        }
        let key = ((graph, self.path, node_id), self.sample);
        let raster = match self.st.memo.get(&key) {
            Some(raster) => {
                let raster = *raster;
                if let Some(read) = self.st.retained.held_memo.get(&key) {
                    self.retention.absorb(read);
                }
                raster
            }
            None => {
                if self.depth == MAX_GRAPH_DEPTH {
                    return Err(self.scope.too_deep(node_id));
                }
                let passes = self.rec.pass_count();
                self.retention.enter(is_static, tracked);
                self.depth += 1;
                let raster = super::record_node(node, self);
                self.depth -= 1;
                let track = self.retention.leave(is_static, tracked);
                let raster = raster?;
                self.st.memo.insert(key, raster);
                let passes = (self.rec.pass_count() - passes) as u64;
                if is_static && self.retention.at_root() {
                    let frames = (f64::NEG_INFINITY, f64::INFINITY);
                    let models = std::mem::take(&mut self.st.retained.static_models);
                    self.retain(graph, node_id, raster, passes, frames, models, Vec::new());
                }
                if let Some(track) = track {
                    self.finish_hold(key, raster, passes, track, values);
                }
                raster
            }
        };
        Ok(raster)
    }

    /// Keeps `raster`, the output of `node`, for the `frames` it stays correct on while the params
    /// reading the frame take `values`, along with the `models` it drew.
    #[expect(clippy::too_many_arguments, reason = "the parts of a slot")]
    fn retain(
        &mut self,
        graph: SceneId,
        node: NodeId,
        raster: Raster,
        passes: u64,
        frames: (f64, f64),
        models: Vec<String>,
        values: Vec<ExpressionValue>,
    ) {
        // A slot kept for other frames makes way.
        self.st.retained.release(self.rec, (graph, node));
        self.rec.retain(raster.tex);
        let bytes = self.rec.texture_bytes(raster.tex);
        self.st.retained.bytes += bytes;
        self.st.retained.generation += 1;
        self.st.retained.slots.insert(
            (graph, node),
            Slot {
                raster,
                passes,
                full: None,
                last_used: self.st.tick,
                bytes,
                generation: self.st.retained.generation,
                frames,
                models,
                values,
            },
        );
        self.st.retained.pending.push((graph, node));
    }

    /// Keeps `raster`, the output of `node` just recorded with the params reading the frame at
    /// `values`, if it stays correct on this frame and on one either side of it too, and tells the
    /// output being recorded around it what it found.
    fn finish_hold(
        &mut self,
        key: MemoKey,
        raster: Raster,
        passes: u64,
        track: HoldTrack,
        values: Vec<ExpressionValue>,
    ) {
        let ((graph, _, node), _) = key;
        self.retention.absorb(&track);
        self.st.retained.held_memo.insert(key, track.clone());
        let frame = self.frame.value();
        let at_root = self.retention.at_root();
        let kept = track.keeps(frame, at_root, |other| {
            self.frame_values(node, other.into())
                .is_some_and(|elsewhere| same_values(&elsewhere, &values))
        });
        if kept {
            self.retain(
                graph,
                node,
                raster,
                passes,
                track.frames,
                track.models,
                values,
            );
        }
    }

    /// The values that the params reading the frame take at `frame`: those of `node` and of each
    /// node it reads that is kept while they hold, in the same order on every frame. A node that
    /// blurs its own motion adds their values at either end of its shutter, where it evaluates
    /// them too. `None` when one fails to evaluate.
    fn frame_values(&mut self, node: NodeId, frame: FrameTime) -> Option<Vec<ExpressionValue>> {
        let graph = self.scope.id;
        let mut values = Vec::new();
        let mut pending = vec![node];
        let mut seen = HashSet::default();
        while let Some(id) = pending.pop() {
            if !seen.insert(id) {
                continue;
            }
            values.extend_from_slice(&self.own_values(id, frame)?);
            let held = |input: &NodeId| {
                self.st.statics.get(&(graph, *input)) == Some(&Staticness::WhileEqual)
            };
            let inputs = self.scope.node(id)?.input_ports();
            pending.extend(inputs.into_iter().flatten().filter(held));
        }
        Some(values)
    }

    /// The values the params of `id` itself that read the frame take at `frame`, evaluated once
    /// per frame for every node that reads it with the same props, which can differ between two
    /// reads of one frame.
    fn own_values(&mut self, id: NodeId, frame: FrameTime) -> Option<Arc<[ExpressionValue]>> {
        let props = self.scope.props.as_ref();
        let key = (
            (self.scope.id, self.path, id),
            frame.value().to_bits(),
            props.map_or(0, |props| Arc::as_ptr(props) as usize),
        );
        if let Some((_, values)) = self.st.frame_values.get(&key) {
            return values.clone();
        }
        let values = self.evaluate_own_values(id, frame);
        let entry = (self.scope.props.clone(), values.clone());
        self.st.frame_values.insert(key, entry);
        values
    }

    fn evaluate_own_values(&self, id: NodeId, frame: FrameTime) -> Option<Arc<[ExpressionValue]>> {
        let expr = self.scope.expr_context(frame, self.media);
        let node = self.scope.node(id)?;
        let mut contexts = vec![expr];
        if self.analytic_motion_blur()
            && let Some(shutter) = node.analytic_shutter()
            && let Some(offset) =
                crate::node::motion::shutter_offset(shutter.eval(id, "motion_blur", &expr).ok()?)
        {
            contexts.extend([expr.at_subframe(-offset), expr.at_subframe(offset)]);
        }
        let mut values = Vec::new();
        let mut failed = false;
        for expr in &contexts {
            node.for_each_expression(&mut |expression| {
                if !expression.is_frame_dependent() {
                    return;
                }
                match expression.evaluate(expr) {
                    Ok(value) => values.push(value),
                    Err(_) => failed = true,
                }
            });
        }
        (!failed).then(|| values.into())
    }

    /// Records `node_id` stored at its full display size, for readers that read texel for texel.
    pub(crate) fn input_full(&mut self, node_id: NodeId) -> crate::Result<Raster> {
        let raster = self.input(node_id)?;
        if raster.domain.is_full_frame() {
            return Ok(raster);
        }
        let slot_key = (self.scope.id, node_id);
        let kept = |slot: &Slot| slot.raster.tex == raster.tex;
        if let Some(slot) = self.st.retained.slots.get(&slot_key)
            && kept(slot)
            && let Some(full) = slot.full
        {
            return Ok(Raster {
                tex: full,
                domain: TextureDomain::full_frame(raster.domain.display_size()),
                bounds: None,
            });
        }
        let full = self.full_frame(raster);
        if self.st.retained.slots.get(&slot_key).is_some_and(kept) {
            self.rec.retain(full.tex);
            let bytes = self.rec.texture_bytes(full.tex);
            self.st.retained.bytes += bytes;
            let slot = self.st.retained.slots.get_mut(&slot_key).expect("slot");
            slot.full = Some(full.tex);
            slot.bytes += bytes;
            if !self.st.retained.pending.contains(&slot_key) {
                self.st.retained.pending.push(slot_key);
            }
        }
        Ok(full)
    }

    /// How far `node` and everything it reads are static candidates. Walks its inputs depth first
    /// with a stack of its own, so a long chain of nodes can't run out of call stack.
    fn staticness(&mut self, node: &NodeKind) -> Staticness {
        let graph = self.scope.id;
        let mut pending = vec![node.id()];
        // Nodes whose inputs have been put on `pending`, which are decided when next reached.
        let mut entered = HashSet::default();
        while let Some(&id) = pending.last() {
            if self.st.statics.contains_key(&(graph, id)) {
                pending.pop();
                continue;
            }
            let candidate = self
                .scope
                .node(id)
                .filter(|node| !matches!(node, NodeKind::Clip(_) | NodeKind::TimeRemap(_)))
                .map(|node| (node, node.staticness(node.is_frame_dependent())))
                .filter(|(_, own)| *own != Staticness::Never);
            let Some((node, own)) = candidate else {
                self.st.statics.insert((graph, id), Staticness::Never);
                pending.pop();
                continue;
            };
            let inputs = node.input_ports().into_iter().flatten();
            if entered.insert(id) {
                let undecided = inputs
                    .clone()
                    .filter(|input| !self.st.statics.contains_key(&(graph, *input)));
                let before = pending.len();
                pending.extend(undecided);
                if pending.len() > before {
                    continue;
                }
            }
            // An input still undecided here is part of a cycle, which validation rejects.
            let result = inputs.into_iter().fold(own, |all, input| {
                all.and(
                    self.st
                        .statics
                        .get(&(graph, input))
                        .copied()
                        .unwrap_or(Staticness::Never),
                )
            });
            // A `motion_blur` mixes its source at frames other than this one, where the values
            // its source is kept by aren't read.
            let result = match (node, result) {
                (NodeKind::MotionBlur(_), Staticness::WhileEqual) => Staticness::Never,
                _ => result,
            };
            self.st.statics.insert((graph, id), result);
            pending.pop();
        }
        self.st
            .statics
            .get(&(graph, node.id()))
            .copied()
            .unwrap_or(Staticness::Never)
    }

    /// Notes that `source` was read as `kind` and had `identity`, so that what is kept of this
    /// frame is dropped once it has another. It counts while a kept output records, including one
    /// kept while its models hold a pose or its params' values hold; font families count whenever
    /// text names them, as text instances are kept across frames whatever reads them.
    pub(crate) fn read_source(
        &mut self,
        kind: AssetKind,
        source: &str,
        identity: Option<&MediaIdentity>,
    ) {
        let kept = self.retention.keeping() || kind == AssetKind::Font;
        if kept
            && !self
                .st
                .retained
                .sources
                .contains_key(&(kind, source.to_string()))
        {
            self.st
                .retained
                .sources
                .insert((kind, source.to_string()), identity.cloned());
        }
    }

    /// Notes the font family text names, as [`Self::read_source`] does.
    pub(crate) fn note_font(&mut self, family: &str) {
        if !self
            .st
            .retained
            .sources
            .contains_key(&(AssetKind::Font, family.to_string()))
        {
            let identity = source_identity(self.media, AssetKind::Font, family);
            self.read_source(AssetKind::Font, family, identity.as_ref());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::MediaError;
    use crate::media::ResolvedMedia;

    /// Fails every source the way `failure` says.
    #[derive(Debug)]
    struct Failing(fn(&str) -> MediaError);

    impl MediaStore for Failing {
        fn resolve(&self, source: &str, _kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
            Err(self.0(source))
        }
    }

    #[test]
    fn a_missing_source_that_starts_failing_another_way_counts_as_changed() {
        let changed = |store: &Failing, seen: Option<&MediaIdentity>| {
            source_changed(Some(store), AssetKind::Font, "Brand", seen)
        };
        let missing = Failing(|source| MediaError::source_not_found(source, "not served"));
        let unreadable = Failing(|source| MediaError::decode(source, "permission denied"));
        assert!(!changed(&missing, None));
        assert!(changed(&unreadable, None));
        let seen = MediaIdentity::of_key("brand");
        assert!(changed(&missing, Some(&seen)) && changed(&unreadable, Some(&seen)));
        assert!(!source_changed(None, AssetKind::Font, "Brand", None));
    }

    #[test]
    fn only_the_outermost_static_node_keeps_its_own_output() {
        let mut retention = Retention::default();
        retention.enter(true, false);
        retention.enter(true, false);
        retention.leave(true, false);
        assert!(!retention.at_root() && retention.keeping());
        retention.leave(true, false);
        assert!(retention.at_root() && !retention.keeping());
    }

    #[test]
    fn a_hold_is_kept_only_when_a_neighbouring_frame_holds_too() {
        let mut retention = Retention::default();
        retention.enter(false, true);
        retention.absorb(&HoldTrack {
            frames: (10.0, 20.0),
            models: Vec::new(),
        });
        let track = retention.leave(false, true).expect("tracked");
        assert!(track.keeps(15.0, true, |_| true));
        assert!(track.keeps(20.0, true, |other| other == 19.0));
        assert!(!track.keeps(15.0, true, |_| false));
        assert!(!track.keeps(15.0, false, |_| true));
        assert!(!track.keeps(21.0, true, |_| true));
        let single = HoldTrack {
            frames: (5.0, 5.0),
            models: Vec::new(),
        };
        assert!(!single.keeps(5.0, true, |_| true));
    }

    #[test]
    fn values_differ_by_their_bits() {
        let number = |value: f64| [ExpressionValue::Number(value)];
        assert!(!same_values(&number(0.0), &number(-0.0)));
        assert!(same_values(&number(f64::NAN), &number(f64::NAN)));
    }
}
