//! Frame-graph recording: passes are recorded from what a frame shows, against long-lived pooled
//! textures, per-frame bump buffers and cached programs and bind groups.
//!
//! `acquire` hands out a texture that exists only for this frame's recording. `resolve` then walks
//! the recorded passes and gives each one a pooled texture for the passes from its first use to its
//! last, so textures whose uses don't overlap share one.
//!
//! Resources stay valid across frames in queue order: a pooled texture or bump range rewritten for
//! a later frame is only written by later submissions, so reuse needs no fence. Evicted textures
//! are destroyed, which wgpu defers until the submissions using them complete. Adopted textures
//! never join the pool and are never destroyed here, so whoever else holds one, such as an encoder,
//! decides when it is written next.

mod arena;
mod pool;
mod program;
mod submit;

use std::{hash::Hash, sync::Arc};

use crate::Size;

use arena::Arena;
use pool::{FrameTexture, PoolBucket, PoolEntry, PoolKey};
pub use program::{DEPTH_FORMAT, DepthTest, LayoutEntry, LayoutKind, RenderTargetSpec};
use program::{ProgramEntry, ProgramKey};

type Result<T, E = FrameError> = std::result::Result<T, E>;

/// The workgroup counts of a compute pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dispatch {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

/// Why the recorder failed a frame.
#[derive(Debug)]
pub enum FrameError {
    /// The frame recorded more data than the device's buffers hold.
    TooLarge(String),
    /// The recorder was used against its contract, or has broken one of its own.
    Misuse(String),
    /// The device refused what the recorder built or submitted.
    Device(String),
}

impl std::fmt::Display for FrameError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let (Self::TooLarge(details) | Self::Misuse(details) | Self::Device(details)) = self;
        f.write_str(details)
    }
}

impl std::error::Error for FrameError {}

/// Bytes of pooled textures kept before unused ones are dropped regardless of age.
pub const DEFAULT_POOL_BUDGET_BYTES: u64 = 384 << 20;
/// Set on a `TexRef` that names this frame's recording rather than a pooled or adopted texture.
const FRAME_TEXTURE: u32 = 1 << 31;
/// Frames a pooled texture or bind group stays cached without being used.
const IDLE_FRAMES: u64 = 600;
/// Frames between sweeps for idle resources.
const SWEEP_FRAMES: u64 = 60;
/// Programs built from runtime shader source that stay cached.
const MAX_DYNAMIC_PROGRAMS: usize = 64;
/// Most bindings a pass can have.
pub const MAX_PASS_BINDINGS: usize = 30;
const BUMP_ALIGNMENT: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TexRef(u32);

impl TexRef {
    fn frame_index(self) -> Option<usize> {
        (self.0 & FRAME_TEXTURE != 0).then_some((self.0 & !FRAME_TEXTURE) as usize)
    }
}

