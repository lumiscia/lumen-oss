//! Reuse of render output across frames.
//!
//! A node instance is one node reached through one path of time remaps and clips from the root,
//! so each clip of a template has its own instances of the template's nodes. The compiler
//! records which instance created each texture, buffer and pass, and which instances feed it.
//! From that, a plan knows two kinds of output it can keep instead of rendering again:
//!
//! - Static instances: a node whose params hold no expression and that reads no media, built-in
//!   time or other frame-dependent state, fed only by static instances, renders the same output
//!   on every frame bound to its plan. Once rendered it is not dispatched again until the plan's
//!   epoch changes. Text under kept output still places its glyphs in the shared atlas each
//!   frame, in the order a full bind would, so the atlas evicts and drops exactly the glyphs it
//!   would without kept output; kept text whose glyphs are dropped or placed again renders again,
//!   along with what reads it.
//! - Keyed memos: a `memo` whose `key` is an expression copies its source, with any bounds
//!   published for it, into its own texture, and only when the key's value changes. While it
//!   holds, instances that only it reaches are neither bound nor dispatched.
//!
//! A memo's snapshot belongs to the renderer rather than to a plan: it is identified by the memo
//! node and its path, and holds only across frames of the exact composition it was
//! taken from, as fingerprinted by `composition_fingerprint`, and while the key keeps its value.
//! A plan bound with a held key that does not have the snapshot yet copies it from a plan that
//! does. Snapshots live in their plans' textures, so they count against the plan cache budget;
//! once every plan holding one is evicted or recompiled, the next frame renders the source again
//! and takes a new snapshot.
//!
//! Compiled nodes bind the params they were compiled with, so a plan's static instances bind the
//! same values whatever edits the composition gets until it is compiled again. The epoch hashes
//! what else their output depends on: the loaded fonts and the textures static instances and memos
//! write or read. Any change renders the kept output again.
//!
//! A pass runs only when the instance that created it was bound on the frame, so kept output is
//! only ever overwritten by the instance that owns it, and the pass's uniforms are always the ones
//! its own bind wrote. When that cannot be relied on, as when several instances write one
//! texture, nothing is kept but memo snapshots, whose sources are then bound and dispatched every
//! frame.

use std::collections::HashMap;
use std::hash::Hasher;

use crate::{
    composition::Composition,
    gpu::TimeRemapPath,
    node::{NodeId, NodeKind},
};

/// A memo node reached through one path of time remaps and clips.
pub(crate) type MemoId = (NodeId, TimeRemapPath);

/// What the compiler records for the cache while it builds a plan.
#[derive(Debug, Default)]
pub(crate) struct CacheRecorder {
    /// Each instance's node and path.
    instances: Vec<MemoId>,
    index: HashMap<NodeId, HashMap<TimeRemapPath, usize>>,
    inputs: Vec<Vec<usize>>,
    /// Instances being compiled, innermost last; new resources belong to the last.
    stack: Vec<usize>,
    texture_owners: Vec<Option<usize>>,
    buffer_owners: Vec<Option<usize>>,
    pass_owners: Vec<Option<usize>>,
    memos: Vec<MemoRecord>,
}

#[derive(Debug, Clone)]
struct MemoRecord {
    instance: usize,
    memo: LayoutMemo,
}

impl CacheRecorder {
    /// Makes the instance of `node_id` on `path` current, recording that the previous current
    /// instance reads it.
    pub(crate) fn enter(
        &mut self,
        builder: &lumen_gpu::RenderPlanBuilder,
        node_id: NodeId,
        path: &TimeRemapPath,
    ) -> usize {
        let instance = match self
            .index
            .get(&node_id)
            .and_then(|paths| paths.get(path.as_slice()))
        {
            Some(&instance) => instance,
            None => {
                let instance = self.instances.len();
                self.instances.push((node_id, path.clone()));
                self.inputs.push(Vec::new());
                self.index
                    .entry(node_id)
                    .or_default()
                    .insert(path.clone(), instance);
                instance
            }
        };
        if let Some(&reader) = self.stack.last()
            && !self.inputs[reader].contains(&instance)
        {
            self.inputs[reader].push(instance);
        }
        self.push(builder, instance);
        instance
    }

    /// Attributes what `builder` gained since the last change of instance to the current one,
    /// then makes `instance` current.
    pub(crate) fn push(&mut self, builder: &lumen_gpu::RenderPlanBuilder, instance: usize) {
        self.attribute(builder);
        self.stack.push(instance);
    }

