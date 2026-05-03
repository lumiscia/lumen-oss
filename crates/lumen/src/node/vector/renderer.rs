use bytemuck::{Pod, Zeroable};

use crate::{
    gpu::{CompiledOutput, FrameBinding, RasterHandle, RasterMetadata, compiler},
    node::{NodeId, PortRef},
};

use super::{shape::Shape, text::Text};

pub(crate) const SHAPE_SHADER: &str = include_str!("shape_renderer.wgsl");
pub(crate) const TEXT_SHADER: &str = include_str!("text_renderer.wgsl");
pub(crate) const MAX_TEXT_CHARS: usize = 4096;

pub(crate) struct VectorRenderer<'a, 'b> {
    ctx: &'a mut crate::gpu::CompileContext<'b>,
}

impl<'a, 'b> VectorRenderer<'a, 'b> {
    pub(crate) fn new(ctx: &'a mut crate::gpu::CompileContext<'b>) -> Self {
        Self { ctx }
    }

    pub(crate) fn compile_shape(
        &mut self,
        shape: &Shape,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (texture, params, size) = self.compile_vector_source(
            shape.id,
            port,
            "shape",
            SHAPE_SHADER,
            std::mem::size_of::<ShapeParams>() as u64,
        )?;
        self.ctx.push_frame_binding(FrameBinding::Shape {
            node_id: shape.id,
            geometry_kind: shape.geometry_kind.clone(),
            width: shape.width.clone(),
            height: shape.height.clone(),
            border_radius: shape.border_radius.clone(),
            position: shape.position.clone(),
            fill_enabled: shape.fill_enabled.clone(),
            fill_color: shape.fill_color.clone(),
            stroke_enabled: shape.stroke_enabled.clone(),
            stroke_color: shape.stroke_color.clone(),
            stroke_width: shape.stroke_width.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        }))
    }

    pub(crate) fn compile_text(
        &mut self,
        text: &Text,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(self.ctx.missing_output(text.id, &port.port));
        }

        let size = lumen_gpu::Size::new(
            self.ctx.composition().render_settings.width.max(1),
            self.ctx.composition().render_settings.height.max(1),
        );
        let texture = self.ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:output", text.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:params", text.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<TextParams>() as u64),
        );
        let text_buffer = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:chars", text.id.0)),
            lumen_gpu::BufferDesc::storage((MAX_TEXT_CHARS * std::mem::size_of::<u32>()) as u64),
        );
        let program = self.ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(text.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("text".to_string()),
                shader: TEXT_SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::uniform(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage(
                        2,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        true,
                    ),
                ]),
            }),
        );
        self.ctx
            .builder_mut()
            .compute_pass(lumen_gpu::ComputePassDesc {
                label: Some(format!("text:{}:rasterize", text.id.0)),
                owner: Some(lumen_gpu::NodeKey(text.id.0)),
                program,
                bindings: vec![
                    lumen_gpu::Binding::uniform(0, 0, params),
                    lumen_gpu::Binding::storage_texture(0, 1, texture),
                    lumen_gpu::Binding::storage_buffer(0, 2, text_buffer),
                ],
                dispatch: compiler::dispatch_for(size),
            });
        self.ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(text.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        self.ctx.push_frame_binding(FrameBinding::Text {
            node_id: text.id,
            content: text.content.clone(),
            font_size: text.font_size.clone(),
            max_width: text.max_width.clone(),
            position: text.position.clone(),
            color: text.color.clone(),
            alignment_horizontal: text.alignment_horizontal.clone(),
            alignment_vertical: text.alignment_vertical.clone(),
            buffer: params,
            text_buffer,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        }))
    }

    fn compile_vector_source(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        label: &str,
        shader: &str,
        params_size: u64,
    ) -> crate::Result<(lumen_gpu::TextureId, lumen_gpu::BufferId, lumen_gpu::Size)> {
        if port.port != "output" {
            return Err(self.ctx.missing_output(node_id, &port.port));
        }

        let size = lumen_gpu::Size::new(
            self.ctx.composition().render_settings.width.max(1),
            self.ctx.composition().render_settings.height.max(1),
        );
        let texture = self.ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("{label}:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(node_id.0),
            Some(format!("{label}:{}:params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(params_size),
        );
        let program = self.ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(node_id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some(label.to_string()),
                shader: shader.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::uniform(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                        lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
                    ),
                ]),
            }),
        );
        self.ctx
            .builder_mut()
            .compute_pass(lumen_gpu::ComputePassDesc {
                label: Some(format!("{label}:{}:rasterize", node_id.0)),
                owner: Some(lumen_gpu::NodeKey(node_id.0)),
                program,
                bindings: vec![
                    lumen_gpu::Binding::uniform(0, 0, params),
                    lumen_gpu::Binding::storage_texture(0, 1, texture),
                ],
                dispatch: compiler::dispatch_for(size),
            });
        self.ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(node_id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        Ok((texture, params, size))
    }
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ShapeParams {
    pub(crate) fill_color: [f32; 4],
    pub(crate) stroke_color: [f32; 4],
    pub(crate) position: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) border_radius: f32,
    pub(crate) stroke_width: f32,
    pub(crate) geometry_kind: u32,
    pub(crate) flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct TextParams {
    pub(crate) color: [f32; 4],
    pub(crate) position: [f32; 2],
    pub(crate) font_size: f32,
    pub(crate) max_width: f32,
    pub(crate) content_len: u32,
    pub(crate) line_count: u32,
    pub(crate) alignment_horizontal: u32,
    pub(crate) alignment_vertical: u32,
    pub(crate) _pad: [u32; 4],
}
