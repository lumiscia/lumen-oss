use crate::{BufferId, SamplerId, TextureId};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextureAccess {
    Sampled,
    Storage,
    RenderTarget,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BufferAccess {
    Uniform,
    Storage,
    Vertex,
    Index,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingResource {
    Texture {
        id: TextureId,
        access: TextureAccess,
    },
    Buffer {
        id: BufferId,
        access: BufferAccess,
    },
    Sampler(SamplerId),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Binding {
    pub group: u32,
    pub binding: u32,
    pub resource: BindingResource,
}

impl Binding {
    pub const fn sampled_texture(group: u32, binding: u32, id: TextureId) -> Self {
        Self {
            group,
            binding,
            resource: BindingResource::Texture {
                id,
                access: TextureAccess::Sampled,
            },
        }
    }

    pub const fn storage_texture(group: u32, binding: u32, id: TextureId) -> Self {
        Self {
            group,
            binding,
            resource: BindingResource::Texture {
                id,
                access: TextureAccess::Storage,
            },
        }
    }

    pub const fn uniform(group: u32, binding: u32, id: BufferId) -> Self {
        Self {
            group,
            binding,
            resource: BindingResource::Buffer {
                id,
                access: BufferAccess::Uniform,
            },
        }
    }

    pub const fn storage_buffer(group: u32, binding: u32, id: BufferId) -> Self {
        Self {
            group,
            binding,
            resource: BindingResource::Buffer {
                id,
                access: BufferAccess::Storage,
            },
        }
    }

    pub const fn sampler(group: u32, binding: u32, id: SamplerId) -> Self {
        Self {
            group,
            binding,
            resource: BindingResource::Sampler(id),
        }
    }
}
