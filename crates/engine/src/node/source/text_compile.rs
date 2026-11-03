use bytemuck::{Pod, Zeroable};

use crate::error::RenderError;
use crate::gpu::{CompileContext, CompiledOutput, RasterHandle, RasterMetadata, compiler};
use crate::node::motion::shutter_offset;
use crate::node::vector::{
    drawable::{DRAWABLE_SHADER, DrawableTransform, GpuDrawableMotion, GpuDrawableTransform},
    paint::{GpuPaint, PaintDelegate},
};
use crate::node::{
    Deferred, DelegateEvalContext, NodeParamEvalContext, NodeParams, Override, PortRef,
};

use super::text::{
    CompiledText, DecorationParams, GlyphParams, NodeDecorations, Text, TextAlignmentVertical,
    TextDecorations, TextLayoutStyle, TextParamsDelegate, TextState, decorate, decoration_paints,
    load_font_families, span_strokes, stroked, text_system,
};
use super::text_content::{TextContent, TextContentDelegate, TextLineDelegate, TextSpanDelegate};

fn text_shader() -> String {
    [
        include_str!("../../gpu/shaders/alpha.wgsl"),
        include_str!("../vector/paint/paint.wgsl"),
        DRAWABLE_SHADER,
        lumen_text::TEXT_SHADER,
    ]
    .concat()
}
/// The most glyph, stroke and decoration instances one text node can draw.
const MAX_TEXT_GLYPHS: usize = 2048;

/// A text node's layout and decorations when none of them are animated.
struct ConstantText {
    shaped: lumen_text::ShapedText,
    alignment: TextAlignmentVertical,
    /// The rect instances of its decorations, placed at the origin.
    decorations: Vec<lumen_text::GpuGlyphInstance>,
    /// How far its strokes reach past the glyph edges.
    stroke_reach: f32,
}
/// Times the canvas, along each axis, that the layer of animated, motion-blurred text spans,
/// so that text may sweep the canvas by that much less one canvas over its shutter.
/// Plans are cached and textures are fixed in them, so the layer cannot follow the sweep of
/// each frame. Text that sweeps further is drawn at a lower scale to fit.
const TEXT_LAYER_SCALE: u32 = 2;
/// Largest texture dimension of the devices that engine hosts request, whose limits are
/// wgpu's defaults.
const MAX_TEXTURE_DIMENSION: u32 = 8192;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct TextStyleUniforms {
    pub(crate) transform: GpuDrawableTransform,
    pub(crate) motion: GpuDrawableMotion,
}

/// A paint of a text, and the box `[x, y, width, height]` its relative coordinates map to
/// `0..1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub(crate) struct GpuTextPaint {
    pub(crate) paint: GpuPaint,
    pub(crate) bounds: [f32; 4],
}

