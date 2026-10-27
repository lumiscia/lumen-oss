//! Reuse of render output across frames.
//!
//! A node instance is one node reached through one time-remap path. The compiler records which
//! instance created each texture, buffer and pass, and which instances feed it. From that, a plan
//! knows two kinds of output it can keep instead of rendering again:
//!
//! - Static instances: a node whose params hold no expression and that reads no media, built-in
//!   time or other frame-dependent state, fed only by static instances, renders the same output
//!   on every frame bound to its plan. Once rendered it is neither bound nor dispatched again
//!   until the plan's epoch changes.
//! - Keyed memos: a `memo` whose `key` is an expression copies its source into its own texture,
//!   and only when the key's value changes. While it holds, instances that only it reaches are
//!   neither bound nor dispatched.
//!
//! Compiled nodes bind the params they were compiled with, so a plan's static instances bind the
//! same values whatever edits the composition gets until it is compiled again. The epoch hashes
//! what else their output depends on: the loaded fonts and the textures static instances and memos
//! write or read. Any change renders the kept output again.
//!
//! A pass runs only when the instance that created it was bound on the frame, so kept output is
//! only ever overwritten by the instance that owns it, and the pass's uniforms are always the ones
//! its own bind wrote.

use std::collections::HashMap;
use std::hash::Hasher;

use crate::{
    composition::Composition,
    gpu::TimeRemapPath,
    node::{NodeId, NodeKind},
};

/// What the compiler records for the cache while it builds a plan.
#[derive(Debug, Default)]
pub(crate) struct CacheRecorder {
    node_ids: Vec<NodeId>,
    index: HashMap<NodeId, HashMap<TimeRemapPath, usize>>,
    inputs: Vec<Vec<usize>>,
    /// Instances being compiled, innermost last; new resources belong to the last.
    stack: Vec<usize>,
    texture_owners: Vec<Option<usize>>,
    buffer_owners: Vec<Option<usize>>,
    pass_owners: Vec<Option<usize>>,
    memos: Vec<MemoRecord>,
}

#[derive(Debug, Clone, Copy)]
struct MemoRecord {
    instance: usize,
    texture: lumen_gpu::TextureId,
    copy_pass: lumen_gpu::PassId,
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
                let instance = self.node_ids.len();
                self.node_ids.push(node_id);
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

    /// Records a keyed memo's texture and the pass that copies its source into it, both created
    /// by the instance being compiled.
    pub(crate) fn record_memo(
        &mut self,
        texture: lumen_gpu::TextureId,
        copy_pass: lumen_gpu::PassId,
    ) {
        if let Some(&instance) = self.stack.last() {
            self.memos.push(MemoRecord {
                instance,
                texture,
                copy_pass,
            });
        }
    }

    pub(crate) fn is_memo_texture(&self, texture: lumen_gpu::TextureId) -> bool {
        self.memos.iter().any(|memo| memo.texture == texture)
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
        let count = self.node_ids.len();
        let mut layout = CacheLayout {
            index: self.index,
            pass_owners: self.pass_owners,
            static_instances: vec![false; count],
            memos: HashMap::new(),
            guarded_textures: Vec::new(),
        };
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

        let nodes = &composition.graph.nodes;
        let mut candidate = self
            .node_ids
            .iter()
            .map(|id| nodes.get(id).is_some_and(is_static_candidate))
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
            keyed_memos = self.memos.len(),
            "plan output kept across frames"
        );

        for memo in &self.memos {
            layout
                .memos
                .insert(memo.instance, memo.copy_pass.0 as usize);
            layout.guarded_textures.push(memo.texture);
        }
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

/// A plan's instances and which of their output can be kept across frames.
#[derive(Debug, Default)]
pub(crate) struct CacheLayout {
    index: HashMap<NodeId, HashMap<TimeRemapPath, usize>>,
    /// The instance that created each pass; empty when no output may be kept.
    pass_owners: Vec<Option<usize>>,
    static_instances: Vec<bool>,
    /// The pass copying each keyed memo's source, by memo instance.
    memos: HashMap<usize, usize>,
    /// Textures whose replacement loses kept output.
    guarded_textures: Vec<lumen_gpu::TextureId>,
}

impl CacheLayout {
    pub(crate) fn instance(&self, node_id: NodeId, path: &[NodeId]) -> Option<usize> {
        self.index.get(&node_id)?.get(path).copied()
    }

