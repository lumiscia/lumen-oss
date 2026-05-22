use bytemuck::{Pod, Zeroable};

use crate::{
    gpu::{CompiledOutput, RasterHandle, RasterMetadata, compiler},
    node::{NodeId, PortRef},
};

use crate::node::source::text::{Text, TextFrameBinding};

use super::{
    path::{Path, PathFrameBinding},
    shape::{Shape, ShapeFrameBinding},
};

pub(crate) const SHAPE_SHADER: &str = include_str!("shape_renderer.wgsl");
pub(crate) const PATH_SHADER: &str = include_str!("path_renderer.wgsl");
pub(crate) const TEXT_ATLAS_SIZE: lumen_gpu::Size = lumen_gpu::Size {
    width: 2048,
    height: 2048,
};
pub(crate) const MAX_TEXT_GLYPHS: usize = 2048;
pub(crate) const MAX_PATH_POINTS: usize = 128;

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
        self.ctx.push_frame_binding(ShapeFrameBinding {
            node_id: shape.id,
            geometry_kind: shape.params.geometry_kind.clone(),
            width: shape.params.width.clone(),
            height: shape.params.height.clone(),
            border_radius: shape.params.border_radius.clone(),
            position: shape.params.position.clone(),
            fill_enabled: shape.params.fill_enabled.clone(),
            fill_color: shape.params.fill_color.clone(),
            fill_paint: shape.params.fill_paint.clone(),
            stroke_enabled: shape.params.stroke_enabled.clone(),
            stroke_color: shape.params.stroke_color.clone(),
            stroke_paint: shape.params.stroke_paint.clone(),
            stroke_width: shape.params.stroke_width.clone(),
            buffer: params,
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        }))
    }

    pub(crate) fn compile_path(
        &mut self,
        path: &Path,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        if port.port != "output" {
            return Err(self.ctx.missing_output(path.id, &port.port));
        }

        let size = lumen_gpu::Size::new(
            self.ctx.composition().render_settings.width.max(1),
            self.ctx.composition().render_settings.height.max(1),
        );
        let texture = self.ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(path.id.0),
            Some(format!("path:{}:output", path.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(path.id.0),
            Some(format!("path:{}:params", path.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<PathParams>() as u64),
        );
        let points = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(path.id.0),
            Some(format!("path:{}:points", path.id.0)),
            lumen_gpu::BufferDesc::storage(
                (MAX_PATH_POINTS * std::mem::size_of::<PathPoint>()) as u64,
            ),
        );
        let program = self.ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(path.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("path".to_string()),
                shader: PATH_SHADER.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::uniform(
                        0,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage(
                        1,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        true,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage_texture(
                        2,
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
                label: Some(format!("path:{}:rasterize", path.id.0)),
                owner: Some(lumen_gpu::NodeKey(path.id.0)),
                program,
                bindings: vec![
                    lumen_gpu::Binding::uniform(0, 0, params),
                    lumen_gpu::Binding::storage_buffer(0, 1, points),
                    lumen_gpu::Binding::storage_texture(0, 2, texture),
                ],
                dispatch: compiler::dispatch_for(size).into(),
            });
        self.ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(path.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        self.ctx.push_frame_binding(PathFrameBinding {
            node_id: path.id,
            data: path.params.data.clone(),
            position: path.params.position.clone(),
            fill_enabled: path.params.fill_enabled.clone(),
            fill_color: path.params.fill_color.clone(),
            fill_paint: path.params.fill_paint.clone(),
            stroke_enabled: path.params.stroke_enabled.clone(),
            stroke_color: path.params.stroke_color.clone(),
            stroke_paint: path.params.stroke_paint.clone(),
            stroke_width: path.params.stroke_width.clone(),
            params_buffer: params,
            points_buffer: points,
            max_points: MAX_PATH_POINTS,
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
        crate::node::source::text::clear_text_cache_for(text.id);

        let size = lumen_gpu::Size::new(
            self.ctx.composition().render_settings.width.max(1),
            self.ctx.composition().render_settings.height.max(1),
        );
        let texture = self.ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:output", text.id.0)),
            lumen_gpu::TextureDesc::render_target(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let atlas_texture = self.ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:atlas", text.id.0)),
            lumen_gpu::TextureDesc::sampled(
                TEXT_ATLAS_SIZE,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            ),
        );
        let globals_buffer =
            self.ctx.builder_mut().buffer_for(
                lumen_gpu::NodeKey(text.id.0),
                Some(format!("text:{}:globals", text.id.0)),
                lumen_gpu::BufferDesc::uniform(
                    std::mem::size_of::<lumen_text::GpuTextGlobals>() as u64
                ),
            );
        let instances_buffer = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:instances", text.id.0)),
            lumen_gpu::BufferDesc::storage(
                (MAX_TEXT_GLYPHS * std::mem::size_of::<lumen_text::GpuGlyphInstance>()) as u64,
            ),
        );
        let atlas_sampler = self.ctx.builder_mut().sampler(
            Some(format!("text:{}:atlas-sampler", text.id.0)),
            lumen_gpu::wgpu::SamplerDescriptor {
                label: Some("lumen text atlas sampler"),
                address_mode_u: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                address_mode_v: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                address_mode_w: lumen_gpu::wgpu::AddressMode::ClampToEdge,
                mag_filter: lumen_gpu::wgpu::FilterMode::Linear,
                min_filter: lumen_gpu::wgpu::FilterMode::Linear,
                mipmap_filter: lumen_gpu::wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            },
        );
        let program = self.ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(text.id.0),
            lumen_gpu::ProgramDesc::Render(lumen_gpu::RenderProgramDesc {
                label: Some("text".to_string()),
                shader: lumen_text::ALPHA_TEXT_SHADER.to_string(),
                vertex_entry: "vs_main".to_string(),
                fragment_entry: "fs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(vec![
                    lumen_gpu::BindingLayoutEntry::uniform(
                        0,
                        lumen_gpu::wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ),
                    lumen_gpu::BindingLayoutEntry::texture(
                        1,
                        lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                    ),
                    lumen_gpu::BindingLayoutEntry::sampler(
                        2,
                        lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage(
                        3,
                        lumen_gpu::wgpu::ShaderStages::VERTEX,
                        true,
                    ),
                ]),
                targets: vec![Some(lumen_gpu::wgpu::ColorTargetState {
                    format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(lumen_gpu::wgpu::BlendState::ALPHA_BLENDING),
                    write_mask: lumen_gpu::wgpu::ColorWrites::ALL,
                })],
                vertex_buffers: Vec::new(),
                primitive: lumen_gpu::wgpu::PrimitiveState::default(),
            }),
        );
        self.ctx
            .builder_mut()
            .render_pass(lumen_gpu::RenderPassDesc {
                label: Some(format!("text:{}:render", text.id.0)),
                owner: Some(lumen_gpu::NodeKey(text.id.0)),
                program,
                targets: vec![lumen_gpu::RenderTargetRef {
                    texture,
                    load: lumen_gpu::LoadOp::Clear(lumen_gpu::wgpu::Color::TRANSPARENT),
                    store: lumen_gpu::wgpu::StoreOp::Store,
                }],
                bindings: vec![
                    lumen_gpu::Binding::uniform(0, 0, globals_buffer),
                    lumen_gpu::Binding::sampled_texture(0, 1, atlas_texture),
                    lumen_gpu::Binding::sampler(0, 2, atlas_sampler),
                    lumen_gpu::Binding::storage_buffer(0, 3, instances_buffer),
                ],
                vertex_buffers: Vec::new(),
                index_buffer: None,
                draw: lumen_gpu::DrawCommand::Draw(lumen_gpu::Draw {
                    vertices: 0..6,
                    instances: 0..MAX_TEXT_GLYPHS as u32,
                }),
                scissor: None,
            });
        self.ctx.push_frame_binding(TextFrameBinding {
            node_id: text.id,
            content: text.params.content.clone(),
            font_family: text.params.font_family.clone(),
            font_size: text.params.font_size.clone(),
            font_weight: text.params.font_weight.clone(),
            font_style: text.params.font_style.clone(),
            max_width: text.params.max_width.clone(),
            position: text.params.position.clone(),
            color: text.params.color.clone(),
            alignment_horizontal: text.params.alignment_horizontal.clone(),
            alignment_vertical: text.params.alignment_vertical.clone(),
            atlas_texture,
            globals_buffer,
            instances_buffer,
            atlas_size: TEXT_ATLAS_SIZE,
            max_glyphs: MAX_TEXT_GLYPHS,
            size,
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
                dispatch: compiler::dispatch_for(size).into(),
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
    pub(crate) fill_paint: super::paint::GpuPaint,
    pub(crate) stroke_paint: super::paint::GpuPaint,
    pub(crate) position: [f32; 2],
    pub(crate) size: [f32; 2],
    pub(crate) border_radius: f32,
    pub(crate) stroke_width: f32,
    pub(crate) geometry_kind: u32,
    pub(crate) flags: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PathParams {
    pub(crate) fill_paint: super::paint::GpuPaint,
    pub(crate) stroke_paint: super::paint::GpuPaint,
    pub(crate) position: [f32; 2],
    pub(crate) bounds_min: [f32; 2],
    pub(crate) bounds_size: [f32; 2],
    pub(crate) stroke_width: f32,
    pub(crate) flags: u32,
    pub(crate) point_count: u32,
    pub(crate) _pad: [u32; 3],
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PathPoint {
    pub(crate) position: [f32; 2],
}
