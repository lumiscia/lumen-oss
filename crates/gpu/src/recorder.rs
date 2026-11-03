//! Frame-graph recording: passes are recorded from what a frame shows, against long-lived pooled
//! textures, per-frame bump buffers and cached programs and bind groups, without a compiled plan.
//!
//! Resources stay valid across frames in queue order: a pooled texture or bump range rewritten for
//! a later frame is only written by later submissions, so reuse needs no fence. Evicted textures
//! are dropped, and wgpu holds them until submissions using them complete.

use std::{hash::Hash, sync::Arc};

use anyhow::{Result, anyhow};

use crate::{ExternalTexture, Size, SubmittedExternalTexture, renderer::ValidationScope};

/// Bytes of pooled textures kept before unused ones are dropped regardless of age.
pub const DEFAULT_POOL_BUDGET_BYTES: u64 = 1 << 30;
/// Frames a pooled texture or bind group stays cached without being used.
const IDLE_FRAMES: u64 = 600;
/// Frames between sweeps for idle resources.
const SWEEP_FRAMES: u64 = 60;
/// Programs built from runtime shader source that stay cached.
const MAX_DYNAMIC_PROGRAMS: usize = 64;
/// Most bindings a pass can have: a shader's 16 inputs and 4 others.
pub const MAX_PASS_BINDINGS: usize = 20;
const BUMP_ALIGNMENT: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct TexRef(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ProgramRef(u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct SamplerRef(u32);

#[derive(Debug, Clone, Copy)]
pub enum LayoutKind {
    Texture,
    StorageTexture(wgpu::TextureFormat),
    /// Bound with a dynamic offset into the uniform bump buffer.
    Uniform,
    Storage,
    Sampler,
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum KeyRes {
    Tex(TexRef),
    Uniform { size: u32 },
    Storage { offset: u32, size: u32 },
    Sampler(SamplerRef),
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
        target: TexRef,
        load: TargetLoad,
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
    next: usize,
}

struct Arena {
    label: &'static str,
    usage: wgpu::BufferUsages,
    cpu: Vec<u8>,
    buffer: Option<wgpu::Buffer>,
    capacity: u64,
}

impl Arena {
    fn new(label: &'static str, usage: wgpu::BufferUsages) -> Self {
        Self {
            label,
            usage,
            cpu: Vec::new(),
            buffer: None,
            capacity: 0,
        }
    }

    fn alloc(&mut self, size: u32) -> (u32, &mut [u8]) {
        let offset = (self.cpu.len() as u32).next_multiple_of(BUMP_ALIGNMENT);
        self.cpu.resize((offset + size) as usize, 0);
        (offset, &mut self.cpu[offset as usize..])
    }

    /// Writes this frame's data, growing the buffer first when needed. Returns whether it grew,
    /// which invalidates bind groups on the old one.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let needed = (self.cpu.len() as u64).max(u64::from(BUMP_ALIGNMENT));
        let grew = needed > self.capacity;
        if grew {
            self.capacity = needed.next_power_of_two().max(1 << 16);
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
}

/// How a program's pipeline draws, for render programs.
#[derive(Debug, Clone, Copy)]
pub struct RenderTargetSpec {
    pub format: wgpu::TextureFormat,
    pub blend: wgpu::BlendState,
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
}

pub struct Recorder {
    device: wgpu::Device,
    queue: wgpu::Queue,
    frame: u64,
    entries: Vec<Option<PoolEntry>>,
    free_slots: Vec<u32>,
    buckets: foldhash::HashMap<PoolKey, PoolBucket>,
    pool_bytes: u64,
    budget: u64,
    programs: Vec<Option<ProgramEntry>>,
    program_keys: foldhash::HashMap<ProgramKey, ProgramRef>,
    samplers: Vec<(u64, wgpu::Sampler)>,
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
        Self {
            device,
            queue,
            frame: 0,
            entries: Vec::new(),
            free_slots: Vec::new(),
            buckets: foldhash::HashMap::default(),
            pool_bytes: 0,
            budget: DEFAULT_POOL_BUDGET_BYTES,
            programs: Vec::new(),
            program_keys: foldhash::HashMap::default(),
            samplers: Vec::new(),
            uniforms: Arena::new("lumen frame graph uniforms", wgpu::BufferUsages::UNIFORM),
            storage: Arena::new("lumen frame graph storage", wgpu::BufferUsages::STORAGE),
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

    /// Bytes of textures this recorder holds, in rotation or retained, and of its bump buffers.
    pub fn held_bytes(&self) -> u64 {
        self.pool_bytes + self.uniforms.capacity + self.storage.capacity
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
        self.uniforms.cpu.clear();
        self.storage.cpu.clear();
        for bucket in self.buckets.values_mut() {
            bucket.next = 0;
        }
    }

    /// A texture of this size, format and usage that this frame has not been handed yet. Its
    /// contents are undefined: every pass writing it must write all it later reads.
    pub fn acquire(
        &mut self,
        size: Size,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> TexRef {
        let key = PoolKey {
            width: size.width.max(1),
            height: size.height.max(1),
            format,
            usage,
        };
        let bucket = self.buckets.entry(key).or_default();
        if let Some(&tex) = bucket.textures.get(bucket.next) {
            bucket.next += 1;
            self.entries[tex.0 as usize]
                .as_mut()
                .expect("pooled texture")
                .last_used = self.frame;
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
            format,
            usage,
            view_formats: &[],
        });
        self.stats.textures_created += 1;
        let tex = self.insert(Arc::new(texture), key, false);
        let bucket = self.buckets.get_mut(&key).expect("bucket");
        bucket.textures.push(tex);
        bucket.next = bucket.textures.len();
        tex
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
        let bytes = key.format.theoretical_memory_footprint(wgpu::Extent3d {
            width: key.width,
            height: key.height,
            depth_or_array_layers: 1,
        });
        self.pool_bytes += bytes;
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

    /// Forgets a texture from `adopt`, along with the bind groups using it.
    pub fn forget(&mut self, tex: TexRef) {
        self.remove_entry(tex);
        self.purge_groups(|res| matches!(res, KeyRes::Tex(other) if *other == tex));
    }

    fn remove_entry(&mut self, tex: TexRef) {
        if let Some(entry) = self.entries[tex.0 as usize].take() {
            self.pool_bytes -= entry.bytes;
            self.free_slots.push(tex.0);
            if !entry.external {
                entry.texture.destroy();
            }
        }
    }

    /// Takes the texture out of rotation until as many `release` calls follow, so its contents
    /// survive later frames.
    pub fn retain(&mut self, tex: TexRef) {
        let entry = self.entries[tex.0 as usize].as_mut().expect("texture");
        entry.retained += 1;
        let (first, external, key) = (entry.retained == 1, entry.external, entry.key);
        if first
            && !external
            && let Some(bucket) = self.buckets.get_mut(&key)
        {
            if let Some(index) = bucket.textures.iter().position(|other| *other == tex) {
                bucket.textures.remove(index);
                if index < bucket.next {
                    bucket.next -= 1;
                }
            }
        }
    }

    pub fn release(&mut self, tex: TexRef) {
        let entry = self.entries[tex.0 as usize].as_mut().expect("texture");
        entry.retained -= 1;
        entry.last_used = self.frame;
        if entry.retained == 0 && !entry.external {
            let key = entry.key;
            self.buckets.entry(key).or_default().textures.push(tex);
        }
    }

    pub fn is_retained(&self, tex: TexRef) -> bool {
        self.entries[tex.0 as usize]
            .as_ref()
            .is_some_and(|entry| entry.retained > 0)
    }

    pub fn texture(&self, tex: TexRef) -> &Arc<wgpu::Texture> {
        &self.entries[tex.0 as usize]
            .as_ref()
            .expect("texture")
            .texture
    }

    pub fn texture_bytes(&self, tex: TexRef) -> u64 {
        self.entries[tex.0 as usize]
            .as_ref()
            .expect("texture")
            .bytes
    }

    pub fn usage(&self, tex: TexRef) -> wgpu::TextureUsages {
        self.entries[tex.0 as usize]
            .as_ref()
            .expect("texture")
            .key
            .usage
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
        Ok(self.insert_program(key, RuntimeProgram::Compute(pipeline, group), dynamic))
    }

    /// A render program built from a `'static` shader, cached by that shader's identity and
    /// `variant`, which names its blend and target format.
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
            format: Some(target.format),
        };
        if let Some(&program) = self.program_keys.get(&key) {
            return Ok(program);
        }
        let scope = ValidationScope::push(&self.device);
        let module = self
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(shader.into()),
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
            .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &module,
                    entry_point: Some(vertex),
                    compilation_options: Default::default(),
                    buffers: &[],
                },
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                fragment: Some(wgpu::FragmentState {
                    module: &module,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: target.format,
                        blend: Some(target.blend),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                multiview_mask: None,
                cache: None,
            });
        scope.finish()?;
        Ok(self.insert_program(key, RuntimeProgram::Render(pipeline, group), false))
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
            self.evict_dynamic_programs();
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

    /// Keeps runtime programs to `MAX_DYNAMIC_PROGRAMS`, least recently used out first, so a
    /// source that changes every frame cannot grow memory without limit.
    fn evict_dynamic_programs(&mut self) {
        let dynamic = self.programs.iter().flatten().filter(|p| p.dynamic).count();
        if dynamic < MAX_DYNAMIC_PROGRAMS {
            return;
        }
        let oldest = self
            .programs
            .iter()
            .enumerate()
            .filter_map(|(slot, program)| Some((slot, program.as_ref()?)))
            .filter(|(_, program)| program.dynamic)
            .min_by_key(|(_, program)| program.last_used)
            .map(|(slot, _)| ProgramRef(slot as u32));
        let Some(oldest) = oldest else {
            return;
        };
        self.programs[oldest.0 as usize] = None;
        self.program_keys.retain(|_, program| *program != oldest);
        self.purge_group_keys(|key| key.program == oldest);
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
        let size = (data.len() as u32).max(16);
        let (offset, slot) = self.uniforms.alloc(size);
        slot[..data.len()].copy_from_slice(data);
        self.stats.uniform_bytes += u64::from(size);
        Res::Uniform { offset, size }
    }

    pub fn uniform<T: bytemuck::Pod>(&mut self, value: &T) -> Res {
        self.uniform_bytes(bytemuck::bytes_of(value))
    }

    /// Places `parts` one after another in the storage bump buffer. The range is rounded up to a
    /// power of two, so sizes moving within a class keep the offsets of everything after them,
    /// and the bind groups cached by those offsets.
    pub fn storage_parts(&mut self, parts: &[&[u8]]) -> Res {
        let total: usize = parts.iter().map(|part| part.len()).sum();
        let size = (total as u32).max(BUMP_ALIGNMENT).next_power_of_two();
        let (offset, slot) = self.storage.alloc(size);
        let mut at = 0;
        for part in parts {
            slot[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        self.stats.storage_bytes += u64::from(size);
        Res::Storage { offset, size }
    }

    pub fn compute(
        &mut self,
        program: ProgramRef,
        bindings: &[(u32, Res)],
        dispatch: crate::Dispatch,
    ) {
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
        self.passes.push(RecordedPass::Render {
            program,
            bindings: PassBindings::new(bindings),
            target,
            load,
            vertices,
            instances,
            viewport,
        });
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

    /// Writes this frame's bump buffers, rebuilding bind groups on any that had to grow.
    pub fn upload(&mut self) {
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
    }

    /// Encodes and submits the recorded frame.
    pub fn submit(&mut self) -> Result<wgpu::SubmissionIndex> {
        self.submit_copying(None)
    }

    /// Submits the recorded frame, then copies `output` to `external`, which holds it from then on.
    pub fn submit_to_external(
        &mut self,
        output: TexRef,
        external: ExternalTexture,
    ) -> Result<SubmittedExternalTexture> {
        let texture = external.texture_arc();
        let submission = self.submit_copying(Some((output, &texture)))?;
        Ok(SubmittedExternalTexture::new(submission, texture))
    }

    fn submit_copying(
        &mut self,
        copy_to: Option<(TexRef, &wgpu::Texture)>,
    ) -> Result<wgpu::SubmissionIndex> {
        let scope = ValidationScope::push(&self.device);
        self.resolve_bind_groups()?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen frame graph"),
            });
        let mut compute: Option<wgpu::ComputePass<'static>> = None;
        let mut offsets = Vec::with_capacity(4);
        let mut resolved = self.resolved.iter();
        for pass in &self.passes {
            match pass {
                RecordedPass::Compute {
                    program,
                    bindings,
                    dispatch,
                } => {
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
                    let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.view(*target),
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
                        })],
                        depth_stencil_attachment: None,
                        occlusion_query_set: None,
                        timestamp_writes: None,
                        multiview_mask: None,
                    });
                    dynamic_offsets(bindings, &mut offsets);
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, self.group(group), &offsets);
                    if let Some([x, y, width, height]) = *viewport {
                        pass.set_viewport(x, y, width, height, 0.0, 1.0);
                    }
                    pass.draw(0..*vertices, 0..*instances);
                }
                RecordedPass::Copy {
                    source,
                    destination,
                    origin,
                    size,
                } => {
                    compute = None;
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
        if let Some((output, external)) = copy_to {
            let source = self.texture(output);
            encoder.copy_texture_to_texture(
                source.as_image_copy(),
                external.as_image_copy(),
                source.size(),
            );
        }
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

    /// Ends the frame, dropping textures and bind groups idle past their limit and, when over
    /// budget, the unused textures in rotation that were used least recently.
    pub fn end_frame(&mut self) {
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

fn dynamic_offsets(bindings: &PassBindings, offsets: &mut Vec<u32>) {
    offsets.clear();
    offsets.extend(bindings.as_slice().iter().filter_map(|(_, res)| match res {
        Res::Uniform { offset, .. } => Some(*offset),
        _ => None,
    }));
}
