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
    /// The last [`NodeProfile::frame`] that counted in `frames`.
    last_frame: u32,
}

/// Each node's own bind time, taken when [`super::RecordSettings::profile_nodes`] is set.
#[derive(Default)]
pub(super) struct NodeProfile {
    entries: HashMap<(SceneId, NodeId), Entry>,
    /// Time the nodes running now have spent in nodes inside them, innermost last.
    inner: Vec<Duration>,
    /// The frames recorded, counting one however many times it is recorded again.
    frame: u32,
}

impl NodeProfile {
    pub(super) fn begin_frame(&mut self) {
        self.frame += 1;
    }

    pub(super) fn start(&mut self, scene: SceneId) -> NodeTimer {
        self.inner.push(Duration::ZERO);
        NodeTimer {
            scene,
            started: Instant::now(),
        }
    }

    pub(super) fn finish(&mut self, timer: NodeTimer, node: &NodeKind, composition: &Composition) {
        let total = timer.started.elapsed();
        let inner = self.inner.pop().unwrap_or_default();
        if let Some(outer) = self.inner.last_mut() {
            *outer += total;
        }
        let entry = self
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
        if entry.last_frame != self.frame {
            entry.last_frame = self.frame;
            entry.frames += 1;
        }
    }

    /// What the nodes took since the last call, slowest first.
    pub(super) fn take(&mut self) -> Vec<NodeTiming> {
        let mut timings: Vec<_> = self
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
