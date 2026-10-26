use bytemuck::{Pod, Zeroable};

use crate::{
    gpu::{CompiledOutput, RasterHandle, RasterMetadata, compiler},
    node::{Deferred, NodeId, PortRef},
};

use crate::node::source::text::{CompiledText, Text, TextState};

use super::{
    drawable::GpuDrawableTransform,
    path::{CompiledPath, Path},
    shape::{CompiledPolygonShape, CompiledShape, Shape},
};

pub(crate) const SHAPE_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("paint/paint.wgsl"),
    include_str!("drawable.wgsl"),
    include_str!("vector_common.wgsl"),
    include_str!("shape_renderer.wgsl")
);
pub(crate) const PATH_SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("paint/paint.wgsl"),
    include_str!("drawable.wgsl"),
    include_str!("vector_common.wgsl"),
    include_str!("path_renderer.wgsl")
);
fn text_shader() -> String {
    [
        include_str!("../../gpu/shaders/alpha.wgsl"),
        include_str!("paint/paint.wgsl"),
        super::drawable::DRAWABLE_SHADER,
        lumen_text::TEXT_SHADER,
    ]
    .concat()
}
pub(crate) const MAX_TEXT_GLYPHS: usize = 2048;
/// Most points a path or polygon may flatten to. Points buffers are sized to the
/// flattened point count when the geometry is constant, and to this when animated.
pub(crate) const MAX_PATH_POINTS: usize = 16384;

