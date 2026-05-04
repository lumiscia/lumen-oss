use crate::{NodeKey, ProgramId};

#[derive(Debug, Clone)]
pub struct BindingLayoutEntry {
    pub binding: u32,
    pub visibility: wgpu::ShaderStages,
    pub ty: wgpu::BindingType,
}

impl BindingLayoutEntry {
    pub fn texture(binding: u32, visibility: wgpu::ShaderStages) -> Self {
        Self {
            binding,
            visibility,
            ty: wgpu::BindingType::Texture {
                sample_type: wgpu::TextureSampleType::Float { filterable: true },
                view_dimension: wgpu::TextureViewDimension::D2,
                multisampled: false,
            },
        }
    }

    pub fn storage_texture(
        binding: u32,
        visibility: wgpu::ShaderStages,
        format: wgpu::TextureFormat,
        access: wgpu::StorageTextureAccess,
    ) -> Self {
        Self {
            binding,
            visibility,
            ty: wgpu::BindingType::StorageTexture {
                access,
                format,
                view_dimension: wgpu::TextureViewDimension::D2,
            },
        }
    }

    pub fn sampler(binding: u32, visibility: wgpu::ShaderStages) -> Self {
        Self {
            binding,
            visibility,
            ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
        }
    }

    pub fn uniform(binding: u32, visibility: wgpu::ShaderStages) -> Self {
        Self {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        }
    }

    pub fn storage(binding: u32, visibility: wgpu::ShaderStages, read_only: bool) -> Self {
        Self {
            binding,
            visibility,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Storage { read_only },
                has_dynamic_offset: false,
                min_binding_size: None,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct BindGroupLayoutSpec {
    pub label: Option<String>,
    pub entries: Vec<BindingLayoutEntry>,
}

impl BindGroupLayoutSpec {
    pub fn single(entries: Vec<BindingLayoutEntry>) -> Vec<Self> {
        vec![Self {
            label: None,
            entries,
        }]
    }
}

#[derive(Debug, Clone)]
pub struct RenderProgramDesc {
    pub label: Option<String>,
    pub shader: String,
    pub vertex_entry: String,
    pub fragment_entry: String,
    pub bind_groups: Vec<BindGroupLayoutSpec>,
    pub targets: Vec<Option<wgpu::ColorTargetState>>,
    pub vertex_buffers: Vec<wgpu::VertexBufferLayout<'static>>,
    pub primitive: wgpu::PrimitiveState,
}

#[derive(Debug, Clone)]
pub struct ComputeProgramDesc {
    pub label: Option<String>,
    pub shader: String,
    pub entry: String,
    pub bind_groups: Vec<BindGroupLayoutSpec>,
}

#[derive(Debug, Clone)]
pub enum ProgramDesc {
    Render(RenderProgramDesc),
    Compute(ComputeProgramDesc),
}

#[derive(Debug, Clone)]
pub struct Program {
    pub id: ProgramId,
    pub owner: Option<NodeKey>,
    pub desc: ProgramDesc,
}