    fn instance_count(&self) -> usize {
        self.static_instances.len()
    }

    fn enabled(&self) -> bool {
        !self.pass_owners.is_empty()
    }

    pub(crate) fn is_keyed_memo(&self, instance: usize) -> bool {
        self.memos.contains_key(&instance)
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

/// What a plan has kept from the frames submitted to it.
#[derive(Debug, Default)]
pub(crate) struct CacheState {
    /// The epoch each static instance's output was last rendered at.
    rendered: Vec<Option<u64>>,
    /// The key and epoch each keyed memo last copied its source at.
    memo_keys: HashMap<usize, (u64, u64)>,
    /// What the uploaded frame skips and keeps once submitted.
    pending: Option<PendingFrame>,
}

#[derive(Debug)]
struct PendingFrame {
    skip: Vec<bool>,
    commit: Option<BoundCache>,
}

impl CacheState {
    /// Whether a static instance's output rendered at `epoch` is kept.
    pub(crate) fn keeps_static(&self, layout: &CacheLayout, instance: usize, epoch: u64) -> bool {
        layout.static_instances[instance] && self.rendered.get(instance) == Some(&Some(epoch))
    }

    /// Whether a keyed memo holds the copy it made for `key` at `epoch`.
    pub(crate) fn holds_memo(&self, instance: usize, key: u64, epoch: u64) -> bool {
        self.memo_keys.get(&instance) == Some(&(key, epoch))
    }

    /// Readies the passes `bound` skips for the next submit.
    pub(crate) fn upload(&mut self, layout: &CacheLayout, bound: Option<&BoundCache>) {
        self.pending = bound
            .filter(|_| layout.enabled())
            .map(|bound| PendingFrame {
                skip: layout
                    .pass_owners
                    .iter()
                    .enumerate()
                    .map(|(pass, owner)| {
                        let owner = owner.expect("owned pass");
                        !bound.bound[owner]
                            || (layout.memos.get(&owner) == Some(&pass)
                                && !bound.refreshed.iter().any(|(memo, _)| *memo == owner))
                    })
                    .collect(),
                commit: Some(bound.clone()),
            });
    }

    /// The passes the next submit skips.
    pub(crate) fn skip(&self) -> &[bool] {
        self.pending.as_ref().map_or(&[], |pending| &pending.skip)
    }

    /// Keeps what the submitted frame rendered.
    pub(crate) fn submitted(&mut self, layout: &CacheLayout) {
        let Some(bound) = self
            .pending
            .as_mut()
            .and_then(|pending| pending.commit.take())
            .filter(|bound| bound.keep)
        else {
            return;
        };
        self.rendered.resize(layout.instance_count(), None);
        for (instance, bound_now) in bound.bound.iter().enumerate() {
            if *bound_now && layout.static_instances[instance] {
                self.rendered[instance] = Some(bound.epoch);
            }
        }
        for (memo, key) in bound.refreshed {
            self.memo_keys.insert(memo, (key, bound.epoch));
        }
    }
}

/// Which instances a frame bound, against the state it was bound with.
#[derive(Debug, Clone)]
pub(crate) struct BoundCache {
    epoch: u64,
    bound: Vec<bool>,
    /// Keyed memos that copy their source this frame, with their key.
    refreshed: Vec<(usize, u64)>,
    /// Whether what the frame renders may be kept once submitted.
    keep: bool,
}

impl BoundCache {
    /// Renders the frame without keeping any of it for later frames.
    pub(crate) fn keep_nothing(&mut self) {
        self.keep = false;
    }
}

/// The cache a bind reads and what it records.
pub(crate) struct BindCache<'a> {
    pub(crate) layout: &'a CacheLayout,
    pub(crate) state: &'a CacheState,
    pub(crate) epoch: u64,
    pub(crate) bound: BoundCache,
}

impl<'a> BindCache<'a> {
    pub(crate) fn new(layout: &'a CacheLayout, state: &'a CacheState, epoch: u64) -> Self {
        Self {
            layout,
            state,
            epoch,
            bound: BoundCache {
                epoch,
                bound: vec![false; layout.instance_count()],
                refreshed: Vec::new(),
                keep: true,
            },
        }
    }

    pub(crate) fn mark_bound(&mut self, instance: usize) {
        self.bound.bound[instance] = true;
    }

    pub(crate) fn mark_refreshed(&mut self, instance: usize, key: u64) {
        self.bound.refreshed.push((instance, key));
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
