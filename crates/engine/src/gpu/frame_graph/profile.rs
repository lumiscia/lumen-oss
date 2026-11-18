use std::cell::RefCell;
use std::time::{Duration, Instant};

use foldhash::HashMap;

use crate::{
    composition::{Composition, SceneId},
    gpu::types::NodeTiming,
    node::{Node, NodeId, NodeKind},
};

/// Where a node is timed running: the scene it is in and when it started.
pub(super) struct NodeTimer {
    scene: SceneId,
    started: Instant,
}

struct Entry {
    kind: &'static str,
    template: Option<String>,
    bind: Duration,
    frames: u32,
    /// The last [`State::frame`] that counted in `frames`.
    last_frame: u32,
}

#[derive(Default)]
struct State {
    entries: HashMap<(SceneId, NodeId), Entry>,
    /// Time the nodes running now have spent in nodes inside them, innermost last.
    inner: Vec<Duration>,
    /// The frames recorded, counting one however many times it is recorded again.
    frame: u32,
}

/// Each node's own bind time, taken when [`super::RecordSettings::profile_nodes`] is set. Timers
/// start and finish through a shared reference, and no borrow of the state outlasts a call.
#[derive(Default)]
pub(super) struct NodeProfile {
    state: RefCell<State>,
}

impl NodeProfile {
    pub(super) fn begin_frame(&mut self) {
        self.state.get_mut().frame += 1;
    }

    pub(super) fn start(&self, scene: SceneId) -> NodeTimer {
        self.state.borrow_mut().inner.push(Duration::ZERO);
        NodeTimer {
            scene,
            started: Instant::now(),
        }
    }

    pub(super) fn finish(&self, timer: NodeTimer, node: &NodeKind, composition: &Composition) {
        let total = timer.started.elapsed();
        let mut state = self.state.borrow_mut();
        let state = &mut *state;
        let inner = state.inner.pop().unwrap_or_default();
        if let Some(outer) = state.inner.last_mut() {
            *outer += total;
        }
        let entry = state
            .entries
            .entry((timer.scene, node.id()))
            .or_insert_with(|| Entry {
                kind: node.kind_name(),
                template: composition.template_name(timer.scene).map(str::to_string),
                bind: Duration::ZERO,
                frames: 0,
                last_frame: 0,
            });
        entry.bind += total.saturating_sub(inner);
        if entry.last_frame != state.frame {
            entry.last_frame = state.frame;
            entry.frames += 1;
        }
    }

    /// What the nodes took since the last call, slowest first.
    pub(super) fn take(&mut self) -> Vec<NodeTiming> {
        let mut timings: Vec<_> = self
            .state
            .get_mut()
            .entries
            .drain()
            .map(|((_, node), entry)| NodeTiming {
                node,
                template: entry.template,
                kind: entry.kind,
                bind: entry.bind,
                frames: entry.frames,
            })
            .collect();
        timings.sort_by(|a, b| {
            b.bind
                .cmp(&a.bind)
                .then_with(|| a.template.cmp(&b.template))
                .then_with(|| a.node.cmp(&b.node))
        });
        timings
    }
}

/// Starts timers for the nodes of one scene, from code that holds the frame context by shared
/// reference. Times nothing when node profiling is off.
#[derive(Clone, Copy)]
pub(crate) struct NodeTimes<'a> {
    on: Option<(&'a NodeProfile, SceneId, &'a Composition)>,
}

impl<'a> NodeTimes<'a> {
    pub(super) fn new(
        profiling: bool,
        profile: &'a NodeProfile,
        scene: SceneId,
        composition: &'a Composition,
    ) -> Self {
        Self {
            on: profiling.then_some((profile, scene, composition)),
        }
    }

    /// Times `node` until the result is dropped, less the nodes timed inside it.
    pub(crate) fn time(self, node: &'a NodeKind) -> Timed<'a> {
        Timed(self.on.map(|(profile, scene, composition)| Running {
            profile,
            timer: profile.start(scene),
            node,
            composition,
        }))
    }
}

struct Running<'a> {
    profile: &'a NodeProfile,
    timer: NodeTimer,
    node: &'a NodeKind,
    composition: &'a Composition,
}

/// A node being timed.
pub(crate) struct Timed<'a>(Option<Running<'a>>);

impl Drop for Timed<'_> {
    fn drop(&mut self) {
        if let Some(running) = self.0.take() {
            running
                .profile
                .finish(running.timer, running.node, running.composition);
        }
    }
}
