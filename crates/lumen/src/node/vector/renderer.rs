use bytemuck::{Pod, Zeroable};

use crate::{
    gpu::{CompiledOutput, FrameBinding, RasterHandle, RasterMetadata, compiler},
    node::{NodeId, PortRef},
};

use crate::node::source::text::Text;

use super::{path::Path, shape::Shape};

pub(crate) const SHAPE_SHADER: &str = include_str!("shape_renderer.wgsl");
pub(crate) const PATH_SHADER: &str = include_str!("path_renderer.wgsl");
pub(crate) const TEXT_ATLAS_SIZE: lumen_gpu::Size = lumen_gpu::Size {
    width: 1024,
    height: 1024,
};
pub(crate) const MAX_TEXT_GLYPHS: usize = 2048;
pub(crate) const MAX_TEXT_MSDF_SEGMENTS: usize = 32768;
pub(crate) const MAX_TEXT_MSDF_PIXELS: u32 = 262_144;
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
                dispatch: compiler::dispatch_for(size),
            });
        self.ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: lumen_gpu::NodeKey(path.id.0),
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );
        self.ctx.push_frame_binding(FrameBinding::Path {
            node_id: path.id,
            data: path.data.clone(),
            position: path.position.clone(),
            fill_enabled: path.fill_enabled.clone(),
            fill_color: path.fill_color.clone(),
            stroke_enabled: path.stroke_enabled.clone(),
            stroke_color: path.stroke_color.clone(),
            stroke_width: path.stroke_width.clone(),
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
            lumen_gpu::TextureDesc::storage(
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
        let msdf_globals_buffer =
            self.ctx.builder_mut().buffer_for(
                lumen_gpu::NodeKey(text.id.0),
                Some(format!("text:{}:msdf-globals", text.id.0)),
                lumen_gpu::BufferDesc::uniform(
                    std::mem::size_of::<lumen_text::GpuMsdfGlobals>() as u64
                ),
            );
        let msdf_jobs_buffer = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:msdf-jobs", text.id.0)),
            lumen_gpu::BufferDesc::storage(
                (MAX_TEXT_GLYPHS * std::mem::size_of::<lumen_text::GpuMsdfJob>()) as u64,
            ),
        );
        let msdf_segments_buffer = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:msdf-segments", text.id.0)),
            lumen_gpu::BufferDesc::storage(
                (MAX_TEXT_MSDF_SEGMENTS * std::mem::size_of::<lumen_text::GpuMsdfSegment>()) as u64,
            ),
        );
        let msdf_pixel_jobs_buffer = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:msdf-pixel-jobs", text.id.0)),
            lumen_gpu::BufferDesc::storage(
                (MAX_TEXT_MSDF_PIXELS as usize * std::mem::size_of::<u32>()) as u64,
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
                shader: lumen_text::MSDF_TEXT_SHADER.to_string(),
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
        let msdf_program = self.ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(text.id.0),
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some("text-msdf-generate".to_string()),
                shader: lumen_text::MSDF_GENERATOR_SHADER.to_string(),
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
                    lumen_gpu::BindingLayoutEntry::storage(
                        3,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        true,
                    ),
                    lumen_gpu::BindingLayoutEntry::storage(
                        4,
                        lumen_gpu::wgpu::ShaderStages::COMPUTE,
                        true,
                    ),
                ]),
            }),
        );
        self.ctx
            .builder_mut()
            .compute_pass(lumen_gpu::ComputePassDesc {
                label: Some(format!("text:{}:msdf-generate", text.id.0)),
                owner: Some(lumen_gpu::NodeKey(text.id.0)),
                program: msdf_program,
                bindings: vec![
                    lumen_gpu::Binding::uniform(0, 0, msdf_globals_buffer),
                    lumen_gpu::Binding::storage_texture(0, 1, atlas_texture),
                    lumen_gpu::Binding::storage_buffer(0, 2, msdf_jobs_buffer),
                    lumen_gpu::Binding::storage_buffer(0, 3, msdf_segments_buffer),
                    lumen_gpu::Binding::storage_buffer(0, 4, msdf_pixel_jobs_buffer),
                ],
                dispatch: lumen_gpu::Dispatch {
                    x: MAX_TEXT_MSDF_PIXELS.div_ceil(64),
                    y: 1,
                    z: 1,
                },
            });
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
        self.ctx.push_frame_binding(FrameBinding::Text {
            node_id: text.id,
            content: text.content.clone(),
            font_family: text.font_family.clone(),
            font_size: text.font_size.clone(),
            font_weight: text.font_weight.clone(),
            font_style: text.font_style.clone(),
            max_width: text.max_width.clone(),
            position: text.position.clone(),
            color: text.color.clone(),
            alignment_horizontal: text.alignment_horizontal.clone(),
            alignment_vertical: text.alignment_vertical.clone(),
            atlas_texture,
            globals_buffer,
            instances_buffer,
            msdf_globals_buffer,
            msdf_jobs_buffer,
            msdf_segments_buffer,
            msdf_pixel_jobs_buffer,
            atlas_size: TEXT_ATLAS_SIZE,
            max_glyphs: MAX_TEXT_GLYPHS,
            max_msdf_segments: MAX_TEXT_MSDF_SEGMENTS,
            max_msdf_pixels: MAX_TEXT_MSDF_PIXELS,
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
pub(crate) struct PathParams {
    pub(crate) fill_color: [f32; 4],
    pub(crate) stroke_color: [f32; 4],
    pub(crate) position: [f32; 2],
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