    /// Attributes what `builder` gained to the current instance, then returns to the previous.
    pub(crate) fn pop(&mut self, builder: &lumen_gpu::RenderPlanBuilder) {
        self.attribute(builder);
        self.stack.pop();
    }

    pub(crate) fn texture_owner(&self, texture: lumen_gpu::TextureId) -> Option<usize> {
        self.texture_owners
            .get(texture.0 as usize)
            .copied()
            .flatten()
    }

    /// Records a keyed memo's texture, the bounds buffer copied with it and the pass that copies
    /// its source into them, all created by the instance being compiled.
    pub(crate) fn record_memo(
        &mut self,
        texture: lumen_gpu::TextureId,
        bounds: Option<lumen_gpu::BufferId>,
        copy_pass: lumen_gpu::PassId,
    ) {
        if let Some(&instance) = self.stack.last() {
            self.memos.push(MemoRecord {
                instance,
                memo: LayoutMemo {
                    id: self.instances[instance].clone(),
                    texture,
                    bounds,
                    copy_pass: copy_pass.0 as usize,
                },
            });
        }
    }

    pub(crate) fn is_memo_texture(&self, texture: lumen_gpu::TextureId) -> bool {
        self.memos
            .iter()
            .any(|record| record.memo.texture == texture)
    }

    fn attribute(&mut self, builder: &lumen_gpu::RenderPlanBuilder) {
        let owner = self.stack.last().copied();
        self.texture_owners.resize(builder.texture_count(), owner);
        self.buffer_owners.resize(builder.buffer_count(), owner);
        self.pass_owners.resize(builder.pass_count(), owner);
    }

