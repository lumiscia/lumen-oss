use crate::{BufferId, NodeKey, SamplerId, Size, TextureDomain, TextureId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDesc {
    pub domain: TextureDomain,
    pub format: wgpu::TextureFormat,
    pub usage: wgpu::TextureUsages,
}

impl TextureDesc {
    pub fn render_target(size: Size, format: wgpu::TextureFormat) -> Self {
        Self {
            domain: TextureDomain::full_frame(size),
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
        }
    }

    pub fn sampled(size: Size, format: wgpu::TextureFormat) -> Self {
        Self {
            domain: TextureDomain::full_frame(size),
            format,
            usage: wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
        }
    }

    pub fn storage(size: Size, format: wgpu::TextureFormat) -> Self {
        Self {
            domain: TextureDomain::full_frame(size),
            format,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BufferDesc {
    pub size: u64,
    pub usage: wgpu::BufferUsages,
}

impl BufferDesc {
    pub fn uniform(size: u64) -> Self {
        Self {
            size,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        }
    }

    pub fn storage(size: u64) -> Self {
        Self {
            size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        }
    }

    pub fn vertex(size: u64) -> Self {
        Self {
            size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        }
    }

    pub fn index(size: u64) -> Self {
        Self {
            size,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TextureResource {
    pub id: TextureId,
    pub label: Option<String>,
    pub desc: TextureDesc,
    pub owner: Option<NodeKey>,
}

#[derive(Debug, Clone)]
pub struct BufferResource {
    pub id: BufferId,
    pub label: Option<String>,
    pub desc: BufferDesc,
    pub owner: Option<NodeKey>,
}

#[derive(Debug, Clone)]
pub struct SamplerResource {
    pub id: SamplerId,
    pub label: Option<String>,
    pub desc: wgpu::SamplerDescriptor<'static>,
}
