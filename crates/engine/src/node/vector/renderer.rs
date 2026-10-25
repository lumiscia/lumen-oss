use bytemuck::{Pod, Zeroable};

use crate::{
    gpu::{CompiledOutput, RasterHandle, RasterMetadata, compiler},
    node::{NodeId, PortRef},
};

use crate::node::source::text::{CompiledText, Text, TextState};

use super::{
    path::{CompiledPath, Path},
    shape::{CompiledPolygonShape, CompiledShape, Shape},
};

pub(crate) const SHAPE_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("paint/paint.wgsl"),
    include_str!("vector_common.wgsl"),
    include_str!("shape_renderer.wgsl")
);
pub(crate) const PATH_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("paint/paint.wgsl"),
    include_str!("vector_common.wgsl"),
    include_str!("path_renderer.wgsl")
);
fn text_shader() -> String {
    [
        include_str!("../../gpu/shaders/alpha.wgsl"),
        include_str!("paint/paint.wgsl"),
        lumen_text::TEXT_SHADER,
    ]
    .concat()
}
pub(crate) const MAX_TEXT_GLYPHS: usize = 2048;
pub(crate) const MAX_PATH_POINTS: usize = 128;

// Bits of the `flags` field in both uniform structs; keep in sync with `vector_common.wgsl`.
const STYLE_FILL_ENABLED: u32 = 1;
const STYLE_STROKE_ENABLED: u32 = 2;
const STYLE_EDGE_ANTIALIAS: u32 = 4;

