use std::ops::Range;

use crate::{Binding, BufferId, NodeKey, PassId, ProgramId, Size, TextureId};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum LoadOp {
    Load,
    Clear(wgpu::Color),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RenderTargetRef {
    pub texture: TextureId,
    pub load: LoadOp,
    pub store: wgpu::StoreOp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ScissorRect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Draw {
    pub vertices: Range<u32>,
    pub instances: Range<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DrawIndexed {
    pub indices: Range<u32>,
    pub base_vertex: i32,
    pub instances: Range<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DrawCommand {
    Draw(Draw),
    DrawIndexed(DrawIndexed),
}

#[derive(Debug, Clone)]
pub struct RenderPassDesc {
    pub label: Option<String>,
    pub owner: Option<NodeKey>,
    pub program: ProgramId,
    pub targets: Vec<RenderTargetRef>,
    pub bindings: Vec<Binding>,
    pub vertex_buffers: Vec<BufferId>,
    pub index_buffer: Option<(BufferId, wgpu::IndexFormat)>,
    pub draw: DrawCommand,
    pub scissor: Option<ScissorRect>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Dispatch {
    pub x: u32,
    pub y: u32,
    pub z: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ComputeDispatch {
    Direct(Dispatch),
    Indirect { buffer: BufferId, offset: u64 },
}

impl From<Dispatch> for ComputeDispatch {
    fn from(dispatch: Dispatch) -> Self {
        Self::Direct(dispatch)
    }
}

#[derive(Debug, Clone)]
pub struct ComputePassDesc {
    pub label: Option<String>,
    pub owner: Option<NodeKey>,
    pub program: ProgramId,
    pub bindings: Vec<Binding>,
    pub dispatch: ComputeDispatch,
}

#[derive(Debug, Clone)]
pub struct CopyTextureDesc {
    pub label: Option<String>,
    pub source: TextureId,
    pub destination: TextureId,
    pub origin: wgpu::Origin3d,
    pub size: Size,
}

#[derive(Debug, Clone)]
pub enum PassDesc {
    Render(RenderPassDesc),
    Compute(ComputePassDesc),
    CopyTexture(CopyTextureDesc),
}

#[derive(Debug, Clone)]
pub struct Pass {
    pub id: PassId,
    pub desc: PassDesc,
}
