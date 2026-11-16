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

    /// A texture compute passes write. Where the format allows it on every device, it is a
    /// render attachment too, which lets a browser zero a new one on the GPU instead of
    /// uploading zeros to it.
    pub fn storage(size: Size, format: wgpu::TextureFormat) -> Self {
        let mut usage = wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC;
        let guaranteed = format.guaranteed_format_features(wgpu::Features::empty());
        if guaranteed
            .allowed_usages
            .contains(wgpu::TextureUsages::RENDER_ATTACHMENT)
        {
            usage |= wgpu::TextureUsages::RENDER_ATTACHMENT;
        }
        Self {
            domain: TextureDomain::full_frame(size),
            format,
            usage,
        }
    }
}
