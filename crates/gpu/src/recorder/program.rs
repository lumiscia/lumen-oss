use super::{FrameError, MAX_DYNAMIC_PROGRAMS, ProgramRef, Recorder, Result};
use crate::renderer::ValidationScope;

#[derive(Debug, Clone, Copy)]
pub enum LayoutKind {
    Texture,
    /// A filterable float texture of three dimensions.
    Texture3d,
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
    /// A storage buffer the shader may write, for compute passes.
    StorageReadWrite,
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

pub(super) enum RuntimeProgram {
    Compute(wgpu::ComputePipeline, wgpu::BindGroupLayout),
    Render(wgpu::RenderPipeline, wgpu::BindGroupLayout),
}

pub(super) struct ProgramEntry {
    pub(super) program: RuntimeProgram,
    last_used: u64,
    dynamic: bool,
}

/// What a program is cached by: a static shader's identity, which needs no hashing per pass, or
/// the hash of a runtime source, plus entry points and whatever else varies its pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct ProgramKey {
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

impl Recorder {
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
            return Err(FrameError::Misuse(format!(
                "render program `{label}` binds a storage texture"
            )));
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
                    LayoutKind::Texture3d => wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
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
                    LayoutKind::StorageReadWrite => wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Storage { read_only: false },
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
    pub(super) fn evict_dynamic_programs(&mut self, keep: usize) {
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

    pub(super) fn pin_program(&mut self, program: ProgramRef) {
        if let Some(entry) = &mut self.programs[program.0 as usize] {
            entry.last_used = self.frame;
        }
    }
}