// Bits of the `flags` field in both uniform structs; keep in sync with `vector_common.wgsl`.
pub(super) const STYLE_FILL_ENABLED: u32 = 1;
pub(super) const STYLE_STROKE_ENABLED: u32 = 2;
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
        let source = self.compile_vector_source(
            shape.id,
            port,
            "shape",
            SHAPE_SHADER,
            std::mem::size_of::<ShapeUniforms>(),
            &[],
        )?;
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
        let point_capacity = super::path::point_capacity(&path.params.data, &path.params.scale);
        let (output, buffers) = self.compile_path_source(path.id, port, "path", point_capacity)?;
        self.ctx.register_compiled_node(CompiledPath {
            node_id: path.id,
            params: path.params.clone(),
            buffers,
        });
        Ok(output)
    }

    /// Rasterizes a polygon shape with the path shader, as one closed contour.
    pub(crate) fn compile_polygon_shape(
        &mut self,
        shape: &Shape,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let point_capacity = super::shape::polygon_point_capacity(&shape.params.polygon_points);
        let (output, buffers) =
            self.compile_path_source(shape.id, port, "shape", point_capacity)?;
        self.ctx.register_compiled_node(CompiledPolygonShape {
            node_id: shape.id,
            params: shape.params.clone(),
            buffers,
        });
        Ok(output)
    }

    fn compile_path_source(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        label: &str,
        point_capacity: usize,
    ) -> crate::Result<(CompiledOutput, PathBuffers)> {
        let point_capacity = point_capacity.max(1);
        let band_capacity = super::path_bands::band_capacity(point_capacity);
        let source = self.compile_vector_source(
            node_id,
            port,
            label,
            PATH_SHADER,
            std::mem::size_of::<PathUniforms>(),
            &[
                (
                    "points",
                    (point_capacity * std::mem::size_of::<PathPoint>()) as u64,
                ),
                ("bands", (band_capacity * std::mem::size_of::<u32>()) as u64),
            ],
        )?;
        let [points, bands] = source.storage[..] else {
            unreachable!("a path source has points and bands buffers");
        };
        Ok((
            source.output,
            PathBuffers {
                params: source.params,
                points,
                bands,
                point_capacity,
                band_capacity,
            },
        ))
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
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<TextStyleUniforms>() as u64),
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
        let text_program = |label: &str, entry: &str, blend| {
            lumen_gpu::ProgramDesc::Render(lumen_gpu::RenderProgramDesc {
                label: Some(label.to_string()),
                shader: text_shader(),
                vertex_entry: format!("vs_{entry}"),
                fragment_entry: format!("fs_{entry}"),
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
                        lumen_gpu::wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ),
                ]),
                targets: vec![Some(lumen_gpu::wgpu::ColorTargetState {
                    format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                    blend: Some(blend),
                    write_mask: lumen_gpu::wgpu::ColorWrites::ALL,
                })],
                vertex_buffers: Vec::new(),
                primitive: lumen_gpu::wgpu::PrimitiveState::default(),
            })
        };
        let bindings = vec![
            lumen_gpu::Binding::uniform(0, 0, globals_buffer),
            lumen_gpu::Binding::sampled_texture(0, 1, atlas_texture),
            lumen_gpu::Binding::sampler(0, 2, atlas_sampler),
            lumen_gpu::Binding::storage_buffer(0, 3, instances_buffer),
            lumen_gpu::Binding::uniform(0, 4, paint_buffer),
        ];
        let text_pass = |label: &str, program, load, draw| lumen_gpu::RenderPassDesc {
            label: Some(format!("text:{}:{label}", text.id.0)),
            owner: Some(lumen_gpu::NodeKey(text.id.0)),
            program,
            targets: vec![lumen_gpu::RenderTargetRef {
                texture,
                load,
                store: lumen_gpu::wgpu::StoreOp::Store,
            }],
            bindings: bindings.clone(),
            vertex_buffers: Vec::new(),
            index_buffer: None,
            draw: lumen_gpu::DrawCommand::Draw(draw),
            scissor: None,
        };
        let program = self.ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(text.id.0),
            text_program(
                "text",
                "main",
                lumen_gpu::wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
            ),
        );
        self.ctx.builder_mut().render_pass(text_pass(
            "render",
            program,
            lumen_gpu::LoadOp::Clear(lumen_gpu::wgpu::Color::TRANSPARENT),
            lumen_gpu::Draw {
                vertices: 0..6,
                instances: 0..MAX_TEXT_GLYPHS as u32,
            },
        ));
        // Fading the composited layer keeps overlapping glyphs from doubling up. Fully
        // transparent text draws no glyphs, so constant full opacity needs no pass.
        if !matches!(text.params.opacity, Deferred::Value(opacity) if opacity >= 1.0) {
            let multiply = lumen_gpu::wgpu::BlendComponent {
                src_factor: lumen_gpu::wgpu::BlendFactor::Zero,
                dst_factor: lumen_gpu::wgpu::BlendFactor::SrcAlpha,
                operation: lumen_gpu::wgpu::BlendOperation::Add,
            };
            let program = self.ctx.builder_mut().program_for(
                lumen_gpu::NodeKey(text.id.0),
                text_program(
                    "text opacity",
                    "opacity",
                    lumen_gpu::wgpu::BlendState {
                        color: multiply,
                        alpha: multiply,
                    },
                ),
            );
            self.ctx.builder_mut().render_pass(text_pass(
                "opacity",
                program,
                lumen_gpu::LoadOp::Load,
                lumen_gpu::Draw {
                    vertices: 0..3,
                    instances: 0..1,
                },
            ));
        }
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

    /// Compiles the compute pass shared by all vector rasterizers. Each of `storage`
    /// binds a read-only storage buffer of that name and size, after the uniforms.
    fn compile_vector_source(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        label: &str,
        shader: &str,
        uniforms_size: usize,
        storage: &[(&str, u64)],
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
        let params = self.ctx.builder_mut().buffer_for(
            key,
            Some(format!("{label}:{}:params", node_id.0)),
            lumen_gpu::BufferDesc::uniform(uniforms_size as u64),
        );
        let storage: Vec<_> = storage
            .iter()
            .map(|(name, size)| {
                self.ctx.builder_mut().buffer_for(
                    key,
                    Some(format!("{label}:{}:{name}", node_id.0)),
                    lumen_gpu::BufferDesc::storage(*size),
                )
            })
            .collect();

        let stages = lumen_gpu::wgpu::ShaderStages::COMPUTE;
        let mut layout = vec![lumen_gpu::BindingLayoutEntry::uniform(0, stages)];
        let mut bindings = vec![lumen_gpu::Binding::uniform(0, 0, params)];
        for (index, buffer) in (1..).zip(&storage) {
            layout.push(lumen_gpu::BindingLayoutEntry::storage(index, stages, true));
            bindings.push(lumen_gpu::Binding::storage_buffer(0, index, *buffer));
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
            storage,
        })
    }
}

struct VectorSource {
    output: CompiledOutput,
    params: lumen_gpu::BufferId,
    storage: Vec<lumen_gpu::BufferId>,
}

/// Buffers of a source rasterized by the path shader.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PathBuffers {
    pub(crate) params: lumen_gpu::BufferId,
    pub(crate) points: lumen_gpu::BufferId,
    pub(crate) bands: lumen_gpu::BufferId,
    pub(crate) point_capacity: usize,
    pub(crate) band_capacity: usize,
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
    pub(crate) transform: GpuDrawableTransform,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` outside which nothing is drawn.
    pub(crate) screen_bounds: [f32; 4],
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
    pub(crate) transform: GpuDrawableTransform,
    /// Canvas-space `[min_x, min_y, max_x, max_y]` outside which nothing is drawn.
    pub(crate) screen_bounds: [f32; 4],
    /// Untransformed `y`, relative to `position`, where the first band starts.
    pub(crate) band_origin: f32,
    /// Bands per untransformed pixel.
    pub(crate) band_scale: f32,
    pub(crate) band_count: u32,
    /// Untransformed distance beyond which a segment cannot cover a pixel.
    pub(crate) reach: f32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct TextStyleUniforms {
    pub(crate) fill_paint: super::paint::GpuPaint,
    pub(crate) transform: GpuDrawableTransform,
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