pub(super) fn style_flags(fill: bool, stroke: bool, edge_antialias: bool) -> u32 {
    let mut flags = 0;
    if fill {
        flags |= STYLE_FILL_ENABLED;
    }
    if stroke {
        flags |= STYLE_STROKE_ENABLED;
    }
    if edge_antialias {
        flags |= STYLE_EDGE_ANTIALIAS;
    }
    flags
}

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
        let source = self.compile_vector_source(shape.id, port, "shape", SHAPE_SHADER, None)?;
        self.ctx.register_compiled_node(CompiledShape {
            node_id: shape.id,
            params: shape.params.clone(),
            buffer: source.params,
        });
        Ok(source.output)
    }

    pub(crate) fn compile_path(
        &mut self,
        path: &Path,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (output, params_buffer, points_buffer) =
            self.compile_path_source(path.id, port, "path")?;
        self.ctx.register_compiled_node(CompiledPath {
            node_id: path.id,
            params: path.params.clone(),
            params_buffer,
            points_buffer,
        });
        Ok(output)
    }

    /// Rasterizes a polygon shape with the path shader, as one closed contour.
    pub(crate) fn compile_polygon_shape(
        &mut self,
        shape: &Shape,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (output, params_buffer, points_buffer) =
            self.compile_path_source(shape.id, port, "shape")?;
        self.ctx.register_compiled_node(CompiledPolygonShape {
            node_id: shape.id,
            params: shape.params.clone(),
            params_buffer,
            points_buffer,
        });
        Ok(output)
    }

    fn compile_path_source(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        label: &str,
    ) -> crate::Result<(CompiledOutput, lumen_gpu::BufferId, lumen_gpu::BufferId)> {
        let points_size = (MAX_PATH_POINTS * std::mem::size_of::<PathPoint>()) as u64;
        let source =
            self.compile_vector_source(node_id, port, label, PATH_SHADER, Some(points_size))?;
        let points = source
            .points
            .expect("a source compiled with a points size has a points buffer");
        Ok((source.output, source.params, points))
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
            lumen_gpu::TextureDesc::render_target(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let atlas_texture = self.ctx.text_atlas_texture();
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
        let paint_buffer = self.ctx.builder_mut().buffer_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:paint", text.id.0)),
            lumen_gpu::BufferDesc::uniform(
                std::mem::size_of::<crate::node::vector::paint::GpuPaint>() as u64,
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
                shader: text_shader(),
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
                    lumen_gpu::BindingLayoutEntry::uniform(
                        4,
                        lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                    ),
                ]),
                targets: vec![Some(lumen_gpu::wgpu::ColorTargetState {
                    format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(lumen_gpu::wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
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
                    lumen_gpu::Binding::uniform(0, 4, paint_buffer),
                ],
                vertex_buffers: Vec::new(),
                index_buffer: None,
                draw: lumen_gpu::DrawCommand::Draw(lumen_gpu::Draw {
                    vertices: 0..6,
                    instances: 0..MAX_TEXT_GLYPHS as u32,
                }),
                scissor: None,
            });
        self.ctx.register_compiled_node(CompiledText {
            node_id: text.id,
            params: text.params.clone(),
            atlas_texture,
            globals_buffer,
            instances_buffer,
            paint_buffer,
            max_glyphs: MAX_TEXT_GLYPHS,
            size,
            state: std::sync::Arc::new(std::sync::Mutex::new(TextState::default())),
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            metadata: RasterMetadata::default(),
        }))
    }

    /// Compiles the compute pass shared by all vector rasterizers. Sources with a
    /// `points_size` also bind a read-only storage buffer of that size.
    fn compile_vector_source(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        label: &str,
        shader: &str,
        points_size: Option<u64>,
    ) -> crate::Result<VectorSource> {
        if port.port != "output" {
            return Err(self.ctx.missing_output(node_id, &port.port));
        }

        let size = lumen_gpu::Size::new(
            self.ctx.composition().render_settings.width.max(1),
            self.ctx.composition().render_settings.height.max(1),
        );
        let key = lumen_gpu::NodeKey(node_id.0);
        let texture = self.ctx.builder_mut().texture_for(
            key,
            Some(format!("{label}:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let uniforms_size = if points_size.is_some() {
            std::mem::size_of::<PathUniforms>()
        } else {
            std::mem::size_of::<ShapeUniforms>()
        };
        let params = self.ctx.builder_mut().buffer_for(
            key,
            Some(format!("{label}:{}:params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(uniforms_size as u64),
        );
        let points = points_size.map(|points_size| {
            self.ctx.builder_mut().buffer_for(
                key,
                Some(format!("{label}:{}:points", node_id.0)),
                lumen_gpu::BufferDesc::storage(points_size),
            )
        });

        let stages = lumen_gpu::wgpu::ShaderStages::COMPUTE;
        let mut layout = vec![lumen_gpu::BindingLayoutEntry::uniform(0, stages)];
        let mut bindings = vec![lumen_gpu::Binding::uniform(0, 0, params)];
        if let Some(points) = points {
            layout.push(lumen_gpu::BindingLayoutEntry::storage(1, stages, true));
            bindings.push(lumen_gpu::Binding::storage_buffer(0, 1, points));
        }
        let texture_binding = layout.len() as u32;
        layout.push(lumen_gpu::BindingLayoutEntry::storage_texture(
            texture_binding,
            stages,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            lumen_gpu::wgpu::StorageTextureAccess::WriteOnly,
        ));
        bindings.push(lumen_gpu::Binding::storage_texture(
            0,
            texture_binding,
            texture,
        ));

        let program = self.ctx.builder_mut().program_for(
            key,
            lumen_gpu::ProgramDesc::Compute(lumen_gpu::ComputeProgramDesc {
                label: Some(label.to_string()),
                shader: shader.to_string(),
                entry: "cs_main".to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(layout),
            }),
        );
        self.ctx
            .builder_mut()
            .compute_pass(lumen_gpu::ComputePassDesc {
                label: Some(format!("{label}:{}:rasterize", node_id.0)),
                owner: Some(key),
                program,
                bindings,
                dispatch: compiler::dispatch_for(size).into(),
            });
        self.ctx.builder_mut().param(
            lumen_gpu::ParamKey {
                owner: key,
                slot: 0,
            },
            lumen_gpu::ParamTarget::Buffer(params),
        );

        Ok(VectorSource {
            output: CompiledOutput::Raster(RasterHandle {
                texture,
                domain: lumen_gpu::TextureDomain::full_frame(size),
                metadata: RasterMetadata::default(),
            }),
            params,
            points,
        })
    }
}

struct VectorSource {
    output: CompiledOutput,
    params: lumen_gpu::BufferId,
    points: Option<lumen_gpu::BufferId>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct ShapeUniforms {
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
pub(crate) struct PathUniforms {
    pub(crate) fill_paint: super::paint::GpuPaint,
    pub(crate) stroke_paint: super::paint::GpuPaint,
    pub(crate) position: [f32; 2],
    pub(crate) bounds_min: [f32; 2],
    pub(crate) bounds_size: [f32; 2],
    pub(crate) stroke_width: f32,
    pub(crate) flags: u32,
    pub(crate) point_count: u32,
    pub(crate) trim_start: f32,
    pub(crate) trim_end: f32,
    pub(crate) _pad: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct PathPoint {
    pub(crate) position: [f32; 2],
    pub(crate) contour_start: u32,
    pub(crate) flags: u32,
    pub(crate) offset: f32,
    pub(crate) _pad: f32,
}
