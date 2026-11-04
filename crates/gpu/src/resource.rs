use crate::{Size, TextureDomain};

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