pub(super) fn compile_text(
    ctx: &mut CompileContext<'_>,
    text: &Text,
    port: &PortRef,
) -> crate::Result<CompiledOutput> {
    if port.port != "output" {
        return Err(ctx.missing_output(text.id, &port.port));
    }

    let size = ctx.canvas_size();
    // Text that may move during the shutter is drawn still to a layer first, which is
    // then smeared over the shutter's taps into the output. Text with motion blur
    // constantly off, or whose transform is constant, is drawn to the output directly.
    let params = &text.params;
    let blurred = !matches!(
        params.motion_blur,
        Deferred::Value(angle) if shutter_offset(angle).is_none()
    ) && !(matches!(params.position, Deferred::Value(_))
        && matches!(params.scale, Deferred::Value(_))
        && matches!(params.rotation, Deferred::Value(_))
        && matches!(params.anchor, Deferred::Value(_)));
    let constant = constant_text(ctx, text);
    let bounds = if blurred {
        None
    } else {
        text_bounds(ctx, text, constant.as_ref())
    };
    let capacity = constant.as_ref().map_or(MAX_TEXT_GLYPHS, |constant| {
        let strokes = 1 + usize::from(stroked(params));
        (constant.shaped.glyph_count() * strokes + constant.decorations.len())
            .clamp(1, MAX_TEXT_GLYPHS)
    });
    let domain = ctx.source_domain(bounds);
    let texture = ctx.builder_mut().texture(
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
        let fit = |canvas: u32| (canvas * TEXT_LAYER_SCALE).min(MAX_TEXTURE_DIMENSION.max(canvas));
        lumen_gpu::Size::new(fit(size.width), fit(size.height))
    } else {
        size
    };
    let layer = blurred.then(|| {
        ctx.builder_mut().texture(
            Some(format!("text:{}:layer", text.id.0)),
            lumen_gpu::TextureDesc::render_target(
                layer_size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            ),
        )
    });
    let atlas_texture = ctx.text_atlas_texture();
    let globals_buffer = ctx.builder_mut().buffer(
        Some(format!("text:{}:globals", text.id.0)),
        lumen_gpu::BufferDesc::uniform(std::mem::size_of::<lumen_text::GpuTextGlobals>() as u64),
    );
    let instances_buffer = ctx.builder_mut().buffer(
        Some(format!("text:{}:instances", text.id.0)),
        lumen_gpu::BufferDesc::storage(
            (capacity * std::mem::size_of::<lumen_text::GpuGlyphInstance>()) as u64,
        ),
    );
    let style_buffer = ctx.builder_mut().buffer(
        Some(format!("text:{}:style", text.id.0)),
        lumen_gpu::BufferDesc::uniform(std::mem::size_of::<TextStyleUniforms>() as u64),
    );
    let paint_count = match &params.content {
        TextContentDelegate::Spans(spans) => spans.items.len().max(1),
        TextContentDelegate::Text(_) => 1,
    };
    let decoration_paints = decoration_paints(params);
    let paints = paint_count + decoration_paints.unwrap_or(0);
    if paints > lumen_text::GLYPH_MAX_PAINTS as usize {
        return Err(RenderError::NodeEvaluation {
            frame: 0,
            node_id: text.id,
            node_kind: "Text",
            details: format!(
                "text has {paints} span and decoration paints, more than the {} that can be \
                 painted",
                lumen_text::GLYPH_MAX_PAINTS
            ),
        }
        .into());
    }
    let paints_buffer = ctx.builder_mut().buffer(
        Some(format!("text:{}:paints", text.id.0)),
        lumen_gpu::BufferDesc::storage((paints * std::mem::size_of::<GpuTextPaint>()) as u64),
    );
    let atlas_sampler = ctx.builder_mut().sampler(
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
            lumen_gpu::BindingLayoutEntry::storage(
                6,
                lumen_gpu::wgpu::ShaderStages::FRAGMENT,
                true,
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
            primitive: lumen_gpu::wgpu::PrimitiveState::default(),
        })
    };
    let bindings = vec![
        lumen_gpu::Binding::uniform(0, 0, globals_buffer),
        lumen_gpu::Binding::sampled_texture(0, 1, atlas_texture),
        lumen_gpu::Binding::sampler(0, 2, atlas_sampler),
        lumen_gpu::Binding::storage_buffer(0, 3, instances_buffer),
        lumen_gpu::Binding::uniform(0, 4, style_buffer),
        lumen_gpu::Binding::storage_buffer(0, 6, paints_buffer),
    ];
    let text_pass =
        |label: &str, program, target, load, bindings, draw, viewport| lumen_gpu::RenderPassDesc {
            label: Some(format!("text:{}:{label}", text.id.0)),
            program,
            targets: vec![lumen_gpu::RenderTargetRef {
                texture: target,
                load,
                store: lumen_gpu::wgpu::StoreOp::Store,
            }],
            bindings,
            draw,
            viewport,
        };
    let fullscreen = lumen_gpu::Draw {
        vertices: 0..3,
        instances: 0..1,
    };
    let program = ctx.builder_mut().program(text_program(
        "text",
        "vs_main",
        "fs_main",
        lumen_gpu::wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING,
        false,
    ));
    ctx.builder_mut().render_pass(text_pass(
        "render",
        program,
        layer.unwrap_or(texture),
        lumen_gpu::LoadOp::Clear(lumen_gpu::wgpu::Color::TRANSPARENT),
        bindings.clone(),
        lumen_gpu::Draw {
            vertices: 0..6,
            instances: 0..capacity as u32,
        },
        viewport,
    ));
    if let Some(layer) = layer {
        let program = ctx.builder_mut().program(text_program(
            "text smear",
            "vs_fullscreen",
            "fs_smear",
            lumen_gpu::wgpu::BlendState::REPLACE,
            true,
        ));
        let mut bindings = bindings.clone();
        bindings.push(lumen_gpu::Binding::sampled_texture(0, 5, layer));
        ctx.builder_mut().render_pass(text_pass(
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
        let program = ctx.builder_mut().program(text_program(
            "text opacity",
            "vs_fullscreen",
            "fs_opacity",
            lumen_gpu::wgpu::BlendState {
                color: multiply,
                alpha: multiply,
            },
            false,
        ));
        ctx.builder_mut().render_pass(text_pass(
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
    let expr = ctx.expr_context(text.id, "params");
    let decorations = DecorationParams::take(&mut params);
    let decorations = if decorations.has_expressions() {
        NodeDecorations::Animated(decorations)
    } else {
        NodeDecorations::Constant(decorations.eval(text.id, &expr)?)
    };
    let font_features = if crate::node::DelegateEvaluable::has_expressions(&params.font_features) {
        None
    } else {
        let features = std::mem::take(&mut params.font_features);
        Some(crate::node::DelegateEvaluable::eval(
            &features,
            &DelegateEvalContext {
                node_id: text.id,
                property_path: "font_features",
                expr: &expr,
            },
        )?)
    };
    let content = if crate::node::DelegateEvaluable::has_expressions(&params.content) {
        None
    } else {
        let content = std::mem::take(&mut params.content);
        Some(crate::node::DelegateEvaluable::eval(
            &content,
            &DelegateEvalContext {
                node_id: text.id,
                property_path: "content",
                expr: &expr,
            },
        )?)
    };
    ctx.register_compiled_node(CompiledText {
        node_id: text.id,
        params,
        glyph,
        content,
        decorations,
        font_features,
        decorated: decoration_paints.is_some(),
        stroked: stroked(&text.params),
        atlas_texture,
        globals_buffer,
        instances_buffer,
        style_buffer,
        paints_buffer,
        paint_count,
        capacity,
        size,
        layer_size,
        state: std::sync::Arc::new(std::sync::Mutex::new(TextState::default())),
    });

    Ok(CompiledOutput::Raster(RasterHandle {
        texture,
        domain,
        metadata: RasterMetadata::default(),
        bounds: None,
    }))
}

/// The canvas pixels unblurred text may draw to, or `None` when it should fill the
/// canvas: the ink of its glyphs, as the font that will draw it measures them, and its
/// decorations.
///
/// Only text whose paints are solid colours is bounded, besides the constant text that
/// `constant` holds: its paint is evaluated at the pixel it is drawn to, which a viewport
/// offset moves. Text with `glyph_*` params is not bounded either, since they move glyphs
/// off the layout and fade clusters across their union.
fn text_bounds(
    ctx: &CompileContext<'_>,
    text: &Text,
    constant: Option<&ConstantText>,
) -> Option<lumen_gpu::RectI> {
    let params = &text.params;
    let solid = |paint: &PaintDelegate| matches!(paint, PaintDelegate::SolidColor(..));
    let solid_line = |line: &TextLineDelegate| line.paint.as_ref().is_none_or(solid);
    let solid_span = |span: &TextSpanDelegate| {
        span.color.as_ref().is_none_or(solid)
            && [&span.underline, &span.strikethrough]
                .into_iter()
                .all(|line| !matches!(line, Override::Set(line) if !solid_line(line)))
            && !matches!(&span.highlight, Override::Set(highlight) if !solid(&highlight.paint))
            && !matches!(&span.stroke, Override::Set(stroke) if !solid(&stroke.paint))
    };
    let constant = constant?;
    if GlyphParams::any(params)
        || !solid(&params.color)
        || matches!(&params.content, TextContentDelegate::Spans(spans)
            if !spans.items.iter().all(solid_span))
        || !params
            .stroke
            .as_ref()
            .is_none_or(|stroke| solid(&stroke.paint))
        || !params
            .underline
            .iter()
            .chain(&params.strikethrough)
            .all(solid_line)
        || !params
            .highlight
            .as_ref()
            .is_none_or(|highlight| solid(&highlight.paint))
    {
        return None;
    }
    let ink = text_system(ctx.media())
        .ok()?
        .ink_bounds(&constant.shaped)?;
    let reach = constant.stroke_reach;
    let ink = [
        ink[0] - reach,
        ink[1] - reach,
        ink[2] + reach,
        ink[3] + reach,
    ];
    let ink = constant
        .decorations
        .iter()
        .fold(ink, |[x0, y0, x1, y1], rect| {
            let [min_x, min_y, max_x, max_y] = rect.rect_bounds();
            [x0.min(min_x), y0.min(min_y), x1.max(max_x), y1.max(max_y)]
        });
    let line_height = constant.shaped.measurement.height;
    let alignment = constant.alignment;
    let animated = params.position.is_expr()
        || params.scale.is_expr()
        || params.rotation.is_expr()
        || params.anchor.is_expr();
    let motion = compiler::SourceMotion::of(animated, params);
    ctx.source_bounds(text.id, motion, |expr| {
        let position = params.position.eval(text.id, "position", expr)?;
        let top = match alignment {
            TextAlignmentVertical::Top => position.1 as f32,
            TextAlignmentVertical::Middle => position.1 as f32 - line_height * 0.5,
            TextAlignmentVertical::Bottom => position.1 as f32 - line_height,
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

/// The layout and decorations of `text`, shaped as the font that will draw it shapes
/// them, when nothing that sets them is animated. Text in a font that the media store has
/// not resolved has none, since the font drawn later may shape it differently.
fn constant_text(ctx: &CompileContext<'_>, text: &Text) -> Option<ConstantText> {
    let params = &text.params;
    if crate::node::DelegateEvaluable::has_expressions(&params.content)
        || crate::node::DelegateEvaluable::has_expressions(&params.underline)
        || crate::node::DelegateEvaluable::has_expressions(&params.strikethrough)
        || crate::node::DelegateEvaluable::has_expressions(&params.highlight)
        || crate::node::DelegateEvaluable::has_expressions(&params.stroke)
        || params.font_family.is_expr()
        || params.font_size.is_expr()
        || params.font_weight.is_expr()
        || crate::node::DelegateEvaluable::has_expressions(&params.font_style)
        || crate::node::DelegateEvaluable::has_expressions(&params.font_features)
        || params.max_width.is_expr()
        || params.letter_spacing.is_expr()
        || params.line_height.is_expr()
        || crate::node::DelegateEvaluable::has_expressions(&params.max_lines)
        || crate::node::DelegateEvaluable::has_expressions(&params.max_height)
        || crate::node::DelegateEvaluable::has_expressions(&params.overflow)
        || crate::node::DelegateEvaluable::has_expressions(&params.fit)
        || params.min_font_size.is_expr()
        || params.paragraph_spacing.is_expr()
        || crate::node::DelegateEvaluable::has_expressions(&params.text_transform)
    {
        return None;
    }
    // What sets how the text is drawn, not laid out, may read other nodes, so it is left
    // for binding to evaluate.
    let drawn = TextParamsDelegate::default();
    let layout_params = TextParamsDelegate {
        position: drawn.position,
        scale: drawn.scale,
        rotation: drawn.rotation,
        anchor: drawn.anchor,
        opacity: drawn.opacity,
        motion_blur: drawn.motion_blur,
        glyph_offset: drawn.glyph_offset,
        glyph_scale: drawn.glyph_scale,
        glyph_rotation: drawn.glyph_rotation,
        glyph_opacity: drawn.glyph_opacity,
        color: drawn.color,
        ..params.clone()
    };
    let expr = ctx.expr_context(text.id, "params");
    let mut evaluated = layout_params
        .eval(&NodeParamEvalContext {
            node_id: text.id,
            expr: &expr,
        })
        .ok()?;
    let style = TextLayoutStyle::take(&mut evaluated);
    let request = style.request(&evaluated.content, &evaluated.font_features);
    let unresolved = request.spans.iter().any(|span| {
        let family = span.font_family.as_str();
        !family.is_empty()
            && family != lumen_text::DEFAULT_FONT_FAMILY
            && !ctx
                .media()
                .is_some_and(|media| media.font_resolver_id(family).is_some())
    });
    if unresolved {
        return None;
    }
    let shaped = {
        let mut text_system = text_system(ctx.media()).ok()?;
        load_font_families(
            &mut text_system,
            ctx.media(),
            &style,
            Some(&evaluated.content),
        )
        .ok()?;
        text_system.shape(&request)
    };
    let decorations = TextDecorations {
        underline: evaluated.underline.take(),
        strikethrough: evaluated.strikethrough.take(),
        highlight: evaluated.highlight.take(),
        stroke: evaluated.stroke.take(),
    };
    let spans = match &evaluated.content {
        TextContent::Spans(spans) => &spans[..],
        TextContent::Text(_) => &[],
    };
    let mut rects = Vec::new();
    decorate(
        &shaped,
        &decorations,
        spans,
        [0.0; 2],
        spans.len().max(1),
        &mut rects,
    );
    let mut strokes = Vec::new();
    span_strokes(
        decorations.stroke.as_ref(),
        spans,
        &shaped,
        |_, _, _| 0,
        &mut strokes,
    );
    let stroke_reach = strokes
        .iter()
        .flatten()
        .map(|stroke| lumen_text::stroke_reach(stroke.half_width))
        .fold(0.0, f32::max);
    Some(ConstantText {
        shaped,
        alignment: evaluated.alignment_vertical,
        decorations: rects,
        stroke_reach,
    })
}