/// A texture with a mip chain, adopted whole and level by level.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MipChain {
    /// Every level, to sample with filtering between them.
    pub sampled: TexRef,
    /// Each level alone, largest first.
    pub levels: Vec<TexRef>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramRef(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SamplerRef(u32);

/// A long-lived buffer adopted from outside, bound whole as read-only storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufRef(u32);

/// A resource bound to a pass. Bind groups are cached by everything here but a uniform's offset,
/// which is dynamic, so uniforms moving in the bump buffer don't rebuild them.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Res {
    Tex(TexRef),
    Uniform { offset: u32, size: u32 },
    Storage { offset: u32, size: u32 },
    Sampler(SamplerRef),
    Buffer(BufRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum KeyRes {
    Tex(TexRef),
    Uniform { size: u32 },
    Storage { offset: u32, size: u32 },
    Sampler(SamplerRef),
    Buffer(BufRef),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct BindKey {
    program: ProgramRef,
    count: u8,
    entries: [(u32, KeyRes); MAX_PASS_BINDINGS],
}

impl Hash for BindKey {
    fn hash<H: std::hash::Hasher>(&self, state: &mut H) {
        self.program.hash(state);
        self.entries[..self.count as usize].hash(state);
    }
}

#[derive(Clone, Copy)]
struct PassBindings {
    count: u8,
    entries: [(u32, Res); MAX_PASS_BINDINGS],
}

impl PassBindings {
    fn new(bindings: &[(u32, Res)]) -> Self {
        let mut entries = [(0, Res::Sampler(SamplerRef(0))); MAX_PASS_BINDINGS];
        entries[..bindings.len()].copy_from_slice(bindings);
        // Dynamic offsets apply in binding order.
        entries[..bindings.len()].sort_unstable_by_key(|(binding, _)| *binding);
        Self {
            count: bindings.len() as u8,
            entries,
        }
    }

    fn as_slice(&self) -> &[(u32, Res)] {
        &self.entries[..self.count as usize]
    }

    fn key(&self, program: ProgramRef) -> BindKey {
        let mut entries = [(0, KeyRes::Sampler(SamplerRef(0))); MAX_PASS_BINDINGS];
        for (slot, (binding, res)) in entries.iter_mut().zip(self.as_slice()) {
            *slot = (
                *binding,
                match *res {
                    Res::Tex(tex) => KeyRes::Tex(tex),
                    Res::Uniform { size, .. } => KeyRes::Uniform { size },
                    Res::Storage { offset, size } => KeyRes::Storage { offset, size },
                    Res::Sampler(sampler) => KeyRes::Sampler(sampler),
                    Res::Buffer(buffer) => KeyRes::Buffer(buffer),
                },
            );
        }
        BindKey {
            program,
            count: self.count,
            entries,
        }
    }
}

#[derive(Clone, Copy)]
pub enum TargetLoad {
    Clear,
    Load,
}

#[derive(Clone, Copy)]
enum RecordedPass {
    Compute {
        program: ProgramRef,
        bindings: PassBindings,
        dispatch: [u32; 3],
    },
    Render {
        program: ProgramRef,
        bindings: PassBindings,
        /// `None` for a pass that only draws depth.
        target: Option<TexRef>,
        load: TargetLoad,
        depth: Option<(TexRef, TargetLoad)>,
        vertices: u32,
        instances: u32,
        viewport: Option<[f32; 4]>,
    },
    Copy {
        source: TexRef,
        destination: TexRef,
        origin: [u32; 2],
        size: Size,
    },
    Clear {
        texture: TexRef,
    },
}

impl RecordedPass {
    /// Calls `visit` with every texture the pass reads or writes.
    fn textures_mut(&mut self, mut visit: impl FnMut(&mut TexRef)) {
        let mut bound = |bindings: &mut PassBindings| {
            for (_, res) in &mut bindings.entries[..bindings.count as usize] {
                if let Res::Tex(tex) = res {
                    visit(tex);
                }
            }
        };
        match self {
            Self::Compute { bindings, .. } => bound(bindings),
            Self::Render {
                bindings,
                target,
                depth,
                ..
            } => {
                bound(bindings);
                if let Some(target) = target {
                    visit(target);
                }
                if let Some((depth, _)) = depth {
                    visit(depth);
                }
            }
            Self::Copy {
                source,
                destination,
                ..
            } => {
                visit(source);
                visit(destination);
            }
            Self::Clear { texture } => visit(texture),
        }
    }
}

/// Counters of what recording cost, cumulative since the recorder was made.
#[derive(Debug, Clone, Copy, Default)]
pub struct RecorderStats {
    pub passes: u64,
    pub bind_group_hits: u64,
    pub bind_group_misses: u64,
    pub programs_created: u64,
    pub textures_created: u64,
    pub clears: u64,
    pub uniform_bytes: u64,
    pub storage_bytes: u64,
    pub textures_destroyed: u64,
    /// The most bytes of pooled textures one frame's passes needed at the same time.
    pub peak_frame_bytes: u64,
    /// What those would have needed without sharing textures within the frame.
    pub peak_unshared_bytes: u64,
    /// Outputs copied to an external target rather than rendered into it.
    pub external_copies: u64,
}

pub struct Recorder {
    device: wgpu::Device,
    queue: wgpu::Queue,
    frame: u64,
    entries: Vec<Option<PoolEntry>>,
    free_slots: Vec<u32>,
    buckets: foldhash::HashMap<PoolKey, PoolBucket>,
    pool_bytes: u64,
    /// Bytes of adopted textures, which the budget doesn't count.
    adopted_bytes: u64,
    budget: u64,
    frame_textures: Vec<FrameTexture>,
    /// The widest or tallest texture acquired since it was last replaced.
    largest_texture: Size,
    programs: Vec<Option<ProgramEntry>>,
    program_keys: foldhash::HashMap<ProgramKey, ProgramRef>,
    /// Programs by what they're built from. A `'static` shader isn't guaranteed one address, so
    /// a second key for the same program finds it here.
    program_contents: foldhash::HashMap<String, ProgramRef>,
    samplers: Vec<(u64, wgpu::Sampler)>,
    buffers: Vec<Option<Arc<wgpu::Buffer>>>,
    uniforms: Arena,
    storage: Arena,
    groups: Vec<Option<(wgpu::BindGroup, u64)>>,
    free_groups: Vec<u32>,
    group_keys: foldhash::HashMap<BindKey, u32>,
    passes: Vec<RecordedPass>,
    resolved: Vec<u32>,
    stats: RecorderStats,
}

impl Recorder {
    pub fn new(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        let limits = device.limits();
        Self {
            device,
            queue,
            frame: 0,
            entries: Vec::new(),
            free_slots: Vec::new(),
            buckets: foldhash::HashMap::default(),
            pool_bytes: 0,
            adopted_bytes: 0,
            budget: DEFAULT_POOL_BUDGET_BYTES,
            frame_textures: Vec::new(),
            largest_texture: Size::new(0, 0),
            programs: Vec::new(),
            program_keys: foldhash::HashMap::default(),
            program_contents: foldhash::HashMap::default(),
            samplers: Vec::new(),
            buffers: Vec::new(),
            uniforms: Arena::new(
                "lumen frame graph uniforms",
                wgpu::BufferUsages::UNIFORM,
                limits.max_uniform_buffer_binding_size,
                limits.max_buffer_size,
            ),
            storage: Arena::new(
                "lumen frame graph storage",
                wgpu::BufferUsages::STORAGE,
                limits.max_storage_buffer_binding_size,
                limits.max_buffer_size,
            ),
            groups: Vec::new(),
            free_groups: Vec::new(),
            group_keys: foldhash::HashMap::default(),
            passes: Vec::new(),
            resolved: Vec::new(),
            stats: RecorderStats::default(),
        }
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    pub fn stats(&self) -> RecorderStats {
        self.stats
    }

    /// Bytes of pooled textures, in rotation or retained.
    pub fn pool_bytes(&self) -> u64 {
        self.pool_bytes
    }

    /// Sets the bytes of pooled textures kept for later frames. Those the latest frame used and
    /// those retained don't count toward it.
    pub fn set_pool_budget(&mut self, bytes: u64) {
        self.budget = bytes;
    }

    /// Bytes of textures this recorder holds, in rotation, retained or adopted, and of its bump
    /// buffers.
    pub fn held_bytes(&self) -> u64 {
        self.pool_bytes + self.adopted_bytes + self.uniforms.capacity + self.storage.capacity
    }

    pub fn bind_group_count(&self) -> usize {
        self.group_keys.len()
    }

    pub fn program_count(&self) -> usize {
        self.programs.iter().flatten().count()
    }

    /// Starts recording a frame, discarding what an abandoned frame recorded.
    pub fn begin_frame(&mut self) {
        self.frame += 1;
        self.passes.clear();
        self.frame_textures.clear();
        self.largest_texture = Size::new(0, 0);
        self.uniforms.cpu.clear();
        self.uniforms.overflowed = false;
        self.storage.cpu.clear();
        self.storage.overflowed = false;
        for bucket in self.buckets.values_mut() {
            bucket.next = 0;
            bucket.released.clear();
        }
    }

    /// A cached sampler, identified by `key`.
    pub fn sampler(
        &mut self,
        key: u64,
        desc: impl FnOnce() -> wgpu::SamplerDescriptor<'static>,
    ) -> SamplerRef {
        if let Some(index) = self.samplers.iter().position(|(other, _)| *other == key) {
            return SamplerRef(index as u32);
        }
        self.samplers
            .push((key, self.device.create_sampler(&desc())));
        SamplerRef(self.samplers.len() as u32 - 1)
    }

    pub fn compute(&mut self, program: ProgramRef, bindings: &[(u32, Res)], dispatch: Dispatch) {
        self.pin_program(program);
        self.passes.push(RecordedPass::Compute {
            program,
            bindings: PassBindings::new(bindings),
            dispatch: [dispatch.x, dispatch.y, dispatch.z],
        });
    }

    #[allow(clippy::too_many_arguments, reason = "a draw's flat inputs")]
    pub fn draw(
        &mut self,
        program: ProgramRef,
        bindings: &[(u32, Res)],
        target: TexRef,
        load: TargetLoad,
        vertices: u32,
        instances: u32,
        viewport: Option<[f32; 4]>,
    ) {
        self.pin_program(program);
        self.passes.push(RecordedPass::Render {
            program,
            bindings: PassBindings::new(bindings),
            target: Some(target),
            load,
            depth: None,
            vertices,
            instances,
            viewport,
        });
    }

    /// `draw` into a pass that also tests against, and may write, `depth`, a `DEPTH_FORMAT`
    /// texture of the target's size.
    #[allow(clippy::too_many_arguments, reason = "a draw's flat inputs")]
    pub fn draw_with_depth(
        &mut self,
        program: ProgramRef,
        bindings: &[(u32, Res)],
        target: TexRef,
        load: TargetLoad,
        depth: TexRef,
        depth_load: TargetLoad,
        vertices: u32,
        instances: u32,
    ) {
        self.pin_program(program);
        self.passes.push(RecordedPass::Render {
            program,
            bindings: PassBindings::new(bindings),
            target: Some(target),
            load,
            depth: Some((depth, depth_load)),
            vertices,
            instances,
            viewport: None,
        });
    }

    /// A draw into `depth` alone, a `DEPTH_FORMAT` texture, with a program made with
    /// `RenderTargetSpec::depth_only`.
    pub fn draw_depth(
        &mut self,
        program: ProgramRef,
        bindings: &[(u32, Res)],
        depth: TexRef,
        load: TargetLoad,
        vertices: u32,
        instances: u32,
    ) {
        self.pin_program(program);
        self.passes.push(RecordedPass::Render {
            program,
            bindings: PassBindings::new(bindings),
            target: None,
            load: TargetLoad::Load,
            depth: Some((depth, load)),
            vertices,
            instances,
            viewport: None,
        });
    }

    /// Takes `buffer` in to bind as storage until `forget_buffer`.
    pub fn adopt_buffer(&mut self, buffer: Arc<wgpu::Buffer>) -> BufRef {
        if let Some(slot) = self.buffers.iter().position(Option::is_none) {
            self.buffers[slot] = Some(buffer);
            return BufRef(slot as u32);
        }
        self.buffers.push(Some(buffer));
        BufRef(self.buffers.len() as u32 - 1)
    }

    /// Lets go of an adopted buffer and the bind groups holding it.
    pub fn forget_buffer(&mut self, buffer: BufRef) {
        self.buffers[buffer.0 as usize] = None;
        self.purge_groups(|res| matches!(res, KeyRes::Buffer(held) if *held == buffer));
    }

    pub fn copy(&mut self, source: TexRef, destination: TexRef, origin: [u32; 2], size: Size) {
        self.passes.push(RecordedPass::Copy {
            source,
            destination,
            origin,
            size,
        });
    }

    /// Clears a reused texture whose unwritten texels are read as transparent.
    pub fn clear(&mut self, texture: TexRef) {
        self.stats.clears += 1;
        self.passes.push(RecordedPass::Clear { texture });
    }

    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    /// Destroys every texture only this recorder holds and drops all programs, bind groups,
    /// samplers and bump buffers. Refs taken before this are no longer valid, so nothing may
    /// still be retained or recorded, and the next frame starts from nothing.
    pub fn release_all(&mut self) {
        for entry in self.entries.drain(..).flatten() {
            if Arc::strong_count(&entry.texture) == 1 {
                entry.texture.destroy();
            }
            if !entry.external {
                self.stats.textures_destroyed += 1;
            }
        }
        self.free_slots = Vec::new();
        self.buckets = foldhash::HashMap::default();
        self.pool_bytes = 0;
        self.adopted_bytes = 0;
        self.frame_textures = Vec::new();
        self.programs = Vec::new();
        self.program_keys = foldhash::HashMap::default();
        self.program_contents = foldhash::HashMap::default();
        self.samplers = Vec::new();
        self.buffers = Vec::new();
        self.uniforms.release();
        self.storage.release();
        self.groups = Vec::new();
        self.free_groups = Vec::new();
        self.group_keys = foldhash::HashMap::default();
        self.passes = Vec::new();
        self.resolved = Vec::new();
    }
}