    /// The layout of the finished `plan`, whose output is `output` and whose shared text atlas,
    /// which text binds keep filled for the passes they run, is `text_atlas`.
    pub(crate) fn finish(
        self,
        composition: &Composition,
        plan: &lumen_gpu::RenderPlan,
        output: lumen_gpu::TextureId,
        text_atlas: Option<lumen_gpu::TextureId>,
    ) -> CacheLayout {
        let count = self.instances.len();
        let nodes = self
            .instances
            .iter()
            .map(|(id, path)| {
                crate::scope::Scope::graph_at(composition, path)
                    .and_then(|graph| graph.nodes.get(id))
            })
            .collect::<Vec<_>>();
        let mut layout = CacheLayout {
            index: self.index,
            pass_owners: self.pass_owners,
            static_instances: vec![false; count],
            reaches_text: nodes
                .iter()
                .map(|node| matches!(node, Some(NodeKind::Text(_))))
                .collect(),
            guarded_textures: self
                .memos
                .iter()
                .map(|record| record.memo.texture)
                .collect(),
            memos: self
                .memos
                .into_iter()
                .map(|record| (record.instance, record.memo))
                .collect(),
        };
        loop {
            let mut changed = false;
            for instance in 0..count {
                if !layout.reaches_text[instance]
                    && self.inputs[instance]
                        .iter()
                        .any(|input| layout.reaches_text[*input])
                {
                    layout.reaches_text[instance] = true;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        let accesses = plan.passes().iter().map(pass_accesses).collect::<Vec<_>>();
        if layout.pass_owners.len() != plan.passes().len()
            || layout.pass_owners.iter().any(Option::is_none)
            || !writers_share_owner(&accesses, &layout.pass_owners, plan.textures().len())
        {
            tracing::debug!(
                target: "lumen_render",
                "plan output cannot be reused across frames: a pass has no owner or textures are written by several nodes"
            );
            layout.pass_owners.clear();
            return layout;
        }

        let mut candidate = nodes
            .iter()
            .map(|node| node.is_some_and(is_static_candidate))
            .collect::<Vec<_>>();
        for (pass, access) in accesses.iter().enumerate() {
            if access.writes.contains(&Resource::Texture(output)) {
                candidate[layout.pass_owners[pass].expect("owned pass")] = false;
            }
        }
        let owner_of = |resource: &Resource| match *resource {
            Resource::Texture(id) if Some(id) == text_atlas => None,
            Resource::Texture(id) => {
                Some(self.texture_owners.get(id.0 as usize).copied().flatten())
            }
            Resource::Buffer(id) => Some(self.buffer_owners.get(id.0 as usize).copied().flatten()),
        };
        // Static instances stay static only while what they read and feed on is static too.
        let mut is_static = candidate;
        loop {
            let mut changed = false;
            for instance in 0..count {
                if is_static[instance]
                    && !self.inputs[instance].iter().all(|input| is_static[*input])
                {
                    is_static[instance] = false;
                    changed = true;
                }
            }
            for (pass, access) in accesses.iter().enumerate() {
                let owner = layout.pass_owners[pass].expect("owned pass");
                if is_static[owner]
                    && !access
                        .reads
                        .iter()
                        .chain(&access.writes)
                        .filter_map(owner_of)
                        .all(|resource_owner| resource_owner.is_some_and(|o| is_static[o]))
                {
                    is_static[owner] = false;
                    changed = true;
                }
            }
            if !changed {
                break;
            }
        }
        layout.static_instances = is_static;
        tracing::debug!(
            target: "lumen_render",
            instances = count,
            static_instances = layout.static_instances.iter().filter(|s| **s).count(),
            keyed_memos = layout.memos.len(),
            "plan output kept across frames"
        );

        layout.guarded_textures.extend(
            self.texture_owners
                .iter()
                .enumerate()
                .filter(|(id, owner)| {
                    owner.is_some_and(|owner| layout.static_instances[owner])
                        && Some(lumen_gpu::TextureId(*id as u32)) != text_atlas
                })
                .map(|(id, _)| lumen_gpu::TextureId(id as u32)),
        );
        layout
    }
}

/// Whether the node renders the same output on every frame as long as its inputs do.
fn is_static_candidate(node: &NodeKind) -> bool {
    match node {
        // Media changes with the frame and the media store, shaders see the built-in time, and
        // the output is written into external textures that replace its own.
        NodeKind::MediaIn(_) | NodeKind::WgslShader(_) | NodeKind::MediaOutput(_) => false,
        _ => !node.has_expressions(),
    }
}

/// Identifies the exact content of a composition and its templates: their canvases, timelines,
/// props, connections and every node's params, literal values and expression sources.
pub(crate) fn composition_fingerprint(composition: &Composition) -> u64 {
    use std::fmt::Write;

    // Debug output spells out every param, expression and input.
    struct HashWriter(std::hash::DefaultHasher);
    impl Write for HashWriter {
        fn write_str(&mut self, text: &str) -> std::fmt::Result {
            self.0.write(text.as_bytes());
            Ok(())
        }
    }

    fn write_graph(out: &mut HashWriter, graph: &crate::graph::Graph) {
        let mut nodes = graph.nodes.iter().collect::<Vec<_>>();
        nodes.sort_unstable_by_key(|(id, _)| id.0);
        let _ = write!(out, "{:?}", graph.connections);
        for (id, node) in nodes {
            let _ = write!(out, "{}{:?}", id.0, node);
        }
    }

    let mut out = HashWriter(std::hash::DefaultHasher::new());
    let _ = write!(
        out,
        "{:?}{:?}",
        composition.render_settings, composition.timeline
    );
    write_graph(&mut out, &composition.graph);
    for (name, template) in &composition.compositions {
        let _ = write!(
            out,
            "{name:?}{:?}{:?}{:?}",
            template.render_settings, template.timeline, template.props
        );
        write_graph(&mut out, &template.graph);
    }
    out.0.finish()
}

/// A plan's instances and which of their output can be kept across frames.
#[derive(Debug, Default)]
pub(crate) struct CacheLayout {
    index: HashMap<NodeId, HashMap<TimeRemapPath, usize>>,
    /// The instance that created each pass; empty when no output but memo snapshots may be kept.
    pass_owners: Vec<Option<usize>>,
    static_instances: Vec<bool>,
    /// Whether each instance is text or reads text.
    reaches_text: Vec<bool>,
    /// Keyed memos, by memo instance.
    memos: HashMap<usize, LayoutMemo>,
    /// Textures whose replacement loses kept output.
    guarded_textures: Vec<lumen_gpu::TextureId>,
}

/// Where a plan keeps a keyed memo's snapshot.
#[derive(Debug, Clone)]
pub(crate) struct LayoutMemo {
    pub(crate) id: MemoId,
    pub(crate) texture: lumen_gpu::TextureId,
    pub(crate) bounds: Option<lumen_gpu::BufferId>,
    copy_pass: usize,
}

impl CacheLayout {
    pub(crate) fn instance(&self, node_id: NodeId, path: &[NodeId]) -> Option<usize> {
        self.index.get(&node_id)?.get(path).copied()
    }

    fn instance_count(&self) -> usize {
        self.static_instances.len()
    }

    /// Whether passes may be skipped for instances that were not bound.
    pub(crate) fn enabled(&self) -> bool {
        !self.pass_owners.is_empty()
    }

    pub(crate) fn has_memos(&self) -> bool {
        !self.memos.is_empty()
    }

    pub(crate) fn is_keyed_memo(&self, instance: usize) -> bool {
        self.memos.contains_key(&instance)
    }

    pub(crate) fn reaches_text(&self, instance: usize) -> bool {
        self.reaches_text[instance]
    }

    pub(crate) fn memo(&self, instance: usize) -> Option<&LayoutMemo> {
        self.memos.get(&instance)
    }

    pub(crate) fn memo_by_id(&self, id: &MemoId) -> Option<&LayoutMemo> {
        self.memos.values().find(|memo| memo.id == *id)
    }

    /// Hashes what kept output depends on beyond the plan key and compiled params, as
    /// `renderer` and the shared text system hold it now.
    pub(crate) fn epoch(&self, renderer: &lumen_gpu::Renderer) -> u64 {
        let mut hasher = std::hash::DefaultHasher::new();
        let generation = crate::node::source::text::text_system()
            .ok()
            .map(|text_system| text_system.font_generation());
        std::hash::Hash::hash(&generation, &mut hasher);
        for texture in &self.guarded_textures {
            hasher.write_u64(renderer.texture_generation(*texture).unwrap_or(u64::MAX));
        }
        hasher.finish()
    }
}

/// Memo snapshots kept across the plans of one renderer.
#[derive(Debug, Default)]
pub(crate) struct MemoSnapshots(HashMap<MemoId, MemoSnapshot>);

#[derive(Debug)]
struct MemoSnapshot {
    key: u64,
    /// `composition_fingerprint` of the composition the snapshot was taken from.
    fingerprint: u64,
    /// The plans whose memo texture holds the snapshot, with the epoch they copied it at.
    holders: HashMap<u64, u64>,
}

impl MemoSnapshots {
    /// The snapshots plan `plan`, bound at `epoch`, can show, by memo instance. `copy_from`
    /// tells whether another plan, given its id and the epoch it took the snapshot at, still
    /// holds it in a texture the plan can copy.
    pub(crate) fn held(
        &self,
        layout: &CacheLayout,
        plan: u64,
        epoch: u64,
        copy_from: impl Fn(u64, u64, &LayoutMemo) -> bool,
    ) -> HashMap<usize, HeldMemo> {
        layout
            .memos
            .iter()
            .filter_map(|(instance, memo)| {
                let snapshot = self.0.get(&memo.id)?;
                let from = if snapshot.holders.get(&plan) == Some(&epoch) {
                    None
                } else {
                    let (from, _) = snapshot
                        .holders
                        .iter()
                        .find(|(holder, at)| **holder != plan && copy_from(**holder, **at, memo))?;
                    Some(*from)
                };
                Some((
                    *instance,
                    HeldMemo {
                        key: snapshot.key,
                        from,
                    },
                ))
            })
            .collect()
    }

    pub(crate) fn clear(&mut self) {
        self.0.clear();
    }

    pub(crate) fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Drops the snapshots taken from any composition but the one with `fingerprint`.
    pub(crate) fn retain_composition(&mut self, fingerprint: u64) {
        self.0
            .retain(|_, snapshot| snapshot.fingerprint == fingerprint);
    }
}

/// A memo snapshot a plan can show without rendering the memo's source.
#[derive(Debug, Clone, Copy)]
pub(crate) struct HeldMemo {
    key: u64,
    /// The plan to copy the snapshot from, unless the plan's own texture holds it.
    from: Option<u64>,
}

/// What a plan has kept from the frames submitted to it.
#[derive(Debug, Default)]
pub(crate) struct CacheState {
    /// Identifies the plan among the renderer's plans, including ones compiled again.
    plan: u64,
    /// The epoch each static instance's output was last rendered at.
    rendered: Vec<Option<u64>>,
    /// What the uploaded frame skips and keeps once submitted.
    pending: Option<PendingFrame>,
}

#[derive(Debug)]
struct PendingFrame {
    skip: Vec<bool>,
    commit: Option<BoundCache>,
}

impl CacheState {
    pub(crate) fn new(plan: u64) -> Self {
        Self {
            plan,
            ..Self::default()
        }
    }

    pub(crate) fn plan(&self) -> u64 {
        self.plan
    }

    /// Whether a static instance's output rendered at `epoch` is kept.
    pub(crate) fn keeps_static(&self, layout: &CacheLayout, instance: usize, epoch: u64) -> bool {
        layout.static_instances[instance] && self.rendered.get(instance) == Some(&Some(epoch))
    }

    /// Readies the passes `bound` skips for the next submit.
    pub(crate) fn upload(&mut self, layout: &CacheLayout, bound: Option<&BoundCache>) {
        self.pending = bound.map(|bound| {
            let copies =
                |instance: usize| !bound.refreshed.iter().any(|(memo, _)| *memo == instance);
            let skip = if layout.enabled() {
                layout
                    .pass_owners
                    .iter()
                    .enumerate()
                    .map(|(pass, owner)| {
                        let owner = owner.expect("owned pass");
                        !bound.bound[owner]
                            || (layout.memos.get(&owner).map(|memo| memo.copy_pass) == Some(pass)
                                && copies(owner))
                    })
                    .collect()
            } else {
                let mut skip = Vec::new();
                for (instance, memo) in &layout.memos {
                    if copies(*instance) {
                        skip.resize(skip.len().max(memo.copy_pass + 1), false);
                        skip[memo.copy_pass] = true;
                    }
                }
                skip
            };
            PendingFrame {
                skip,
                commit: Some(bound.clone()),
            }
        });
    }

    /// The passes the next submit skips.
    pub(crate) fn skip(&self) -> &[bool] {
        self.pending.as_ref().map_or(&[], |pending| &pending.skip)
    }

    /// Keeps what the submitted frame rendered.
    pub(crate) fn submitted(&mut self, layout: &CacheLayout, snapshots: &mut MemoSnapshots) {
        let Some(bound) = self
            .pending
            .as_mut()
            .and_then(|pending| pending.commit.take())
        else {
            return;
        };
        self.rendered.resize(layout.instance_count(), None);
        for (instance, bound_now) in bound.bound.iter().enumerate() {
            if *bound_now && layout.static_instances[instance] {
                self.rendered[instance] = bound.keep.then_some(bound.epoch);
            }
        }
        for (instance, key) in &bound.refreshed {
            let memo = &layout.memos[instance];
            if bound.keep {
                snapshots.0.insert(
                    memo.id.clone(),
                    MemoSnapshot {
                        key: *key,
                        fingerprint: bound.fingerprint,
                        holders: HashMap::from([(self.plan, bound.epoch)]),
                    },
                );
            } else {
                // The source was copied here under a new key and nothing retains it, so no
                // plan's copy of the previous snapshot may be shown again.
                snapshots.0.remove(&memo.id);
            }
        }
        for (instance, _) in &bound.adopted {
            if let Some(snapshot) = snapshots.0.get_mut(&layout.memos[instance].id)
                && snapshot.fingerprint == bound.fingerprint
            {
                snapshot.holders.insert(self.plan, bound.epoch);
            }
        }
    }
}

/// Which instances a frame bound, against the state it was bound with.
#[derive(Debug, Clone)]
pub(crate) struct BoundCache {
    epoch: u64,
    /// `composition_fingerprint` of the composition the frame was bound from.
    fingerprint: u64,
    bound: Vec<bool>,
    /// Keyed memos that copy their source this frame, with their key.
    refreshed: Vec<(usize, u64)>,
    /// Keyed memos that copy their snapshot from another plan, with that plan.
    adopted: Vec<(usize, u64)>,
    /// Whether what the frame renders may be kept once submitted.
    keep: bool,
}

impl BoundCache {
    /// Renders the frame without keeping any of it for later frames.
    pub(crate) fn keep_nothing(&mut self) {
        self.keep = false;
    }

    /// Memo instances whose snapshot is copied from another plan, with that plan.
    pub(crate) fn adopted(&self) -> &[(usize, u64)] {
        &self.adopted
    }
}

/// The cache a bind reads and what it records.
pub(crate) struct BindCache<'a> {
    pub(crate) layout: &'a CacheLayout,
    pub(crate) state: &'a CacheState,
    pub(crate) epoch: u64,
    held: HashMap<usize, HeldMemo>,
    pub(crate) bound: BoundCache,
}

impl<'a> BindCache<'a> {
    pub(crate) fn new(
        layout: &'a CacheLayout,
        state: &'a CacheState,
        epoch: u64,
        fingerprint: u64,
        held: HashMap<usize, HeldMemo>,
    ) -> Self {
        Self {
            layout,
            state,
            epoch,
            held,
            bound: BoundCache {
                epoch,
                fingerprint,
                bound: vec![false; layout.instance_count()],
                refreshed: Vec::new(),
                adopted: Vec::new(),
                keep: true,
            },
        }
    }

    pub(crate) fn mark_bound(&mut self, instance: usize) {
        self.bound.bound[instance] = true;
    }

    /// Whether the keyed memo `instance` shows a snapshot taken for `key` instead of copying
    /// its source this frame.
    pub(crate) fn hold_memo(&mut self, instance: usize, key: u64) -> bool {
        match self.held.get(&instance) {
            Some(held) if held.key == key => {
                if let Some(from) = held.from {
                    self.bound.adopted.push((instance, from));
                }
                true
            }
            _ => {
                self.bound.refreshed.push((instance, key));
                false
            }
        }
    }
}

/// Number of passes `skip` marks.
pub(crate) fn skipped_count(skip: &[bool]) -> usize {
    skip.iter().filter(|skipped| **skipped).count()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Resource {
    Texture(lumen_gpu::TextureId),
    Buffer(lumen_gpu::BufferId),
}

#[derive(Debug, Default)]
struct PassAccess {
    reads: Vec<Resource>,
    writes: Vec<Resource>,
}

/// The resources a pass reads and writes. Storage bindings count as both.
fn pass_accesses(pass: &lumen_gpu::Pass) -> PassAccess {
    let mut access = PassAccess::default();
    let mut bindings = |bindings: &[lumen_gpu::Binding]| {
        for binding in bindings {
            match binding.resource {
                lumen_gpu::BindingResource::Texture { id, access: kind } => {
                    access.reads.push(Resource::Texture(id));
                    if kind != lumen_gpu::TextureAccess::Sampled {
                        access.writes.push(Resource::Texture(id));
                    }
                }
                lumen_gpu::BindingResource::Buffer { id, access: kind } => {
                    access.reads.push(Resource::Buffer(id));
                    if kind == lumen_gpu::BufferAccess::Storage {
                        access.writes.push(Resource::Buffer(id));
                    }
                }
                lumen_gpu::BindingResource::Sampler(_) => {}
            }
        }
    };
    match &pass.desc {
        lumen_gpu::PassDesc::Render(desc) => {
            bindings(&desc.bindings);
            access
                .reads
                .extend(desc.vertex_buffers.iter().map(|id| Resource::Buffer(*id)));
            access.reads.extend(
                desc.index_buffer
                    .iter()
                    .map(|(id, _)| Resource::Buffer(*id)),
            );
            for target in &desc.targets {
                access.writes.push(Resource::Texture(target.texture));
                if target.load == lumen_gpu::LoadOp::Load {
                    access.reads.push(Resource::Texture(target.texture));
                }
            }
        }
        lumen_gpu::PassDesc::Compute(desc) => {
            bindings(&desc.bindings);
            if let lumen_gpu::ComputeDispatch::Indirect { buffer, .. } = desc.dispatch {
                access.reads.push(Resource::Buffer(buffer));
            }
        }
        lumen_gpu::PassDesc::CopyTexture(desc) => {
            access.reads.push(Resource::Texture(desc.source));
            access.writes.push(Resource::Texture(desc.destination));
        }
    }
    access
}

/// Whether every texture and buffer is written by the passes of at most one instance.
fn writers_share_owner(
    accesses: &[PassAccess],
    owners: &[Option<usize>],
    texture_count: usize,
) -> bool {
    let mut texture_writers = vec![None; texture_count];
    let mut buffer_writers = HashMap::new();
    for (access, owner) in accesses.iter().zip(owners) {
        for resource in &access.writes {
            let writer = match *resource {
                Resource::Texture(id) => match texture_writers.get_mut(id.0 as usize) {
                    Some(writer) => writer,
                    None => return false,
                },
                Resource::Buffer(id) => buffer_writers.entry(id).or_insert(None),
            };
            if writer.is_some_and(|writer| Some(writer) != *owner) {
                return false;
            }
            *writer = *owner;
        }
    }
    true
}
