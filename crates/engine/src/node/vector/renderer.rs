use bytemuck::{Pod, Zeroable};

use crate::{
    gpu::{CompiledOutput, RasterHandle, RasterMetadata, compiler},
    node::{Deferred, NodeId, NodeParams, PortRef},
};

use crate::node::source::text::{CompiledText, GlyphParams, Text, TextState};

use super::{
    drawable::{DrawableTransform, GpuDrawableMotion, GpuDrawableTransform},
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
/// Times the canvas, along each axis, that the layer of animated, motion-blurred text spans,
/// so that text may sweep the canvas by that much less one canvas over its shutter.
/// Plans are cached and textures are fixed in them, so the layer cannot follow the sweep of
/// each frame. Text that sweeps further is drawn at a lower scale to fit.
const TEXT_LAYER_SCALE: u32 = 2;
/// Largest texture dimension of the devices that engine hosts request, whose limits are
/// wgpu's defaults.
const MAX_TEXTURE_DIMENSION: u32 = 8192;
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
        let bounds = self.ctx.source_bounds(
            shape.id,
            compiler::SourceMotion::of(
                super::shape::shape_is_animated(&shape.params),
                &shape.params,
            ),
            |expr| {
                super::shape::shape_frame(&shape.params, shape.id, expr)
                    .map(|uniforms| Some(uniforms.screen_bounds))
            },
        );
        let domain = self.ctx.source_domain(bounds);
        let source = self.compile_vector_source(
            shape.id,
            port,
            "shape",
            SHAPE_SHADER,
            std::mem::size_of::<ShapeUniforms>(),
            &[],
            domain,
        )?;
        self.ctx.register_compiled_node(CompiledShape {
            node_id: shape.id,
            params: shape.params.clone(),
            buffer: source.params,
            origin: domain_origin(domain),
        });
        Ok(source.output)
    }

    pub(crate) fn compile_path(
        &mut self,
        path: &Path,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let point_capacity = super::path::point_capacity(&path.params.data, &path.params.scale);
        // Opacity only hides what is drawn, so it is left out of the bounds.
        let opaque = super::path::PathParamsDelegate {
            opacity: Deferred::value(1.0),
            ..path.params.clone()
        };
        let (output, buffers) = self.compile_path_source(
            path.id,
            port,
            "path",
            point_capacity,
            compiler::SourceMotion::of(super::path::path_is_animated(&path.params), &path.params),
            |expr, point_capacity, band_capacity| {
                super::path::path_frame(&opaque, path.id, expr, point_capacity, band_capacity)
            },
        )?;
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
        // Opacity only hides what is drawn, so it is left out of the bounds.
        let opaque = super::shape::ShapeParamsDelegate {
            opacity: Deferred::value(1.0),
            ..shape.params.clone()
        };
        let (output, buffers) = self.compile_path_source(
            shape.id,
            port,
            "shape",
            point_capacity,
            compiler::SourceMotion::of(
                super::shape::shape_is_animated(&shape.params),
                &shape.params,
            ),
            |expr, point_capacity, band_capacity| {
                super::shape::polygon_frame(&opaque, shape.id, expr, point_capacity, band_capacity)
            },
        )?;
        self.ctx.register_compiled_node(CompiledPolygonShape {
            node_id: shape.id,
            params: shape.params.clone(),
            buffers,
        });
        Ok(output)
    }

    /// Compiles a source rasterized by the path shader, sized to the screen bounds that
    /// `frame_at` gives over the frames it may draw.
    #[allow(
        clippy::too_many_arguments,
        reason = "the path and polygon sources share every input but how a frame is evaluated"
    )]
    fn compile_path_source(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        label: &str,
        point_capacity: usize,
        motion: compiler::SourceMotion,
        frame_at: impl Fn(
            &crate::expr::ExpressionContext<'_>,
            usize,
            usize,
        ) -> crate::Result<super::path::PathFrame>,
    ) -> crate::Result<(CompiledOutput, PathBuffers)> {
        let point_capacity = point_capacity.max(1);
        let band_capacity = super::path_bands::band_capacity(point_capacity);
        let bounds = self.ctx.source_bounds(node_id, motion, |expr| {
            frame_at(expr, point_capacity, band_capacity)
                .map(|frame| Some(frame.uniforms.screen_bounds))
        });
        let domain = self.ctx.source_domain(bounds);
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
            domain,
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
                origin: domain_origin(domain),
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

        let size = self.ctx.canvas_size();
        // Text that may move during the shutter is drawn still to a layer first, which is
        // then smeared over the shutter's taps into the output. Text with motion blur
        // constantly off, or whose transform is constant, is drawn to the output directly.
        let params = &text.params;
        let blurred = !matches!(
            params.motion_blur,
            Deferred::Value(angle) if crate::node::motion::shutter_offset(angle).is_none()
        ) && !(matches!(params.position, Deferred::Value(_))
            && matches!(params.scale, Deferred::Value(_))
            && matches!(params.rotation, Deferred::Value(_))
            && matches!(params.anchor, Deferred::Value(_)));
        let bounds = if blurred {
            None
        } else {
            self.text_bounds(text)
        };
        let domain = self.ctx.source_domain(bounds);
        let texture = self.ctx.builder_mut().texture_for(
            lumen_gpu::NodeKey(text.id.0),
            Some(format!("text:{}:output", text.id.0)),
            lumen_gpu::TextureDesc::render_target(
                domain.storage_size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            )
            .with_domain(domain),
        );
        // Glyphs are placed in canvas pixels, so a bounded output sees the canvas through a
        // viewport offset to its origin.
        let viewport = (!domain.is_full_frame()).then(|| lumen_gpu::Viewport {
            x: -domain.data_rect.x as f32,
            y: -domain.data_rect.y as f32,
            width: size.width as f32,
            height: size.height as f32,
        });
        let layer_size = if blurred {
            let fit =
                |canvas: u32| (canvas * TEXT_LAYER_SCALE).min(MAX_TEXTURE_DIMENSION.max(canvas));
            lumen_gpu::Size::new(fit(size.width), fit(size.height))
        } else {
            size
        };
        let layer = blurred.then(|| {
            self.ctx.builder_mut().texture_for(
                lumen_gpu::NodeKey(text.id.0),
                Some(format!("text:{}:layer", text.id.0)),
                lumen_gpu::TextureDesc::render_target(
                    layer_size,
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                ),
            )
        });
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
        let text_program = |label: &str, vertex: &str, fragment: &str, blend, reads_layer| {
            let mut entries = vec![
                lumen_gpu::BindingLayoutEntry::uniform(
                    0,
                    lumen_gpu::wgpu::ShaderStages::VERTEX_FRAGMENT,
                ),
                lumen_gpu::BindingLayoutEntry::texture(1, lumen_gpu::wgpu::ShaderStages::FRAGMENT),
                lumen_gpu::BindingLayoutEntry::sampler(2, lumen_gpu::wgpu::ShaderStages::FRAGMENT),
                lumen_gpu::BindingLayoutEntry::storage(
                    3,
                    lumen_gpu::wgpu::ShaderStages::VERTEX_FRAGMENT,
                    true,
                ),
                lumen_gpu::BindingLayoutEntry::uniform(
                    4,
                    lumen_gpu::wgpu::ShaderStages::VERTEX_FRAGMENT,
                ),
            ];
            if reads_layer {
                entries.push(lumen_gpu::BindingLayoutEntry::texture(
                    5,
                    lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                ));
            }
            lumen_gpu::ProgramDesc::Render(lumen_gpu::RenderProgramDesc {
                label: Some(label.to_string()),
                shader: text_shader(),
                vertex_entry: vertex.to_string(),
                fragment_entry: fragment.to_string(),
                bind_groups: lumen_gpu::BindGroupLayoutSpec::single(entries),
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
        let text_pass = |label: &str, program, target, load, bindings, draw, viewport| {
            lumen_gpu::RenderPassDesc {
                label: Some(format!("text:{}:{label}", text.id.0)),
                owner: Some(lumen_gpu::NodeKey(text.id.0)),
                program,
                targets: vec![lumen_gpu::RenderTargetRef {
                    texture: target,
                    load,
                    store: lumen_gpu::wgpu::StoreOp::Store,
                }],
                bindings,
                vertex_buffers: Vec::new(),
                index_buffer: None,
                draw: lumen_gpu::DrawCommand::Draw(draw),
                scissor: None,
                viewport,
            }
        };
        let fullscreen = lumen_gpu::Draw {
            vertices: 0..3,
            instances: 0..1,
        };
        let program = self.ctx.builder_mut().program_for(
            lumen_gpu::NodeKey(text.id.0),
            text_program(
                "text",
                "vs_main",
                "fs_main",
                lumen_gpu::wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
                false,
            ),
        );
        self.ctx.builder_mut().render_pass(text_pass(
            "render",
            program,
            layer.unwrap_or(texture),
            lumen_gpu::LoadOp::Clear(lumen_gpu::wgpu::Color::TRANSPARENT),
            bindings.clone(),
            lumen_gpu::Draw {
                vertices: 0..6,
                instances: 0..MAX_TEXT_GLYPHS as u32,
            },
            viewport,
        ));
        if let Some(layer) = layer {
            let program = self.ctx.builder_mut().program_for(
                lumen_gpu::NodeKey(text.id.0),
                text_program(
                    "text smear",
                    "vs_fullscreen",
                    "fs_smear",
                    lumen_gpu::wgpu::BlendState::REPLACE,
                    true,
                ),
            );
            let mut bindings = bindings.clone();
            bindings.push(lumen_gpu::Binding::sampled_texture(0, 5, layer));
            self.ctx.builder_mut().render_pass(text_pass(
                "smear",
                program,
                texture,
                lumen_gpu::LoadOp::Clear(lumen_gpu::wgpu::Color::TRANSPARENT),
                bindings,
                fullscreen.clone(),
                None,
            ));
        }
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
                    "vs_fullscreen",
                    "fs_opacity",
                    lumen_gpu::wgpu::BlendState {
                        color: multiply,
                        alpha: multiply,
                    },
                    false,
                ),
            );
            self.ctx.builder_mut().render_pass(text_pass(
                "opacity",
                program,
                texture,
                lumen_gpu::LoadOp::Load,
                bindings,
                fullscreen,
                None,
            ));
        }
        let mut params = text.params.clone();
        let glyph = GlyphParams::take(&mut params);
        self.ctx.register_compiled_node(CompiledText {
            node_id: text.id,
            params,
            glyph,
            atlas_texture,
            globals_buffer,
            instances_buffer,
            paint_buffer,
            max_glyphs: MAX_TEXT_GLYPHS,
            size,
            layer_size,
            state: std::sync::Arc::new(std::sync::Mutex::new(TextState::default())),
        });

        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain,
            metadata: RasterMetadata::default(),
        }))
    }

    /// The canvas pixels unblurred text may draw to, or `None` when it should fill the
    /// canvas: the ink of its glyphs, as the font that will draw it measures them.
    ///
    /// Only text whose layout is constant and whose paint is a solid colour is bounded: its
    /// paint is evaluated at the pixel it is drawn to, which a viewport offset moves. Text in a
    /// font that the media store has not resolved is not bounded either, since the font drawn
    /// later may be wider than the one measured here.
    fn text_bounds(&self, text: &Text) -> Option<lumen_gpu::RectI> {
        let params = &text.params;
        if !matches!(params.color, super::paint::PaintDelegate::SolidColor(..))
            || params.content.is_expr()
            || params.font_family.is_expr()
            || params.font_size.is_expr()
            || params.font_weight.is_expr()
            || params.max_width.is_expr()
            || params.letter_spacing.is_expr()
            || params.line_height.is_expr()
        {
            return None;
        }
        let expr = self.ctx.expr_context(text.id, "params");
        let evaluated = params
            .eval(&crate::node::NodeParamEvalContext {
                node_id: text.id,
                expr: &expr,
            })
            .ok()?;
        let mut request = crate::node::source::text::layout_request(
            evaluated.content,
            evaluated.font_family,
            evaluated.font_size as f32,
            evaluated.font_weight,
            evaluated.font_style,
            evaluated.max_width as f32,
            evaluated.alignment_horizontal,
        );
        crate::node::source::text::apply_line_spacing(
            &mut request,
            evaluated.letter_spacing,
            evaluated.line_height,
        );
        let family = request.font_family.as_str();
        let resolved = self
            .ctx
            .media()
            .is_some_and(|media| media.font_resolver_id(family).is_some());
        if !resolved && !family.is_empty() && family != lumen_text::DEFAULT_FONT_FAMILY {
            return None;
        }
        let (shaped, ink) = {
            let mut text_system = crate::node::source::text::text_system().ok()?;
            crate::node::source::text::load_font_family(
                &mut text_system,
                self.ctx.media(),
                &request.font_family,
            )
            .ok()?;
            let shaped = text_system.shape(&request);
            let ink = text_system.ink_bounds(&shaped)?;
            (shaped, ink)
        };
        let line_height = shaped.measurement.height;
        let alignment = evaluated.alignment_vertical;
        let animated = params.position.is_expr()
            || params.scale.is_expr()
            || params.rotation.is_expr()
            || params.anchor.is_expr();
        let motion = compiler::SourceMotion::of(animated, params);
        self.ctx.source_bounds(text.id, motion, |expr| {
            let position = params.position.eval(text.id, "position", expr)?;
            let top = match alignment {
                crate::node::source::text::TextAlignmentVertical::Top => position.1 as f32,
                crate::node::source::text::TextAlignmentVertical::Middle => {
                    position.1 as f32 - line_height * 0.5
                }
                crate::node::source::text::TextAlignmentVertical::Bottom => {
                    position.1 as f32 - line_height
                }
            };
            let min = [position.0 as f32 + ink[0], top + ink[1]];
            let max = [position.0 as f32 + ink[2], top + ink[3]];
            let transform = DrawableTransform {
                position,
                scale: params.scale.eval(text.id, "scale", expr)?,
                rotation: params.rotation.eval(text.id, "rotation", expr)?,
                anchor: params.anchor.eval(text.id, "anchor", expr)?,
                opacity: 1.0,
            };
            let [x0, y0, x1, y1] = transform.to_gpu().canvas_bounds(min, max);
            Ok(Some([x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0]))
        })
    }

    /// Compiles the compute pass shared by all vector rasterizers, over the texels of
    /// `domain`. Each of `storage` binds a read-only storage buffer of that name and size,
    /// after the uniforms.
    #[allow(
        clippy::too_many_arguments,
        reason = "every vector rasterizer passes the same flat set of compile inputs"
    )]
    fn compile_vector_source(
        &mut self,
        node_id: NodeId,
        port: &PortRef,
        label: &str,
        shader: &str,
        uniforms_size: usize,
        storage: &[(&str, u64)],
        domain: lumen_gpu::TextureDomain,
    ) -> crate::Result<VectorSource> {
        if port.port != "output" {
            return Err(self.ctx.missing_output(node_id, &port.port));
        }

        let size = domain.storage_size;
        let key = lumen_gpu::NodeKey(node_id.0);
        let texture = self.ctx.builder_mut().texture_for(
            key,
            Some(format!("{label}:{}:output", node_id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm)
                .with_domain(domain),
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
                domain,
                metadata: RasterMetadata::default(),
            }),
            params,
            storage,
        })
    }
}

/// The canvas pixel of a domain's first texel.
pub(crate) fn domain_origin(domain: lumen_gpu::TextureDomain) -> [i32; 2] {
    [domain.data_rect.x, domain.data_rect.y]
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
    /// Canvas pixel of the output's first texel.
    pub(crate) origin: [i32; 2],
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
    /// Canvas pixel of the output's first texel.
    pub(crate) origin: [i32; 2],
    pub(crate) _origin_pad: [i32; 2],
    pub(crate) motion: GpuDrawableMotion,
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
    /// Canvas pixel of the output's first texel.
    pub(crate) origin: [i32; 2],
    pub(crate) _origin_pad: [i32; 2],
    pub(crate) motion: GpuDrawableMotion,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct TextStyleUniforms {
    pub(crate) fill_paint: super::paint::GpuPaint,
    pub(crate) transform: GpuDrawableTransform,
    pub(crate) motion: GpuDrawableMotion,
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
