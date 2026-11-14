use bytemuck::{Pod, Zeroable};

use crate::error::RenderError;
use crate::gpu::frame_graph::RenderProgram;
use crate::node::motion::shutter_offset;
use crate::node::vector::{
    drawable::{DRAWABLE_SHADER, GpuDrawableMotion, GpuDrawableTransform},
    paint::{GpuPaint, PaintDelegate},
};
use crate::node::{Deferred, DelegateEvalContext, Override};

use super::text::{Stable, Text, TextInstance, TextState, decoration_paints, stroked};
use super::text_content::{TextContentDelegate, TextLineDelegate, TextSpanDelegate};
use super::text_fonts::used_font_families;

fn text_shader() -> String {
    [
        include_str!("../../gpu/shaders/alpha.wgsl"),
        include_str!("../../gpu/shaders/color.wgsl"),
        include_str!("../vector/paint/paint.wgsl"),
        DRAWABLE_SHADER,
        lumen_text::TEXT_SHADER,
    ]
    .concat()
}
/// Times the canvas, along each axis, that the layer of animated, motion-blurred text spans,
/// so that text may sweep the canvas by that much less one canvas over its shutter. Text that
/// sweeps further is drawn at a lower scale to fit.
const TEXT_LAYER_SCALE: u32 = 2;
/// Largest texture dimension of the devices that engine hosts request, whose limits are
/// wgpu's defaults.
const MAX_TEXTURE_DIMENSION: u32 = 8192;

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "TextStyle", source = shared_text_shader())]
pub(crate) struct TextStyleUniforms {
    pub(crate) transform: GpuDrawableTransform,
    pub(crate) motion: GpuDrawableMotion,
}

/// A paint of a text, and the box `[x, y, width, height]` its relative coordinates map to
/// `0..1`.
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "TextPaint", source = shared_text_shader())]
pub(crate) struct GpuTextPaint {
    pub(crate) paint: GpuPaint,
    pub(crate) bounds: [f32; 4],
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

/// Whether text's output may be sized to its ink: its paints are all solid colours and no
/// `glyph_*` param moves its glyphs. A paint is otherwise evaluated at the pixel it is drawn to,
/// which a viewport offset moves, and `glyph_*` params move glyphs off the layout and fade clusters
/// across their union.
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
        return Err(RenderError::evaluating::<Text>(
            text.id,
            format!(
                "text has {paints} span and decoration paints, more than the {} that can be \
                 painted",
                lumen_text::GLYPH_MAX_PAINTS
            ),
        )
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

pub(crate) fn shared_text_shader() -> &'static str {
    static SHADER: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    SHADER.get_or_init(text_shader)
}

fn text_blend(blend: lumen_gpu::wgpu::BlendState) -> lumen_gpu::RenderTargetSpec {
    lumen_gpu::RenderTargetSpec::flat(crate::gpu::WORKING_COLOR_FORMAT, blend)
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

/// A pass of text: its program, and the target and variant it draws with.
struct TextPass {
    program: RenderProgram,
    target: lumen_gpu::RenderTargetSpec,
    variant: u64,
}

/// Draws the glyphs, and the decorations.
fn glyph_pass() -> TextPass {
    TextPass {
        program: RenderProgram {
            label: "text",
            shader: shared_text_shader(),
            vertex: "vs_main",
            fragment: "fs_main",
            layout: RECORD_TEXT_LAYOUT,
        },
        target: text_blend(lumen_gpu::wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
        variant: 0,
    }
}

/// Smears the layer of motion-blurred text into the target.
fn smear_pass() -> TextPass {
    TextPass {
        program: RenderProgram {
            label: "text smear",
            shader: shared_text_shader(),
            vertex: "vs_fullscreen",
            fragment: "fs_smear",
            layout: RECORD_SMEAR_LAYOUT,
        },
        target: text_blend(lumen_gpu::wgpu::BlendState::REPLACE),
        variant: 1,
    }
}

/// Fades the drawn text by its opacity.
fn opacity_pass() -> TextPass {
    TextPass {
        program: RenderProgram {
            label: "text opacity",
            shader: shared_text_shader(),
            vertex: "vs_fullscreen",
            fragment: "fs_opacity",
            layout: RECORD_TEXT_LAYOUT,
        },
        target: text_blend(opacity_blend()),
        variant: 2,
    }
}

/// Builds the programs text draws with.
pub(crate) fn text_programs(
    programs: &mut crate::gpu::frame_graph::Programs<'_>,
) -> crate::Result<()> {
    for pass in [glyph_pass(), smear_pass(), opacity_pass()] {
        programs.render(&pass.program, pass.target, pass.variant)?;
    }
    Ok(())
}

/// Records a text, drawing what it placed this frame.
pub(crate) fn record_text(
    text: &Text,
    cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
) -> crate::Result<crate::gpu::frame_graph::Raster> {
    use lumen_gpu::{Res, TargetLoad, wgpu};

    let size = cx.canvas_size();
    let motion_blur = cx.analytic_motion_blur();
    let blurred = motion_blur && text_blurred(text);
    let layer_size = text_layer_size(blurred, size);
    let atlas = cx.atlas()?;
    let instance = cx.text_instance(text.id, |cx| {
        text_paint_count(text)?;
        let params = &text.params;
        let mut layout = params.layout.clone();
        let span_styles = layout.content.take_span_styles();
        let expr = cx.expr();
        let fields = DelegateEvalContext {
            node_id: text.id,
            expr: &expr,
        };
        Ok(TextInstance {
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
            paint_count: text_span_paints(text),
            size,
            layer_size,
            measure_ink: !blurred && text_may_be_bounded(text),
            motion_blur,
            state: std::sync::Arc::new(std::sync::Mutex::new(TextState::default())),
        })
    })?;
    let families: Vec<String> = {
        let expr = cx.expr();
        let fields = DelegateEvalContext {
            node_id: text.id,
            expr: &expr,
        };
        let layout = instance.layout.get(&fields)?;
        used_font_families(&layout).map(str::to_string).collect()
    };
    for family in &families {
        cx.note_font(family);
    }
    let drawn = cx.bind_text(&instance)?;

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
        (0, drawn.globals),
        (1, Res::Tex(atlas)),
        (2, Res::Sampler(sampler)),
        (3, drawn.instances),
        (4, drawn.style),
        (6, drawn.paints),
    ];
    let pass = glyph_pass();
    let program = cx.render_program(&pass.program, pass.target, pass.variant)?;
    let layer = blurred.then(|| cx.render_target(layer_size));
    cx.rec.draw(
        program,
        &shared,
        layer.unwrap_or(target),
        TargetLoad::Clear,
        6,
        drawn.glyph_count,
        viewport,
    );
    if let Some(layer) = layer {
        let pass = smear_pass();
        let program = cx.render_program(&pass.program, pass.target, pass.variant)?;
        let mut bindings = shared.to_vec();
        bindings.push((5, Res::Tex(layer)));
        cx.rec
            .draw(program, &bindings, target, TargetLoad::Clear, 3, 1, None);
    }
    // Fading the composited layer keeps overlapping glyphs from doubling up.
    if drawn.opacity < 1.0 {
        let pass = opacity_pass();
        let program = cx.render_program(&pass.program, pass.target, pass.variant)?;
        cx.rec
            .draw(program, &shared, target, TargetLoad::Load, 3, 1, None);
    }
    Ok(crate::gpu::frame_graph::Raster {
        tex: target,
        domain,
        bounds: None,
    })
}
