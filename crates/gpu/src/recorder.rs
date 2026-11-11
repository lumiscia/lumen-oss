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

use std::{hash::Hash, sync::Arc};

use anyhow::{Result, anyhow};

use crate::{Size, renderer::ValidationScope};

/// The workgroup counts of a compute pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dispatch {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

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
pub const MAX_PASS_BINDINGS: usize = 28;
const BUMP_ALIGNMENT: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TexRef(u32);

impl TexRef {
    fn frame_index(self) -> Option<usize> {
        (self.0 & FRAME_TEXTURE != 0).then_some((self.0 & !FRAME_TEXTURE) as usize)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramRef(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SamplerRef(u32);

/// A long-lived buffer adopted from outside, bound whole as read-only storage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BufRef(u32);

#[derive(Debug, Clone, Copy)]
pub enum LayoutKind {
    Texture,
    /// A float texture read with `textureLoad`, such as `R32Float`, which can't be filtered.
    UnfilterableTexture,
    /// An unsigned-integer texture, read with `textureLoad`.
    UintTexture,
    /// A depth texture, read with a comparison sampler.
    DepthTexture,
    StorageTexture(wgpu::TextureFormat),
    /// Bound with a dynamic offset into the uniform bump buffer.
    Uniform,
    Storage,
    Sampler,
    /// A sampler that compares depth, made with a `compare` function.
    ComparisonSampler,
}

#[derive(Debug, Clone, Copy)]
pub struct LayoutEntry {
    pub binding: u32,
    pub visibility: wgpu::ShaderStages,
    pub kind: LayoutKind,
}

impl LayoutEntry {
    pub const fn compute(binding: u32, kind: LayoutKind) -> Self {
        Self {
            binding,
            visibility: wgpu::ShaderStages::COMPUTE,
            kind,
        }
    }

    pub const fn staged(binding: u32, visibility: wgpu::ShaderStages, kind: LayoutKind) -> Self {
        Self {
            binding,
            visibility,
            kind,
        }
    }
}

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

/// A render pass kept open at submit so later draws onto the same attachments join it.
struct OpenRender {
    pass: wgpu::RenderPass<'static>,
    target: Option<TexRef>,
    depth: Option<TexRef>,
    viewport_set: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct PoolKey {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
}

struct PoolEntry {
    texture: Arc<wgpu::Texture>,
    view: wgpu::TextureView,
    key: PoolKey,
    bytes: u64,
    last_used: u64,
    /// Holders of the texture outside the pool, which leave it out of rotation until released.
    retained: u32,
    /// Adopted from outside, never handed out by `acquire`.
    external: bool,
}

/// Textures of one size and format, handed out in the same order every frame so that a stable
/// frame finds the same textures in the same slots, and its bind groups with them.
#[derive(Default)]
struct PoolBucket {
    textures: Vec<TexRef>,
    /// The first `next` of `textures` were handed out this frame.
    next: usize,
    /// Those whose frame texture has had its last use, most recently freed last.
    released: Vec<TexRef>,
}

/// A texture of the frame being recorded.
struct FrameTexture {
    key: PoolKey,
    physical: Option<TexRef>,
    /// Holds taken with `retain` before it had a pooled texture.
    retains: u32,
    /// Written outside the passes, before they run, so it cannot share with an earlier pass.
    written_early: bool,
    /// The last pass using it.
    last_use: usize,
    /// Its pooled texture is back in the bucket.
    released: bool,
}

struct Arena {
    label: &'static str,
    usage: wgpu::BufferUsages,
    cpu: Vec<u8>,
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
    /// The largest range one binding may cover.
    max_binding: u64,
    /// The largest the buffer may grow.
    max_buffer: u64,
    /// The frame asked for more than the device takes, so it must not be uploaded.
    overflowed: bool,
}

impl Arena {
    fn new(
        label: &'static str,
        usage: wgpu::BufferUsages,
        max_binding: u64,
        max_buffer: u64,
    ) -> Self {
        Self {
            label,
            usage,
            cpu: Vec::new(),
            buffer: None,
            capacity: 0,
            max_binding,
            // Offsets are `u32`.
            max_buffer: max_buffer.min(1 << 31),
            overflowed: false,
        }
    }

    /// A zeroed range of `size` bytes, or `None`, marking the frame as too large, when the
    /// device can't bind that much or the buffer can't grow to hold it.
    fn alloc(&mut self, size: u64) -> Option<(u32, &mut [u8])> {
        let offset = (self.cpu.len() as u64).next_multiple_of(u64::from(BUMP_ALIGNMENT));
        if size > self.max_binding || Self::capacity_for(offset + size) > self.max_buffer {
            self.overflowed = true;
            return None;
        }
        self.cpu.resize((offset + size) as usize, 0);
        Some((offset as u32, &mut self.cpu[offset as usize..]))
    }

    /// The buffer size that holds `bytes` of frame data.
    fn capacity_for(bytes: u64) -> u64 {
        bytes.next_power_of_two().max(1 << 16)
    }

    /// Writes this frame's data, growing the buffer first when needed. Returns whether it grew,
    /// which invalidates bind groups on the old one.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let needed = (self.cpu.len() as u64).max(u64::from(BUMP_ALIGNMENT));
        let grew = needed > self.capacity;
        if grew {
            self.capacity = Self::capacity_for(needed);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.capacity,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if !self.cpu.is_empty() {
            queue.write_buffer(self.buffer.as_ref().expect("arena buffer"), 0, &self.cpu);
        }
        grew
    }

    fn release(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            buffer.destroy();
        }
        self.cpu = Vec::new();
        self.capacity = 0;
    }
}

enum RuntimeProgram {
    Compute(wgpu::ComputePipeline, wgpu::BindGroupLayout),
    Render(wgpu::RenderPipeline, wgpu::BindGroupLayout),
}

struct ProgramEntry {
    program: RuntimeProgram,
    last_used: u64,
    dynamic: bool,
}

/// What a program is cached by: a static shader's identity, which needs no hashing per pass, or
/// the hash of a runtime source, plus entry points and whatever else varies its pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
struct ProgramKey {
    shader: (usize, u64),
    entry: usize,
    variant: u64,
    format: Option<wgpu::TextureFormat>,
    depth: Option<DepthTest>,
    cull: Option<wgpu::Face>,
}

/// The format of every depth attachment a render pass draws with.
pub const DEPTH_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Depth32Float;

/// How a render program uses its pass's depth attachment, testing closer-than.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DepthTest {
    /// Tests and writes depth, for opaque geometry.
    Write,
    /// Tests without writing, for blended geometry drawn after the opaque.
    Read,
}

/// How a program's pipeline draws, for render programs.
#[derive(Debug, Clone, Copy)]
pub struct RenderTargetSpec {
    /// The colour target's format, or `None` for a program that only draws depth.
    pub format: Option<wgpu::TextureFormat>,
    pub blend: wgpu::BlendState,
    /// Set for programs drawn into a pass with a depth attachment.
    pub depth: Option<DepthTest>,
    pub cull: Option<wgpu::Face>,
}

impl RenderTargetSpec {
    /// A target without depth or culling.
    pub const fn flat(format: wgpu::TextureFormat, blend: wgpu::BlendState) -> Self {
        Self {
            format: Some(format),
            blend,
            depth: None,
            cull: None,
        }
    }

    /// A program drawing only into a depth attachment, for `draw_depth`.
    pub const fn depth_only(depth: DepthTest, cull: Option<wgpu::Face>) -> Self {
        Self {
            format: None,
            blend: wgpu::BlendState::REPLACE,
            depth: Some(depth),
            cull,
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

    /// Sets the bytes of pooled textures kept once frames no longer use them.
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

    /// A texture of this size, format and usage for this frame's passes, valid in them until
    /// `resolve`. Its contents are undefined, possibly another intermediate's pixels: whoever
    /// writes it must write everything later read, or `clear` it first.
    pub fn acquire(
        &mut self,
        size: Size,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> TexRef {
        if size.width.max(size.height) > self.largest_texture.width.max(self.largest_texture.height)
        {
            self.largest_texture = size;
        }
        self.frame_textures.push(FrameTexture {
            key: PoolKey {
                width: size.width.max(1),
                height: size.height.max(1),
                format,
                usage,
            },
            physical: None,
            retains: 0,
            written_early: false,
            last_use: 0,
            released: false,
        });
        TexRef(FRAME_TEXTURE | (self.frame_textures.len() as u32 - 1))
    }

    /// Sets the widest or tallest texture acquired so far to `size` and returns what it was.
    /// `begin_frame` resets it to zero by zero.
    pub fn replace_largest_acquired_texture(&mut self, size: Size) -> Size {
        std::mem::replace(&mut self.largest_texture, size)
    }

    /// `acquire` for a texture written between `resolve` and the passes running.
    pub fn acquire_written_early(
        &mut self,
        size: Size,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> TexRef {
        let tex = self.acquire(size, format, usage);
        self.frame_textures
            .last_mut()
            .expect("frame texture")
            .written_early = true;
        tex
    }

    /// A pooled texture this frame hasn't handed out, or one no later pass reads.
    fn take_pooled(&mut self, key: PoolKey) -> TexRef {
        let frame = self.frame;
        let bucket = self.buckets.entry(key).or_default();
        if let Some(tex) = bucket.released.pop() {
            return tex;
        }
        if let Some(&tex) = bucket.textures.get(bucket.next) {
            bucket.next += 1;
            self.entries[tex.0 as usize]
                .as_mut()
                .expect("pooled texture")
                .last_used = frame;
            return tex;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lumen frame graph target"),
            size: wgpu::Extent3d {
                width: key.width,
                height: key.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: key.format,
            usage: key.usage,
            view_formats: &[],
        });
        self.stats.textures_created += 1;
        let tex = self.insert(Arc::new(texture), key, false);
        let bucket = self.buckets.get_mut(&key).expect("bucket");
        bucket.textures.push(tex);
        bucket.next = bucket.textures.len();
        tex
    }

    /// Gives each of this frame's textures a pooled texture for the passes using it, and rewrites
    /// the passes to them. `output` stays intact after the frame. When `external` can stand in for
    /// it, its passes write `external` directly; otherwise a last pass copies it there.
    pub fn resolve(&mut self, output: TexRef, external: Option<TexRef>) -> Result<TexRef> {
        let output_frame = output.frame_index();
        if let Some(external) = external {
            let target = self.entry(external);
            let (target_key, target_usage) = (target.key, target.key.usage);
            let output_key = match output_frame {
                Some(index) => self.frame_textures[index].key,
                None => self.entry(output).key,
            };
            if (target_key.width, target_key.height) != (output_key.width, output_key.height) {
                return Err(anyhow!(
                    "the external texture has size {}x{}, expected {}x{}",
                    target_key.width,
                    target_key.height,
                    output_key.width,
                    output_key.height
                ));
            }
            if target_key.format != output_key.format {
                return Err(anyhow!(
                    "the external texture has format {:?}, expected {:?}",
                    target_key.format,
                    output_key.format
                ));
            }
            let stands_in = output_frame.is_some_and(|index| {
                let texture = &self.frame_textures[index];
                texture.retains == 0
                    && !texture.written_early
                    && (texture.key.width, texture.key.height)
                        == (target_key.width, target_key.height)
                    && texture.key.format == target_key.format
                    && target_usage.contains(texture.key.usage)
            });
            match output_frame {
                Some(index) if stands_in => self.frame_textures[index].physical = Some(external),
                _ => {
                    if !target_usage.contains(wgpu::TextureUsages::COPY_DST) {
                        return Err(anyhow!(
                            "the external texture can't take a kept output: it has usage {target_usage:?}, without COPY_DST"
                        ));
                    }
                    let size = Size::new(target_key.width, target_key.height);
                    self.copy(output, external, [0, 0], size);
                    self.stats.external_copies += 1;
                }
            }
        }

        let mut passes = std::mem::take(&mut self.passes);
        for (index, pass) in passes.iter_mut().enumerate() {
            let textures = &mut self.frame_textures;
            pass.textures_mut(|tex| {
                if let Some(frame) = tex.frame_index() {
                    textures[frame].last_use = index;
                }
            });
        }
        let mut live_bytes = 0;
        let mut peak_bytes = 0;
        let mut unshared_bytes = 0;
        for index in 0..self.frame_textures.len() {
            if self.frame_textures[index].written_early {
                self.assign(index, &mut live_bytes, &mut peak_bytes, &mut unshared_bytes);
            }
        }
        let mut ended = Vec::new();
        for (index, pass) in passes.iter_mut().enumerate() {
            ended.clear();
            pass.textures_mut(|tex| {
                let Some(frame) = tex.frame_index() else {
                    return;
                };
                self.assign(frame, &mut live_bytes, &mut peak_bytes, &mut unshared_bytes);
                *tex = self.frame_textures[frame].physical.expect("assigned");
                if self.frame_textures[frame].last_use == index {
                    ended.push(frame);
                }
            });
            for frame in ended.drain(..) {
                let texture = &mut self.frame_textures[frame];
                if Some(frame) == output_frame || texture.retains > 0 || texture.released {
                    continue;
                }
                texture.released = true;
                let (key, tex) = (texture.key, texture.physical.expect("assigned"));
                live_bytes -= pool_entry_bytes(key);
                self.buckets
                    .get_mut(&key)
                    .expect("bucket")
                    .released
                    .push(tex);
            }
        }
        self.passes = passes;
        // What no pass uses still has to exist when something reads it after the frame.
        for index in 0..self.frame_textures.len() {
            if Some(index) == output_frame || self.frame_textures[index].retains > 0 {
                self.assign(index, &mut live_bytes, &mut peak_bytes, &mut unshared_bytes);
            }
        }
        for index in 0..self.frame_textures.len() {
            let texture = &self.frame_textures[index];
            let (retains, physical) = (texture.retains, texture.physical);
            if let Some(physical) = physical {
                for _ in 0..retains {
                    self.retain(physical);
                }
            }
        }
        self.stats.peak_frame_bytes = self.stats.peak_frame_bytes.max(peak_bytes);
        self.stats.peak_unshared_bytes = self.stats.peak_unshared_bytes.max(unshared_bytes);
        Ok(self.physical(output))
    }

    fn assign(
        &mut self,
        frame: usize,
        live_bytes: &mut u64,
        peak_bytes: &mut u64,
        unshared_bytes: &mut u64,
    ) {
        if self.frame_textures[frame].physical.is_some() {
            return;
        }
        let key = self.frame_textures[frame].key;
        let tex = self.take_pooled(key);
        self.frame_textures[frame].physical = Some(tex);
        let bytes = pool_entry_bytes(key);
        *live_bytes += bytes;
        *peak_bytes = (*peak_bytes).max(*live_bytes);
        *unshared_bytes += bytes;
    }

    /// The pooled or adopted texture `tex` names once the frame is resolved.
    pub fn physical(&self, tex: TexRef) -> TexRef {
        match tex.frame_index() {
            Some(index) => self.frame_textures[index]
                .physical
                .expect("a frame texture nothing used or kept"),
            None => tex,
        }
    }

    /// Registers a texture made elsewhere, such as an atlas or image, for binding.
    pub fn adopt(&mut self, texture: Arc<wgpu::Texture>) -> TexRef {
        let key = PoolKey {
            width: texture.width(),
            height: texture.height(),
            format: texture.format(),
            usage: texture.usage(),
        };
        self.insert(texture, key, true)
    }

    fn insert(&mut self, texture: Arc<wgpu::Texture>, key: PoolKey, external: bool) -> TexRef {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bytes = pool_entry_bytes(key);
        if external {
            self.adopted_bytes += bytes;
        } else {
            self.pool_bytes += bytes;
        }
        let entry = PoolEntry {
            texture,
            view,
            key,
            bytes,
            last_used: self.frame,
            retained: 0,
            external,
        };
        match self.free_slots.pop() {
            Some(slot) => {
                self.entries[slot as usize] = Some(entry);
                TexRef(slot)
            }
            None => {
                self.entries.push(Some(entry));
                TexRef(self.entries.len() as u32 - 1)
            }
        }
    }

    /// Takes a pooled texture out of the pool without destroying it, so whoever holds it keeps its
    /// pixels. Does nothing to an adopted texture.
    pub fn disown(&mut self, tex: TexRef) {
        let Some(entry) = &self.entries[tex.0 as usize] else {
            return;
        };
        if entry.external {
            return;
        }
        let (key, bytes) = (entry.key, entry.bytes);
        if let Some(bucket) = self.buckets.get_mut(&key) {
            bucket.textures.retain(|other| *other != tex);
            bucket.released.retain(|other| *other != tex);
            bucket.next = bucket.next.min(bucket.textures.len());
        }
        self.pool_bytes -= bytes;
        self.entries[tex.0 as usize] = None;
        self.free_slots.push(tex.0);
        self.purge_groups(|res| matches!(res, KeyRes::Tex(other) if *other == tex));
    }

    /// Forgets a texture from `adopt`, along with the bind groups using it.
    pub fn forget(&mut self, tex: TexRef) {
        self.remove_entry(tex);
        self.purge_groups(|res| matches!(res, KeyRes::Tex(other) if *other == tex));
    }

    fn remove_entry(&mut self, tex: TexRef) {
        if let Some(entry) = self.entries[tex.0 as usize].take() {
            self.free_slots.push(tex.0);
            if entry.external {
                self.adopted_bytes -= entry.bytes;
            } else {
                self.pool_bytes -= entry.bytes;
                self.stats.textures_destroyed += 1;
                entry.texture.destroy();
            }
        }
    }

    /// Takes the texture out of rotation until as many `release` calls follow, so its contents
    /// survive later frames.
    pub fn retain(&mut self, tex: TexRef) {
        if let Some(index) = tex.frame_index() {
            let texture = &mut self.frame_textures[index];
            if texture.physical.is_none() {
                texture.retains += 1;
                return;
            }
        }
        let tex = self.physical(tex);
        let entry = self.entries[tex.0 as usize].as_mut().expect("texture");
        entry.retained += 1;
        let (first, external, key) = (entry.retained == 1, entry.external, entry.key);
        if first
            && !external
            && let Some(bucket) = self.buckets.get_mut(&key)
            && let Some(index) = bucket.textures.iter().position(|other| *other == tex)
        {
            bucket.textures.remove(index);
            if index < bucket.next {
                bucket.next -= 1;
            }
        }
    }

    pub fn release(&mut self, tex: TexRef) {
        if let Some(index) = tex.frame_index() {
            let texture = &mut self.frame_textures[index];
            if texture.physical.is_none() {
                texture.retains -= 1;
                return;
            }
        }
        let tex = self.physical(tex);
        let entry = self.entries[tex.0 as usize].as_mut().expect("texture");
        entry.retained -= 1;
        entry.last_used = self.frame;
        if entry.retained == 0 && !entry.external {
            let key = entry.key;
            self.buckets.entry(key).or_default().textures.push(tex);
        }
    }

    /// A resolved texture.
    pub fn texture(&self, tex: TexRef) -> &Arc<wgpu::Texture> {
        &self.entry(self.physical(tex)).texture
    }

    pub fn adopted_bytes(&self) -> u64 {
        self.adopted_bytes
    }

    pub fn texture_bytes(&self, tex: TexRef) -> u64 {
        match tex.frame_index() {
            Some(index) => pool_entry_bytes(self.frame_textures[index].key),
            None => self.entry(tex).bytes,
        }
    }

    pub fn usage(&self, tex: TexRef) -> wgpu::TextureUsages {
        match tex.frame_index() {
            Some(index) => self.frame_textures[index].key.usage,
            None => self.entry(tex).key.usage,
        }
    }

    fn entry(&self, tex: TexRef) -> &PoolEntry {
        self.entries[tex.0 as usize].as_ref().expect("texture")
    }

    /// A program built from a `'static` shader, cached by that shader's identity.
    pub fn compute_program(
        &mut self,
        label: &str,
        shader: &'static str,
        entry: &'static str,
        layout: &'static [LayoutEntry],
    ) -> Result<ProgramRef> {
        let key = ProgramKey {
            shader: (shader.as_ptr() as usize, shader.len() as u64),
            entry: entry.as_ptr() as usize,
            variant: layout.as_ptr() as u64,
            format: None,
            depth: None,
            cull: None,
        };
        self.compute_with_key(key, label, || shader.to_string(), entry, layout, false)
    }

    /// A program built from a shader source only known at runtime, cached by a hash of what
    /// varies it, so an expression-selected source compiles once per distinct source.
    pub fn dynamic_compute_program(
        &mut self,
        label: &str,
        source_hash: u64,
        shader: impl FnOnce() -> String,
        entry: &'static str,
        layout: &[LayoutEntry],
    ) -> Result<ProgramRef> {
        let key = ProgramKey {
            shader: (0, source_hash),
            entry: entry.as_ptr() as usize,
            variant: layout.len() as u64,
            format: None,
            depth: None,
            cull: None,
        };
        self.compute_with_key(key, label, shader, entry, layout, true)
    }

    fn compute_with_key(
        &mut self,
        key: ProgramKey,
        label: &str,
        shader: impl FnOnce() -> String,
        entry: &str,
        layout: &[LayoutEntry],
        dynamic: bool,
    ) -> Result<ProgramRef> {
        if let Some(&program) = self.program_keys.get(&key) {
            self.programs[program.0 as usize]
                .as_mut()
                .expect("program")
                .last_used = self.frame;
            return Ok(program);
        }
        let source = shader();
        let contents = (!dynamic).then(|| format!("{entry}{layout:?}{source}"));
        if let Some(contents) = &contents
            && let Some(&program) = self.program_contents.get(contents)
        {
            self.program_keys.insert(key, program);
            return Ok(program);
        }
        let scope = ValidationScope::push(&self.device);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
        let group = self.bind_group_layout(layout);
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&group)],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &module,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
        scope.finish()?;
        let program = self.insert_program(key, RuntimeProgram::Compute(pipeline, group), dynamic);
        if let Some(contents) = contents {
            self.program_contents.insert(contents, program);
        }
        Ok(program)
    }

    /// A render program built from a `'static` shader, cached by that shader's identity and
    /// `variant`, which names its blend and target format. Its layout can't hold a storage
    /// texture: draws share a render pass, where a texture written by one and read by another
    /// would conflict.
    #[allow(
        clippy::too_many_arguments,
        reason = "a render program's parts, cached together"
    )]
    pub fn render_program(
        &mut self,
        label: &str,
        shader: &'static str,
        vertex: &'static str,
        fragment: &'static str,
        layout: &'static [LayoutEntry],
        target: RenderTargetSpec,
        variant: u64,
    ) -> Result<ProgramRef> {
        let key = ProgramKey {
            shader: (shader.as_ptr() as usize, shader.len() as u64),
            entry: fragment.as_ptr() as usize,
            variant: variant ^ (vertex.as_ptr() as u64),
            format: target.format,
            depth: target.depth,
            cull: target.cull,
        };
        if let Some(&program) = self.program_keys.get(&key) {
            return Ok(program);
        }
        if layout
            .iter()
            .any(|entry| matches!(entry.kind, LayoutKind::StorageTexture(_)))
        {
            return Err(anyhow!("render program `{label}` binds a storage texture"));
        }
        let contents = format!(
            "{vertex}{fragment}{variant}{:?}{:?}{:?}{:?}{layout:?}{shader}",
            target.format, target.blend, target.depth, target.cull
        );
        if let Some(&program) = self.program_contents.get(&contents) {
            self.program_keys.insert(key, program);
            return Ok(program);
        }
        let scope = ValidationScope::push(&self.device);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(shader.into()),
            });
        let color_target = target.format.map(|format| wgpu::ColorTargetState {
            format,
            // Replacing is the same as not blending, which unblendable formats need.
            blend: (target.blend != wgpu::BlendState::REPLACE).then_some(target.blend),
            write_mask: wgpu::ColorWrites::ALL,
        });
        let color_targets = match color_target {
            Some(_) => std::slice::from_ref(&color_target),
            None => &[],
        };
        let group = self.bind_group_layout(layout);
        let pipeline_layout = self
            .device
            .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&group)],
                immediate_size: 0,
            });
        let pipeline = self
            .device
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState {
                    cull_mode: target.cull,
                    ..Default::default()
                },
                depth_stencil: target.depth.map(|depth| wgpu::DepthStencilState {
                    format: DEPTH_FORMAT,
                    depth_write_enabled: Some(depth == DepthTest::Write),
                    depth_compare: Some(wgpu::CompareFunction::Less),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: color_targets,
                }),
                multiview_mask: None,
                cache: None,
            });
        scope.finish()?;
        let program = self.insert_program(key, RuntimeProgram::Render(pipeline, group), false);
        self.program_contents.insert(contents, program);
        Ok(program)
    }

    fn bind_group_layout(&self, layout: &[LayoutEntry]) -> wgpu::BindGroupLayout {
        let entries: Vec<_> = layout
            .iter()
            .map(|entry| wgpu::BindGroupLayoutEntry {
                binding: entry.binding,
                visibility: entry.visibility,
                ty: match entry.kind {
                    LayoutKind::Texture => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    LayoutKind::UnfilterableTexture => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    LayoutKind::UintTexture => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Uint,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    LayoutKind::DepthTexture => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Depth,
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    LayoutKind::StorageTexture(format) => wgpu::BindingType::StorageTexture {
                        access: wgpu::StorageTextureAccess::WriteOnly,
                        format,
                        view_dimension: wgpu::TextureViewDimension::D2,
                    },
                    LayoutKind::Uniform => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: true,
                        min_binding_size: None,
                    },
                    LayoutKind::Storage => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: true },
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    LayoutKind::Sampler => {
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering)
                    }
                    LayoutKind::ComparisonSampler => {
                        wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Comparison)
                    }
                },
                count: None,
            })
            .collect();
        self.device
            .create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: None,
                entries: &entries,
            })
    }

    fn insert_program(
        &mut self,
        key: ProgramKey,
        program: RuntimeProgram,
        dynamic: bool,
    ) -> ProgramRef {
        self.stats.programs_created += 1;
        if dynamic {
            self.evict_dynamic_programs(MAX_DYNAMIC_PROGRAMS - 1);
        }
        let entry = ProgramEntry {
            program,
            last_used: self.frame,
            dynamic,
        };
        let id = match self.programs.iter().position(Option::is_none) {
            Some(slot) => {
                self.programs[slot] = Some(entry);
                ProgramRef(slot as u32)
            }
            None => {
                self.programs.push(Some(entry));
                ProgramRef(self.programs.len() as u32 - 1)
            }
        };
        self.program_keys.insert(key, id);
        id
    }

    /// Keeps runtime programs to `keep`, least recently used out first, so a source that changes
    /// every frame cannot grow memory without limit. What the current frame has used is pinned:
    /// a recorded pass names its program by slot, so a frame using more than the limit holds them
    /// all, and the excess goes once later frames stop using it.
    fn evict_dynamic_programs(&mut self, keep: usize) {
        let mut dynamic = self.programs.iter().flatten().filter(|p| p.dynamic).count();
        while dynamic > keep {
            let oldest = self
                .programs
                .iter()
                .enumerate()
                .filter_map(|(slot, program)| Some((slot, program.as_ref()?)))
                .filter(|(_, program)| program.dynamic && program.last_used < self.frame)
                .min_by_key(|(_, program)| program.last_used)
                .map(|(slot, _)| ProgramRef(slot as u32));
            let Some(oldest) = oldest else {
                return;
            };
            self.programs[oldest.0 as usize] = None;
            self.program_keys.retain(|_, program| *program != oldest);
            self.purge_group_keys(|key| key.program == oldest);
            dynamic -= 1;
        }
    }

    fn pin_program(&mut self, program: ProgramRef) {
        if let Some(entry) = &mut self.programs[program.0 as usize] {
            entry.last_used = self.frame;
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

    /// Places `data` in the uniform bump buffer.
    pub fn uniform_bytes(&mut self, data: &[u8]) -> Res {
        let size = (data.len() as u64).max(16);
        let Some((offset, slot)) = self.uniforms.alloc(size) else {
            return Res::Uniform {
                offset: 0,
                size: 16,
            };
        };
        slot[..data.len()].copy_from_slice(data);
        self.stats.uniform_bytes += size;
        Res::Uniform {
            offset,
            size: size as u32,
        }
    }

    pub fn uniform<T: bytemuck::Pod>(&mut self, value: &T) -> Res {
        self.uniform_bytes(bytemuck::bytes_of(value))
    }

    /// Places `parts` one after another in the storage bump buffer. The range is rounded up to a
    /// power of two, so sizes moving within a class keep the offsets of everything after them,
    /// and the bind groups cached by those offsets. A range the device can't take makes
    /// [`Self::upload`] fail for this frame.
    pub fn storage_parts(&mut self, parts: &[&[u8]]) -> Res {
        let total: usize = parts.iter().map(|part| part.len()).sum();
        let size = (total as u64)
            .max(u64::from(BUMP_ALIGNMENT))
            .next_power_of_two();
        let Some((offset, slot)) = self.storage.alloc(size) else {
            return Res::Storage {
                offset: 0,
                size: BUMP_ALIGNMENT,
            };
        };
        let mut at = 0;
        for part in parts {
            slot[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        self.stats.storage_bytes += size;
        Res::Storage {
            offset,
            size: size as u32,
        }
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

    /// Holds the storage bump buffer to `max_binding` bytes a binding and `max_buffer` bytes in
    /// all, where the device allows more.
    pub fn limit_storage(&mut self, max_binding: u64, max_buffer: u64) {
        self.storage.max_binding = self.storage.max_binding.min(max_binding);
        self.storage.max_buffer = self.storage.max_buffer.min(max_buffer);
    }

    /// Writes this frame's bump buffers, rebuilding bind groups on any that had to grow. Fails,
    /// writing nothing, when the frame recorded more data than the device's buffers hold.
    pub fn upload(&mut self) -> Result<()> {
        for arena in [&self.uniforms, &self.storage] {
            if arena.overflowed {
                return Err(anyhow!(
                    "the frame's data doesn't fit the GPU: {} can bind at most {} bytes at once \
                     and hold {} bytes in all",
                    arena.label,
                    arena.max_binding,
                    arena.max_buffer
                ));
            }
        }
        let uniforms_grew = self.uniforms.upload(&self.device, &self.queue);
        let storage_grew = self.storage.upload(&self.device, &self.queue);
        if uniforms_grew || storage_grew {
            self.purge_group_keys(|key| {
                key.entries[..key.count as usize].iter().any(|(_, res)| {
                    (uniforms_grew && matches!(res, KeyRes::Uniform { .. }))
                        || (storage_grew && matches!(res, KeyRes::Storage { .. }))
                })
            });
        }
        Ok(())
    }

    /// Encodes and submits the resolved frame.
    pub fn submit(&mut self) -> Result<wgpu::SubmissionIndex> {
        let scope = ValidationScope::push(&self.device);
        self.resolve_bind_groups()?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen frame graph"),
            });
        let mut compute: Option<wgpu::ComputePass<'static>> = None;
        let mut open: Option<OpenRender> = None;
        let mut offsets = Vec::with_capacity(4);
        let mut resolved = self.resolved.iter();
        for pass in &self.passes {
            match pass {
                RecordedPass::Compute {
                    program,
                    bindings,
                    dispatch,
                } => {
                    open = None;
                    let group = *resolved.next().expect("resolved bind group");
                    let RuntimeProgram::Compute(pipeline, _) = &self.programs[program.0 as usize]
                        .as_ref()
                        .expect("program")
                        .program
                    else {
                        return Err(anyhow!("recorded a render program in a compute pass"));
                    };
                    let pass = compute.get_or_insert_with(|| {
                        encoder
                            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                                label: Some("lumen frame graph compute"),
                                timestamp_writes: None,
                            })
                            .forget_lifetime()
                    });
                    dynamic_offsets(bindings, &mut offsets);
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, self.group(group), &offsets);
                    pass.dispatch_workgroups(dispatch[0], dispatch[1], dispatch[2]);
                }
                RecordedPass::Render {
                    program,
                    bindings,
                    target,
                    load,
                    depth,
                    vertices,
                    instances,
                    viewport,
                } => {
                    compute = None;
                    let group = *resolved.next().expect("resolved bind group");
                    let RuntimeProgram::Render(pipeline, _) = &self.programs[program.0 as usize]
                        .as_ref()
                        .expect("program")
                        .program
                    else {
                        return Err(anyhow!("recorded a compute program in a render pass"));
                    };
                    let depth_tex = depth.map(|(texture, _)| texture);
                    let continues = matches!(load, TargetLoad::Load)
                        && depth.is_none_or(|(_, load)| matches!(load, TargetLoad::Load))
                        && open.as_ref().is_some_and(|open| {
                            open.target == *target
                                && open.depth == depth_tex
                                && (viewport.is_some() || !open.viewport_set)
                        });
                    if !continues {
                        drop(open.take());
                        let color = target.map(|target| wgpu::RenderPassColorAttachment {
                            view: self.view(target),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: match load {
                                    TargetLoad::Clear => {
                                        wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                                    }
                                    TargetLoad::Load => wgpu::LoadOp::Load,
                                },
                                store: wgpu::StoreOp::Store,
                            },
                        });
                        let color_attachments = match color {
                            Some(_) => std::slice::from_ref(&color),
                            None => &[],
                        };
                        open = Some(OpenRender {
                            pass: encoder
                                .begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: None,
                                    color_attachments,
                                    depth_stencil_attachment: depth.map(|(depth, load)| {
                                        wgpu::RenderPassDepthStencilAttachment {
                                            view: self.view(depth),
                                            depth_ops: Some(wgpu::Operations {
                                                load: match load {
                                                    TargetLoad::Clear => wgpu::LoadOp::Clear(1.0),
                                                    TargetLoad::Load => wgpu::LoadOp::Load,
                                                },
                                                store: wgpu::StoreOp::Store,
                                            }),
                                            stencil_ops: None,
                                        }
                                    }),
                                    occlusion_query_set: None,
                                    timestamp_writes: None,
                                    multiview_mask: None,
                                })
                                .forget_lifetime(),
                            target: *target,
                            depth: depth_tex,
                            viewport_set: false,
                        });
                    }
                    let open = open.as_mut().expect("open render pass");
                    dynamic_offsets(bindings, &mut offsets);
                    open.pass.set_pipeline(pipeline);
                    open.pass.set_bind_group(0, self.group(group), &offsets);
                    if let Some([x, y, width, height]) = *viewport {
                        open.pass.set_viewport(x, y, width, height, 0.0, 1.0);
                        open.viewport_set = true;
                    }
                    open.pass.draw(0..*vertices, 0..*instances);
                }
                RecordedPass::Copy {
                    source,
                    destination,
                    origin,
                    size,
                } => {
                    compute = None;
                    open = None;
                    encoder.copy_texture_to_texture(
                        self.texture(*source).as_image_copy(),
                        wgpu::TexelCopyTextureInfo {
                            texture: self.texture(*destination),
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: origin[0],
                                y: origin[1],
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        size.as_extent(),
                    );
                }
                RecordedPass::Clear { texture } => {
                    compute = None;
                    open = None;
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.view(*texture),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        occlusion_query_set: None,
                        timestamp_writes: None,
                        multiview_mask: None,
                    });
                }
            }
        }
        drop(compute);
        drop(open);
        let submission = self.queue.submit([encoder.finish()]);
        self.stats.passes += self.passes.len() as u64;
        scope.finish()?;
        Ok(submission)
    }

    fn view(&self, tex: TexRef) -> &wgpu::TextureView {
        &self.entries[tex.0 as usize].as_ref().expect("texture").view
    }

    fn group(&self, group: u32) -> &wgpu::BindGroup {
        &self.groups[group as usize].as_ref().expect("bind group").0
    }

    fn resolve_bind_groups(&mut self) -> Result<()> {
        let passes = std::mem::take(&mut self.passes);
        let mut resolved = std::mem::take(&mut self.resolved);
        resolved.clear();
        let mut failed = None;
        for pass in &passes {
            let (program, bindings) = match pass {
                RecordedPass::Compute {
                    program, bindings, ..
                }
                | RecordedPass::Render {
                    program, bindings, ..
                } => (*program, bindings),
                _ => continue,
            };
            match self.bind_group(program, bindings) {
                Ok(group) => resolved.push(group),
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        self.passes = passes;
        self.resolved = resolved;
        failed.map_or(Ok(()), Err)
    }

    fn bind_group(&mut self, program: ProgramRef, bindings: &PassBindings) -> Result<u32> {
        let key = bindings.key(program);
        if let Some(&group) = self.group_keys.get(&key) {
            self.stats.bind_group_hits += 1;
            self.groups[group as usize].as_mut().expect("bind group").1 = self.frame;
            return Ok(group);
        }
        self.stats.bind_group_misses += 1;
        let layout = match &self.programs[program.0 as usize]
            .as_ref()
            .ok_or_else(|| anyhow!("bind group for an evicted program"))?
            .program
        {
            RuntimeProgram::Compute(_, layout) | RuntimeProgram::Render(_, layout) => layout,
        };
        let uniforms = self.uniforms.buffer.as_ref();
        let storage = self.storage.buffer.as_ref();
        let entries: Vec<_> = bindings
            .as_slice()
            .iter()
            .map(|(binding, res)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: match *res {
                    Res::Tex(tex) => wgpu::BindingResource::TextureView(
                        &self.entries[tex.0 as usize].as_ref().expect("texture").view,
                    ),
                    Res::Uniform { size, .. } => {
                        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: uniforms.expect("uniform buffer"),
                            offset: 0,
                            size: wgpu::BufferSize::new(u64::from(size)),
                        })
                    }
                    Res::Storage { offset, size } => {
                        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: storage.expect("storage buffer"),
                            offset: u64::from(offset),
                            size: wgpu::BufferSize::new(u64::from(size)),
                        })
                    }
                    Res::Sampler(sampler) => {
                        wgpu::BindingResource::Sampler(&self.samplers[sampler.0 as usize].1)
                    }
                    Res::Buffer(buffer) => self.buffers[buffer.0 as usize]
                        .as_ref()
                        .expect("adopted buffer")
                        .as_entire_binding(),
                },
            })
            .collect();
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &entries,
        });
        let slot = match self.free_groups.pop() {
            Some(slot) => {
                self.groups[slot as usize] = Some((group, self.frame));
                slot
            }
            None => {
                self.groups.push(Some((group, self.frame)));
                self.groups.len() as u32 - 1
            }
        };
        self.group_keys.insert(key, slot);
        Ok(slot)
    }

    fn purge_groups(&mut self, matches: impl Fn(&KeyRes) -> bool) {
        self.purge_group_keys(|key| {
            key.entries[..key.count as usize]
                .iter()
                .any(|(_, res)| matches(res))
        });
    }

    fn purge_group_keys(&mut self, matches: impl Fn(&BindKey) -> bool) {
        let groups = &mut self.groups;
        let free = &mut self.free_groups;
        self.group_keys.retain(|key, group| {
            let purge = matches(key);
            if purge {
                groups[*group as usize] = None;
                free.push(*group);
            }
            !purge
        });
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

    /// Ends the frame, dropping textures and bind groups idle past their limit and, when over
    /// budget, the unused textures in rotation that were used least recently.
    pub fn end_frame(&mut self) {
        self.evict_dynamic_programs(MAX_DYNAMIC_PROGRAMS);
        let over_budget = self.pool_bytes > self.budget;
        if !over_budget && !self.frame.is_multiple_of(SWEEP_FRAMES) {
            return;
        }
        let frame = self.frame;
        let idle_before = frame.saturating_sub(IDLE_FRAMES);
        let mut removed = foldhash::HashSet::default();
        let mut surplus: Vec<(u64, PoolKey)> = Vec::new();
        let entries = &self.entries;
        for (key, bucket) in &mut self.buckets {
            while bucket.textures.len() > bucket.next {
                let tail = *bucket.textures.last().expect("tail");
                let last_used = entries[tail.0 as usize]
                    .as_ref()
                    .expect("texture")
                    .last_used;
                if last_used > idle_before {
                    surplus.push((last_used, *key));
                    break;
                }
                bucket.textures.pop();
                removed.insert(tail);
            }
        }
        for tex in removed.clone() {
            self.remove_entry(tex);
        }
        // Over budget: textures not used this frame go oldest first.
        surplus.sort_unstable_by_key(|(last_used, _)| *last_used);
        let mut surplus = surplus.into_iter();
        while self.pool_bytes > self.budget {
            let Some((_, key)) = surplus.next() else {
                break;
            };
            while self.pool_bytes > self.budget {
                let bucket = self.buckets.get_mut(&key).expect("bucket");
                if bucket.textures.len() <= bucket.next {
                    break;
                }
                let tail = bucket.textures.pop().expect("tail");
                removed.insert(tail);
                self.remove_entry(tail);
            }
        }
        self.buckets.retain(|_, bucket| !bucket.textures.is_empty());
        if !removed.is_empty() {
            self.purge_groups(|res| matches!(res, KeyRes::Tex(tex) if removed.contains(tex)));
        }
        let idle_groups = frame.saturating_sub(IDLE_FRAMES);
        let groups = &mut self.groups;
        let free = &mut self.free_groups;
        self.group_keys.retain(|_, group| {
            let keep = groups[*group as usize]
                .as_ref()
                .is_some_and(|g| g.1 > idle_groups);
            if !keep {
                groups[*group as usize] = None;
                free.push(*group);
            }
            keep
        });
    }
}

fn pool_entry_bytes(key: PoolKey) -> u64 {
    key.format.theoretical_memory_footprint(wgpu::Extent3d {
        width: key.width,
        height: key.height,
        depth_or_array_layers: 1,
    })
}

fn dynamic_offsets(bindings: &PassBindings, offsets: &mut Vec<u32>) {
    offsets.clear();
    offsets.extend(bindings.as_slice().iter().filter_map(|(_, res)| match res {
        Res::Uniform { offset, .. } => Some(*offset),
        _ => None,
    }));
}
