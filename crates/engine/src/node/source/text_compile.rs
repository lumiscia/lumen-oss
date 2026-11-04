use bytemuck::{Pod, Zeroable};

use crate::error::RenderError;
use crate::gpu::{CompileContext, RasterHandle, compiler};
use crate::node::motion::shutter_offset;
use crate::node::vector::{
    drawable::{DRAWABLE_SHADER, DrawableTransform, GpuDrawableMotion, GpuDrawableTransform},
    paint::{GpuPaint, PaintDelegate},
};
use crate::node::{Deferred, DelegateEvalContext, DelegateEvaluable, Override};

use super::text::{
    CompiledText, Stable, Text, TextAlignmentVertical, TextDecorationParamsDelegate,
    TextLayoutParamsDelegate, TextState, decorate, decoration_paints, load_font_families,
    span_strokes, stroked, text_system,
};
use super::text_content::{
    SpanStyleDelegate, TextContentDelegate, TextLineDelegate, TextSpanDelegate,
};

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
    shaped: std::sync::Arc<lumen_text::ShapedText>,
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
) -> crate::Result<RasterHandle> {
    let size = ctx.canvas_size();
    let params = &text.params;
    let blurred = text_blurred(text);
    // What sets how the text is drawn, not laid out, may read other nodes, so it is left for
    // binding to evaluate.
    let mut layout = params.layout.clone();
    let span_styles = layout.content.take_span_styles();
    let (layout, span_styles, decorations) = {
        let expr = ctx.expr_context();
        let fields = DelegateEvalContext {
            node_id: text.id,
            expr: &expr,
        };
        (
            Stable::new(layout, &fields)?,
            Stable::new(span_styles, &fields).map_err(|error| error.in_field("content"))?,
            Stable::new(params.decorations.clone(), &fields)?,
        )
    };
    let constant = constant_text(ctx, text, &layout, &span_styles, &decorations);
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
    let layer_size = text_layer_size(blurred, size);
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
    let paint_count = text_span_paints(text);
    let paints = text_paint_count(text)?;
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
    if !matches!(text.params.placement.opacity, Deferred::Value(opacity) if opacity >= 1.0) {
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
    ctx.register_compiled_node(CompiledText {
        node_id: text.id,
        draw: text.params.draw.clone(),
        placement: text.params.placement.clone(),
        glyph: text.params.glyph.clone(),
        layout,
        span_styles,
        decorations,
        decorated: decoration_paints(params).is_some(),
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
        measure_ink: false,
        state: std::sync::Arc::new(std::sync::Mutex::new(TextState::default())),
    });

    Ok(RasterHandle {
        texture,
        domain,
        bounds: None,
    })
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
    let constant = constant?;
    if !text_may_be_bounded(text) {
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
    let placement = &params.placement;
    let motion = compiler::SourceMotion::of(placement.transform_is_animated(), params);
    ctx.source_bounds(motion, |expr| {
        let position = placement.position.eval(text.id, "position", expr)?;
        let top = match alignment {
            TextAlignmentVertical::Top => position.1 as f32,
            TextAlignmentVertical::Middle => position.1 as f32 - line_height * 0.5,
            TextAlignmentVertical::Bottom => position.1 as f32 - line_height,
        };
        let min = [position.0 as f32 + ink[0], top + ink[1]];
        let max = [position.0 as f32 + ink[2], top + ink[3]];
        let transform = DrawableTransform {
            position,
            scale: placement.scale.eval(text.id, "scale", expr)?,
            rotation: placement.rotation.eval(text.id, "rotation", expr)?,
            anchor: placement.anchor.eval(text.id, "anchor", expr)?,
            opacity: 1.0,
        };
        let [x0, y0, x1, y1] = transform.to_gpu().canvas_bounds(min, max);
        Ok(Some([x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0]))
    })
}

/// The layout and decorations of `text`, shaped as the font that will draw it shapes
/// them, when nothing that sets them is animated. Text in a font that the media store has
/// not resolved has none, since the font drawn later may shape it differently.
fn constant_text(
    ctx: &CompileContext<'_>,
    text: &Text,
    layout: &Stable<TextLayoutParamsDelegate>,
    span_styles: &Stable<Vec<SpanStyleDelegate>>,
    decorations: &Stable<TextDecorationParamsDelegate>,
) -> Option<ConstantText> {
    let (Stable::Constant(layout), Stable::Constant(spans), Stable::Constant(decorations)) =
        (layout, span_styles, decorations)
    else {
        return None;
    };
    let request = layout.request(spans);
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
        load_font_families(&mut text_system, ctx.media(), layout).ok()?;
        text_system.shape(&request)
    };
    let mut rects = Vec::new();
    decorate(
        &shaped,
        decorations,
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
    let expr = ctx.expr_context();
    let alignment = text
        .params
        .draw
        .alignment_vertical
        .eval(&DelegateEvalContext {
            node_id: text.id,
            expr: &expr,
        })
        .ok()?;
    Some(ConstantText {
        shaped,
        alignment,
        decorations: rects,
        stroke_reach,
    })
}

/// Whether text may move during the shutter: it is then drawn still to a layer first, which is
/// smeared over the shutter's taps into the output. Text with motion blur constantly off, or
/// whose transform is constant, is drawn to the output directly.
fn text_blurred(text: &Text) -> bool {
    let placement = &text.params.placement;
    !matches!(
        placement.motion_blur,
        Deferred::Value(angle) if shutter_offset(angle).is_none()
    ) && placement.transform_is_animated()
}

fn text_layer_size(blurred: bool, size: lumen_gpu::Size) -> lumen_gpu::Size {
    if !blurred {
        return size;
    }
    let fit = |canvas: u32| (canvas * TEXT_LAYER_SCALE).min(MAX_TEXTURE_DIMENSION.max(canvas));
    lumen_gpu::Size::new(fit(size.width), fit(size.height))
}

/// Whether text's paints are all solid colours and no `glyph_*` param moves its glyphs. A paint
/// is otherwise evaluated at the pixel it is drawn to, which a viewport offset moves.
fn text_may_be_bounded(text: &Text) -> bool {
    let params = &text.params;
    let solid = |paint: &PaintDelegate| matches!(paint, PaintDelegate::SolidColor(..));
    let solid_line = |line: &TextLineDelegate| line.paint.as_ref().is_none_or(solid);
    let solid_span = |span: &TextSpanDelegate| {
        let style = &span.style;
        style.color.as_ref().is_none_or(solid)
            && [&style.underline, &style.strikethrough]
                .into_iter()
                .all(|line| !matches!(line, Override::Set(line) if !solid_line(line)))
            && !matches!(&style.highlight, Override::Set(highlight) if !solid(&highlight.paint))
            && !matches!(&style.stroke, Override::Set(stroke) if !solid(&stroke.paint))
    };
    let decorations = &params.decorations;
    params.glyph.is_identity()
        && solid(&params.draw.color)
        && !matches!(&params.layout.content, TextContentDelegate::Spans(spans)
            if !spans.iter().all(solid_span))
        && decorations
            .stroke
            .as_ref()
            .is_none_or(|stroke| solid(&stroke.paint))
        && decorations
            .underline
            .iter()
            .chain(&decorations.strikethrough)
            .all(solid_line)
        && decorations
            .highlight
            .as_ref()
            .is_none_or(|highlight| solid(&highlight.paint))
}

fn text_span_paints(text: &Text) -> usize {
    match &text.params.layout.content {
        TextContentDelegate::Spans(spans) => spans.len().max(1),
        TextContentDelegate::Text(_) => 1,
    }
}

/// How many paints the text binds, failing when that is more than can be painted.
fn text_paint_count(text: &Text) -> crate::Result<usize> {
    let paints = text_span_paints(text) + decoration_paints(&text.params).unwrap_or(0);
    if paints > lumen_text::GLYPH_MAX_PAINTS as usize {
        return Err(RenderError::NodeEvaluation {
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
    Ok(paints)
}

const VF: lumen_gpu::wgpu::ShaderStages = lumen_gpu::wgpu::ShaderStages::VERTEX_FRAGMENT;
const FRAG: lumen_gpu::wgpu::ShaderStages = lumen_gpu::wgpu::ShaderStages::FRAGMENT;
const RECORD_TEXT_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::staged(0, VF, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::staged(1, FRAG, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::staged(2, FRAG, lumen_gpu::LayoutKind::Sampler),
    lumen_gpu::LayoutEntry::staged(3, VF, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::staged(4, VF, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::staged(6, FRAG, lumen_gpu::LayoutKind::Storage),
];
const RECORD_SMEAR_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::staged(0, VF, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::staged(1, FRAG, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::staged(2, FRAG, lumen_gpu::LayoutKind::Sampler),
    lumen_gpu::LayoutEntry::staged(3, VF, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::staged(4, VF, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::staged(5, FRAG, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::staged(6, FRAG, lumen_gpu::LayoutKind::Storage),
];

fn shared_text_shader() -> &'static str {
    static SHADER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SHADER.get_or_init(text_shader)
}

const GLOBALS: lumen_gpu::BufferId = lumen_gpu::BufferId(0);
const INSTANCES: lumen_gpu::BufferId = lumen_gpu::BufferId(1);
const STYLE: lumen_gpu::BufferId = lumen_gpu::BufferId(2);
const PAINTS: lumen_gpu::BufferId = lumen_gpu::BufferId(3);

fn text_blend(blend: lumen_gpu::wgpu::BlendState) -> lumen_gpu::RenderTargetSpec {
    lumen_gpu::RenderTargetSpec {
        format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
        blend,
    }
}

fn opacity_blend() -> lumen_gpu::wgpu::BlendState {
    let multiply = lumen_gpu::wgpu::BlendComponent {
        src_factor: lumen_gpu::wgpu::BlendFactor::Zero,
        dst_factor: lumen_gpu::wgpu::BlendFactor::SrcAlpha,
        operation: lumen_gpu::wgpu::BlendOperation::Add,
    };
    lumen_gpu::wgpu::BlendState {
        color: multiply,
        alpha: multiply,
    }
}

/// Builds the programs text draws with.
pub(crate) fn text_programs(
    programs: &mut crate::gpu::frame_graph::Programs<'_>,
) -> crate::Result<()> {
    use lumen_gpu::wgpu::BlendState;
    let shader = shared_text_shader();
    programs.render(
        "text",
        shader,
        "vs_main",
        "fs_main",
        RECORD_TEXT_LAYOUT,
        text_blend(BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        0,
    )?;
    programs.render(
        "text smear",
        shader,
        "vs_fullscreen",
        "fs_smear",
        RECORD_SMEAR_LAYOUT,
        text_blend(BlendState::REPLACE),
        1,
    )?;
    programs.render(
        "text opacity",
        shader,
        "vs_fullscreen",
        "fs_opacity",
        RECORD_TEXT_LAYOUT,
        text_blend(opacity_blend()),
        2,
    )
}

/// Records a text, binding it as a plan would and drawing what it placed this frame. Its
/// instances still fit `MAX_TEXT_GLYPHS`, which bounds the work one node can ask for.
pub(crate) fn record_text(
    text: &Text,
    cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
) -> crate::Result<crate::gpu::frame_graph::Raster> {
    use lumen_gpu::{Res, TargetLoad, wgpu};

    let size = cx.canvas_size();
    let blurred = text_blurred(text);
    let layer_size = text_layer_size(blurred, size);
    let atlas = cx.atlas()?;
    let compiled = cx.text_instance(text.id, |cx| {
        text_paint_count(text)?;
        let params = &text.params;
        let mut layout = params.layout.clone();
        let span_styles = layout.content.take_span_styles();
        let expr = cx.expr();
        let fields = DelegateEvalContext {
            node_id: text.id,
            expr: &expr,
        };
        Ok(CompiledText {
            node_id: text.id,
            draw: params.draw.clone(),
            placement: params.placement.clone(),
            glyph: params.glyph.clone(),
            layout: Stable::new(layout, &fields)?,
            span_styles: Stable::new(span_styles, &fields)
                .map_err(|error| error.in_field("content"))?,
            decorations: Stable::new(params.decorations.clone(), &fields)?,
            decorated: decoration_paints(params).is_some(),
            stroked: stroked(params),
            atlas_texture: lumen_gpu::TextureId(0),
            globals_buffer: GLOBALS,
            instances_buffer: INSTANCES,
            style_buffer: STYLE,
            paints_buffer: PAINTS,
            paint_count: text_span_paints(text),
            capacity: MAX_TEXT_GLYPHS,
            size,
            layer_size,
            measure_ink: !blurred && text_may_be_bounded(text),
            state: std::sync::Arc::new(std::sync::Mutex::new(TextState::default())),
        })
    })?;
    for family in std::iter::once(&text.params.layout.font_family).chain(
        match &text.params.layout.content {
            TextContentDelegate::Spans(spans) => spans.as_slice(),
            TextContentDelegate::Text(_) => &[],
        }
        .iter()
        .filter_map(|span| span.layout.font_family.as_ref()),
    ) {
        if let Deferred::Value(family) = family {
            cx.note_font(family);
        }
    }
    let bind = cx.bind_context();
    compiled.bind_text(&bind, &mut cx.bound, false)?;
    let drawn = compiled.drawn();

    cx.bound.keep_recorded_atlas_regions();
    let uploads = cx.bound.take_buffer_uploads();
    let part = |id: lumen_gpu::BufferId| {
        uploads
            .iter()
            .filter(move |(buffer, ..)| *buffer == id)
            .map(|(_, _, data)| data.as_slice())
    };
    let globals_data = part(GLOBALS).next().expect("text globals");
    let globals: lumen_text::GpuTextGlobals = bytemuck::pod_read_unaligned(globals_data);
    let globals_res = cx.rec.uniform_bytes(globals_data);
    let style_res = cx
        .rec
        .uniform_bytes(part(STYLE).next().expect("text style"));
    let instance_parts: Vec<&[u8]> = part(INSTANCES).collect();
    let instances = cx.rec.storage_parts(&instance_parts);
    let paints = cx
        .rec
        .storage_parts(&[part(PAINTS).next().expect("text paints")]);

    let bounds = cx.bounds_of(drawn.canvas_bounds);
    let domain = cx.source_domain(bounds);
    let target = cx.render_target(domain.storage_size);
    let viewport = (!domain.is_full_frame()).then_some([
        -domain.data_rect.x as f32,
        -domain.data_rect.y as f32,
        size.width as f32,
        size.height as f32,
    ]);
    let sampler = cx.sampler(3, || wgpu::SamplerDescriptor {
        label: Some("lumen text atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Nearest,
        ..Default::default()
    });
    let shared = [
        (0, globals_res),
        (1, Res::Tex(atlas)),
        (2, Res::Sampler(sampler)),
        (3, instances),
        (4, style_res),
        (6, paints),
    ];
    let shader = shared_text_shader();
    let program = cx.render_program(
        "text",
        shader,
        "vs_main",
        "fs_main",
        RECORD_TEXT_LAYOUT,
        text_blend(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        0,
    )?;
    let layer = blurred.then(|| cx.render_target(layer_size));
    cx.rec.draw(
        program,
        &shared,
        layer.unwrap_or(target),
        TargetLoad::Clear,
        6,
        globals.glyph_count,
        viewport,
    );
    drop(instance_parts);
    cx.bound.keep_recorded_uploads(
        uploads
            .into_iter()
            .filter(|(buffer, ..)| *buffer == INSTANCES),
    );
    if let Some(layer) = layer {
        let program = cx.render_program(
            "text smear",
            shader,
            "vs_fullscreen",
            "fs_smear",
            RECORD_SMEAR_LAYOUT,
            text_blend(wgpu::BlendState::REPLACE),
            1,
        )?;
        let mut bindings = shared.to_vec();
        bindings.push((5, Res::Tex(layer)));
        cx.rec
            .draw(program, &bindings, target, TargetLoad::Clear, 3, 1, None);
    }
    // Fading the composited layer keeps overlapping glyphs from doubling up.
    if drawn.opacity < 1.0 {
        let program = cx.render_program(
            "text opacity",
            shader,
            "vs_fullscreen",
            "fs_opacity",
            RECORD_TEXT_LAYOUT,
            text_blend(opacity_blend()),
            2,
        )?;
        cx.rec
            .draw(program, &shared, target, TargetLoad::Load, 3, 1, None);
    }
    Ok(crate::gpu::frame_graph::Raster {
        tex: target,
        domain,
        bounds: None,
    })
}
