use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, LazyLock, Mutex, OnceLock, Weak},
};

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;

use crate::error::{LumenError, MediaError, RenderError};
use crate::expr::{ExprNode, ExpressionContext, GlobalVar, GlyphScope};
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};
use crate::media::{FontResolver, MediaStore, TextSession};
use crate::node::vector::{
    drawable::{DeferredTransform, DrawableMotion, DrawableTransform},
    paint::Paint,
    renderer::{GpuTextPaint, TextStyleUniforms},
};
use crate::node::{
    Deferred, DeferredValue, DelegateEvalContext, NodeId, NodeParamEvalContext, NodeParams,
    Override, PortRef,
};

use super::text_content::{
    FontFeatures, TextContent, TextContentDelegate, TextHighlight, TextHighlightDelegate, TextLine,
    TextLineDelegate, TextLineStyle, TextSpan, TextStroke, TextStrokeAlign, TextStrokeDelegate,
};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum TextFontStyle {
    #[default]
    Normal = 0,
    Italic = 1,
    Oblique = 2,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TextAlignmentHorizontal {
    #[default]
    Left = 0,
    Center = 1,
    Right = 2,
    Justify = 3,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TextAlignmentVertical {
    #[default]
    Top = 0,
    Middle = 1,
    Bottom = 2,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TextPaintSpace {
    #[default]
    Span = 0,
    Text = 1,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TextRenderMode {
    #[default]
    Raster = 0,
    Msdf = 1,
    Auto = 2,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TextOverflow {
    #[default]
    Clip = 0,
    Ellipsis = 1,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TextFit {
    #[default]
    None = 0,
    Shrink = 1,
}

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum TextTransform {
    #[default]
    None = 0,
    Uppercase = 1,
    Lowercase = 2,
    /// Uppercases the first letter of each word.
    Capitalize = 3,
}

impl From<TextTransform> for lumen_text::TextTransform {
    fn from(transform: TextTransform) -> Self {
        match transform {
            TextTransform::None => Self::None,
            TextTransform::Uppercase => Self::Uppercase,
            TextTransform::Lowercase => Self::Lowercase,
            TextTransform::Capitalize => Self::Capitalize,
        }
    }
}

/// Produces a text source.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct TextParams {
    /// Text to render: a string, or spans that each override the style these params give.
    #[meta(multiline, recommended_rows = 4)]
    pub content: TextContent,
    /// Font family name.
    #[meta(format = "font_family")]
    pub font_family: String,
    /// Font size in pixels.
    #[meta(min = 1, step = 1)]
    pub font_size: f64,
    /// Font weight. Variable fonts render any value in their weight range.
    #[meta(min = 1, max = 1000)]
    pub font_weight: i64,
    /// Font style.
    #[meta(kind = "enum", enum_type = TextFontStyle)]
    pub font_style: TextFontStyle,
    /// Glyph rendering mode. MSDF reuses outline distance fields across sizes; Auto is MSDF from 512 px.
    #[meta(kind = "enum", enum_type = TextRenderMode)]
    pub render_mode: TextRenderMode,
    /// Maximum line width in pixels. Use 0 for automatic width.
    #[meta(min = 0, step = 1)]
    pub max_width: f64,
    /// Extra space after every glyph in pixels. Negative values tighten the text.
    #[meta(step = 0.1)]
    pub letter_spacing: f64,
    /// OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0 }`. 0 turns a
    /// feature off, 1 turns it on and larger values select an alternate. Values take literals
    /// or expressions, but the whole map cannot be an expression.
    #[meta()]
    pub font_features: FontFeatures,
    /// Distance between lines in pixels. Use 0 for 1.2 × the font size.
    #[meta(min = 0, step = 1)]
    pub line_height: f64,
    /// Text origin in pixels.
    #[meta()]
    pub position: (f64, f64),
    /// Scale multiplier per axis, about the anchor. Raster glyphs are resampled, so
    /// use MSDF rendering to keep scaled text sharp.
    #[meta()]
    pub scale: (f64, f64),
    /// Clockwise rotation in degrees, about the anchor.
    #[meta(step = 1)]
    pub rotation: f64,
    /// Pivot for scale and rotation, in pixels relative to `position`.
    #[meta()]
    pub anchor: (f64, f64),
    /// Opacity multiplier.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub opacity: f64,
    /// Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the
    /// usual film look and 360 smears over the whole frame. Blurs the text's own movement,
    /// not that of the `glyph_*` params.
    #[meta(min = 0, max = 360, step = 1)]
    pub motion_blur: f64,
    /// Offset of each glyph in pixels. The `glyph_*` params are evaluated for every glyph,
    /// and can read its `glyph`, `word`, `line` and `span` index and their counts.
    #[meta()]
    pub glyph_offset: (f64, f64),
    /// Scale multiplier of each glyph per axis, about the middle of its advance and line.
    #[meta()]
    pub glyph_scale: (f64, f64),
    /// Clockwise rotation of each glyph in degrees, about the middle of its advance and line.
    #[meta(step = 1)]
    pub glyph_rotation: f64,
    /// Opacity multiplier of each glyph, applied before `opacity`.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub glyph_opacity: f64,
    /// Text color, and that of spans without their own.
    #[meta(role = "color")]
    pub color: Paint,
    /// Box a span's own gradient `color` spans: the span's lines, or the whole text. The
    /// node's `color` always spans the whole text.
    #[meta(kind = "enum", enum_type = TextPaintSpace)]
    pub paint_space: TextPaintSpace,
    /// Line under the text, and under spans that leave theirs unset. Off when absent. Its
    /// fields take literals or expressions, but the whole line cannot be an expression.
    #[meta()]
    pub underline: Option<TextLine>,
    /// Line through the text, and through spans that leave theirs unset. Off when absent.
    #[meta()]
    pub strikethrough: Option<TextLine>,
    /// Box behind each line of the text, and of spans that leave theirs unset. Off when
    /// absent.
    #[meta()]
    pub highlight: Option<TextHighlight>,
    /// Outline along the glyphs, and along those of spans that leave theirs unset. Off when
    /// absent. It does not change the layout or the measured size. `glyph_opacity` fades a
    /// glyph's stroke and fill as one.
    #[meta()]
    pub stroke: Option<TextStroke>,
    /// Horizontal text alignment.
    #[meta(kind = "enum", enum_type = TextAlignmentHorizontal)]
    pub alignment_horizontal: TextAlignmentHorizontal,
    /// Vertical text alignment.
    #[meta(kind = "enum", enum_type = TextAlignmentVertical)]
    pub alignment_vertical: TextAlignmentVertical,
    /// Enables 4×4 supersampling when evaluating paint at each glyph pixel.
    #[meta()]
    pub paint_supersample: bool,
    /// Most lines to show. Unlimited when absent.
    #[meta(min = 0, step = 1)]
    pub max_lines: Option<i64>,
    /// Most height in pixels of the lines to show. Unlimited when absent.
    #[meta(min = 0, step = 1)]
    pub max_height: Option<f64>,
    /// How the last line shown ends when lines past `max_lines` or `max_height` are dropped: as
    /// it is, or in an ellipsis. Clusters at that line's end are dropped until the ellipsis fits
    /// `max_width`. The ellipsis takes the style and paint of the text it follows, and is one
    /// glyph, numbered after the last one shown.
    #[meta(kind = "enum", enum_type = TextOverflow)]
    pub overflow: TextOverflow,
    /// Whether the text shrinks to fit `max_width`, `max_lines` and `max_height` without breaking
    /// a word, before any lines are dropped. Every font size, letter spacing, `line_height`,
    /// baseline shift, paragraph spacing, decoration size and stroke width scales by the largest
    /// factor up to 1 that fits.
    #[meta(kind = "enum", enum_type = TextFit)]
    pub fit: TextFit,
    /// Smallest `font_size` that `fit` shrinks the text to. Lines that still do not fit are
    /// dropped by `overflow`.
    #[meta(min = 1, step = 1)]
    pub min_font_size: f64,
    /// Case applied to the text, and to spans that leave theirs unset.
    #[meta(kind = "enum", enum_type = TextTransform)]
    pub text_transform: TextTransform,
    /// Extra space in pixels after each paragraph, but the last. A line break starts a new
    /// paragraph.
    #[meta(min = 0, step = 1)]
    pub paragraph_spacing: f64,
}

impl Default for TextParams {
    fn default() -> Self {
        Self {
            content: TextContent::default(),
            font_family: lumen_text::DEFAULT_FONT_FAMILY.to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            render_mode: TextRenderMode::Raster,
            max_width: 0.0,
            letter_spacing: 0.0,
            font_features: FontFeatures::default(),
            line_height: 0.0,
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
            motion_blur: 0.0,
            glyph_offset: (0.0, 0.0),
            glyph_scale: (1.0, 1.0),
            glyph_rotation: 0.0,
            glyph_opacity: 1.0,
            color: Paint::solid([255, 255, 255, 255]),
            paint_space: TextPaintSpace::Span,
            underline: None,
            strikethrough: None,
            highlight: None,
            stroke: None,
            alignment_horizontal: TextAlignmentHorizontal::Left,
            alignment_vertical: TextAlignmentVertical::Top,
            paint_supersample: true,
            max_lines: None,
            max_height: None,
            overflow: TextOverflow::Clip,
            fit: TextFit::None,
            min_font_size: 8.0,
            text_transform: TextTransform::None,
            paragraph_spacing: 0.0,
        }
    }
}

/// Produces a text source.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "text", name = "Text", category = "source")]
pub struct Text {
    pub id: NodeId,
    #[params]
    pub params: TextParamsDelegate,
}

impl Default for Text {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: TextParamsDelegate::default(),
        }
    }
}

impl GpuCompileNode for Text {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        crate::node::vector::renderer::VectorRenderer::new(ctx).compile_text(self, port)
    }
}

#[derive(Debug)]
pub(crate) struct CompiledText {
    pub(crate) node_id: NodeId,
    /// The node's params, less the per-glyph ones in `glyph` and constant `content`.
    pub(crate) params: TextParamsDelegate,
    pub(crate) glyph: GlyphParams,
    /// The content, evaluated at compile when it has no expressions.
    pub(crate) content: Option<TextContent>,
    pub(crate) decorations: NodeDecorations,
    /// The node's font features, evaluated at compile when they have no expressions.
    pub(crate) font_features: Option<FontFeatures>,
    /// Whether the node or any span has a decoration or stroke.
    pub(crate) decorated: bool,
    /// Whether the node or any span has a stroke.
    pub(crate) stroked: bool,
    pub(crate) atlas_texture: lumen_gpu::TextureId,
    pub(crate) globals_buffer: lumen_gpu::BufferId,
    pub(crate) instances_buffer: lumen_gpu::BufferId,
    pub(crate) style_buffer: lumen_gpu::BufferId,
    /// A paint for each span, or the one paint of string content, and then those of the
    /// decorations with their own and of the strokes.
    pub(crate) paints_buffer: lumen_gpu::BufferId,
    /// The number of span paints.
    pub(crate) paint_count: usize,
    pub(crate) max_glyphs: usize,
    pub(crate) size: lumen_gpu::Size,
    /// The size of what the glyphs are drawn to: the layer of blurred text, else the output.
    pub(crate) layer_size: lumen_gpu::Size,
    pub(crate) state: Arc<Mutex<TextState>>,
}

impl GpuCompiledNode for CompiledText {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        self.bind_text(ctx, bound, false).map(drop)
    }

    /// Places the glyphs in the shared atlas as `bind` would, so text bound later in the frame
    /// finds the atlas as it would without kept output.
    fn bind_kept(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<bool> {
        self.bind_text(ctx, bound, true)
    }
}

impl CompiledText {
    /// Binds the text, stopping once its glyphs are placed when `kept` and the output kept from
    /// an earlier frame still matches: no glyph was dropped or placed again. Returns whether it
    /// bound fully.
    fn bind_text(
        &self,
        ctx: &FrameBindContext<'_>,
        bound: &mut BoundFrame,
        kept: bool,
    ) -> crate::Result<bool> {
        let trace_started = crate::log_level_enabled(tracing::Level::TRACE).then(trace_now_ms);
        let mut expr = ctx.expr_context(self.node_id, "params");
        let mut evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &expr,
        })?;
        let (position_x, position_y) = evaluated.position;
        let drawable = DrawableTransform {
            position: evaluated.position,
            scale: evaluated.scale,
            rotation: evaluated.rotation,
            anchor: evaluated.anchor,
            opacity: evaluated.opacity,
        };
        let shutter = DeferredTransform {
            position: &self.params.position,
            scale: &self.params.scale,
            rotation: &self.params.rotation,
            anchor: &self.params.anchor,
        }
        .shutter(evaluated.motion_blur, drawable, self.node_id, &expr)?;
        let style = TextLayoutStyle::take(&mut evaluated);
        let mut content = self
            .content
            .is_none()
            .then(|| std::mem::take(&mut evaluated.content));
        // Undecorated text only needs the span colors.
        let (span_paints, span_styles) = match content.as_mut() {
            Some(content) if self.decorated => (Vec::new(), take_span_styles(content)),
            Some(content) => (take_span_paints(content), Vec::new()),
            None => Default::default(),
        };
        let font_features = self
            .font_features
            .is_none()
            .then(|| std::mem::take(&mut evaluated.font_features));
        let decorations = match &self.decorations {
            NodeDecorations::Constant(decorations) => std::borrow::Cow::Borrowed(decorations),
            NodeDecorations::Animated(params) => {
                std::borrow::Cow::Owned(params.eval(self.node_id, &expr)?)
            }
        };

        let mut text_system = text_system(ctx.media())?;
        load_font_families(
            &mut text_system,
            ctx.media(),
            &style,
            self.content.as_ref().or(content.as_ref()),
        )?;
        let mut state = self.state.lock().map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: format!("text node {} state lock was poisoned", self.node_id.0),
            })
        })?;
        let state = &mut *state;

        let shape_key = TextShapeKey {
            style,
            content,
            font_features,
            font_generation: text_system.font_generation(),
        };
        let mut layout_ms = 0.0;
        if state
            .shaped
            .as_ref()
            .is_none_or(|(key, _)| *key != shape_key)
        {
            let layout_started = trace_started.map(|_| trace_now_ms());
            let content = match (&shape_key.content, &self.content) {
                (Some(content), _) | (None, Some(content)) => content,
                (None, None) => unreachable!("content is constant or evaluated"),
            };
            let font_features = match (&shape_key.font_features, &self.font_features) {
                (Some(features), _) | (None, Some(features)) => features,
                (None, None) => unreachable!("font features are constant or evaluated"),
            };
            let request = shape_key.style.request(content, font_features);
            state.shaped = Some((shape_key, text_system.shape(&request)));
            if let Some(started) = layout_started {
                layout_ms = trace_now_ms() - started;
            }
        }
        let Some((_, shaped)) = &state.shaped else {
            unreachable!("text was shaped above");
        };

        let height = shaped.measurement.height;
        let origin = [
            position_x as f32,
            match evaluated.alignment_vertical {
                TextAlignmentVertical::Top => position_y as f32,
                TextAlignmentVertical::Middle => position_y as f32 - height * 0.5,
                TextAlignmentVertical::Bottom => position_y as f32 - height,
            },
        ];
        let layout = shaped.place(origin, [1.0; 4]);
        let spans: &[TextSpan] = match &self.content {
            Some(TextContent::Spans(spans)) => spans,
            Some(TextContent::Text(_)) => &[],
            None => &span_styles,
        };
        let span_color = |span: usize| match &self.content {
            None if !self.decorated => span_paints.get(span).and_then(Option::as_ref),
            _ => spans.get(span).and_then(|span| span.color.as_ref()),
        };
        // A span's own gradient spans its box under `TextPaintSpace::Span`, and other paints
        // the whole text.
        let text_paint = |paint: &Paint, span: Option<usize>| {
            let mut gpu = paint.to_gpu([255, 255, 255, 255]);
            gpu.paint_supersample = u32::from(evaluated.paint_supersample);
            let own_box = span
                .filter(|_| {
                    evaluated.paint_space == TextPaintSpace::Span
                        && !matches!(paint, Paint::SolidColor(..))
                })
                .and_then(|span| shaped.span_boxes.get(span).copied().flatten());
            let bounds = own_box.map_or(layout.bounds, |[x, y, width, height]| {
                [origin[0] + x, origin[1] + y, width, height]
            });
            GpuTextPaint { paint: gpu, bounds }
        };
        state.decorations.clear();
        // The paints of the decorations with their own and of the strokes, which follow the span
        // paints, each with the half width of its stroke.
        let mut decoration_paints = Vec::new();
        let mut before_glyphs = 0;
        let mut dropped_decorations = 0;
        state.strokes.clear();
        state.half_widths.clear();
        if self.decorated {
            // Spans without a color of their own share the first one's paint, so a line
            // runs along them as one rect.
            let plain = spans
                .iter()
                .position(|span| span.color.is_none())
                .unwrap_or(0) as u32;
            before_glyphs = decorate(
                shaped,
                &decorations,
                spans,
                origin,
                |span| match spans.get(span) {
                    Some(TextSpan { color: Some(_), .. }) => span as u32,
                    _ => plain,
                },
                |paint, span| {
                    decoration_paints.push((paint, span, 0.0));
                    (self.paint_count + decoration_paints.len() - 1) as u32
                },
                &mut state.decorations,
            );
            dropped_decorations = state.decorations.len().saturating_sub(self.max_glyphs);
            state.decorations.truncate(self.max_glyphs);
            before_glyphs = before_glyphs.min(state.decorations.len());
            if self.stroked {
                span_strokes(
                    decorations.stroke.as_ref(),
                    spans,
                    shaped,
                    |stroke, span, half_width| {
                        decoration_paints.push((&stroke.paint, span, half_width));
                        (self.paint_count + decoration_paints.len() - 1) as u32
                    },
                    &mut state.strokes,
                );
            }
            if state.strokes.iter().any(Option::is_some) {
                state.half_widths.extend(
                    state
                        .strokes
                        .iter()
                        .map(|stroke| stroke.map_or(0.0, |stroke| stroke.half_width)),
                );
            }
        }
        let mode = match evaluated.render_mode {
            TextRenderMode::Raster => lumen_text::TextRenderMode::Raster,
            TextRenderMode::Msdf => {
                if !lumen_text::MSDF_SUPPORTED {
                    static WARN: std::sync::Once = std::sync::Once::new();
                    WARN.call_once(|| tracing::warn!(target: "lumen_text", "MSDF support is disabled; using raster text"));
                }
                lumen_text::TextRenderMode::Msdf
            }
            TextRenderMode::Auto => lumen_text::TextRenderMode::Auto,
        };
        let atlas_started = trace_now_ms();
        let mut atlas = bound
            .text_atlas
            .as_ref()
            .expect("frame atlas")
            .lock()
            .map_err(|_| RenderError::Gpu {
                details: "text atlas lock was poisoned".to_string(),
            })?;
        let mut prepared = text_system.prepare_stroked_glyphs(
            &mut atlas,
            &layout,
            mode,
            self.max_glyphs - state.decorations.len(),
            &state.half_widths,
            &mut state.overfull,
        );
        let atlas_size = atlas.size();
        drop(atlas);
        drop(text_system);
        let atlas_ms = trace_now_ms() - atlas_started;
        if mode != lumen_text::TextRenderMode::Raster {
            bound.text_generation_count += prepared.uploads.len();
            bound.text_generation_ms += atlas_ms;
        }

        let dropped_glyphs = prepared.dropped_glyphs + dropped_decorations;
        let dropping = dropped_glyphs > 0;
        bound.text_dropped_glyphs |= dropping;
        if dropping != state.dropping_glyphs {
            if dropping {
                tracing::warn!(
                    target: "lumen_text",
                    node_id = self.node_id.0,
                    dropped_glyphs,
                    max_glyphs = self.max_glyphs,
                    atlas_size,
                    "text glyphs dropped: too many glyphs or glyph atlas full"
                );
            } else {
                tracing::info!(
                    target: "lumen_text",
                    node_id = self.node_id.0,
                    "text glyphs no longer dropped"
                );
            }
            state.dropping_glyphs = dropping;
        }

        let upload_count = prepared.uploads.len();
        if kept && !dropping && upload_count == 0 {
            return Ok(false);
        }
        state.paints.clear();
        for span in 0..self.paint_count {
            let own = span_color(span);
            state.paints.push(text_paint(
                own.unwrap_or(&evaluated.color),
                own.and(Some(span)),
            ));
        }
        for (paint, span, half_width) in decoration_paints {
            let mut paint = text_paint(paint, span);
            paint.paint.stroke = half_width.to_bits();
            state.paints.push(paint);
        }
        for upload in prepared.uploads {
            let [width, height] = upload.size;
            bound.write_texture_rgba8_region(
                self.atlas_texture,
                upload.pixels,
                [upload.origin[0], upload.origin[1], 0],
                lumen_gpu::Size::new(width, height),
                width * 4,
                height,
            );
        }
        if self.paint_count > 1 {
            for (instance, &source) in prepared.instances.iter_mut().zip(&prepared.sources) {
                instance.set_paint(layout.glyphs[source].item.span);
            }
        }
        let mut ink_bounds = self.glyph.apply(
            self.node_id,
            &layout,
            &mut prepared.instances,
            &prepared.sources,
            &mut expr,
        )?;
        // A stroke takes its glyph's transform and fade. Outside strokes are drawn under the
        // glyphs, and centred ones over them.
        state.under_strokes.clear();
        state.over_strokes.clear();
        for (mut instance, &glyph) in prepared.strokes.into_iter().zip(&prepared.stroke_glyphs) {
            let fill = &prepared.instances[glyph];
            let span = layout.glyphs[prepared.sources[glyph]].item.span as usize;
            let Some(stroke) = state.strokes[span] else {
                continue;
            };
            instance.set_paint(stroke.paint);
            instance.opacity = fill.opacity;
            if fill.flags & lumen_text::GLYPH_TRANSFORMED != 0 {
                instance.flags |= lumen_text::GLYPH_TRANSFORMED;
                instance.linear = fill.linear;
                instance.offset = fill.offset;
                let [x0, y0, x1, y1] = transformed_bounds(&instance);
                ink_bounds = Some(ink_bounds.map_or([x0, y0, x1, y1], |bounds| {
                    [
                        bounds[0].min(x0),
                        bounds[1].min(y0),
                        bounds[2].max(x1),
                        bounds[3].max(y1),
                    ]
                }));
            }
            if stroke.over {
                state.over_strokes.push(instance);
            } else {
                state.under_strokes.push(instance);
            }
        }
        // A faded glyph's lower layer reads its partner's coverage, so the pair fades as one,
        // and the strokes of a fading cluster fade as one like its fills.
        if prepared.instances.iter().any(|fill| fill.opacity < 1.0) {
            let fills = before_glyphs + state.under_strokes.len();
            let overs = fills + prepared.instances.len();
            let mut stroke_at = vec![None; prepared.instances.len()];
            let (mut under, mut over) = (0, 0);
            for &glyph in &prepared.stroke_glyphs {
                let span = layout.glyphs[prepared.sources[glyph]].item.span as usize;
                let Some(stroke) = state.strokes[span] else {
                    continue;
                };
                let count = if stroke.over { &mut over } else { &mut under };
                stroke_at[glyph] = Some((stroke.over, *count));
                *count += 1;
            }
            let mut index = 0;
            while index < prepared.instances.len() {
                let lead = index;
                index += 1
                    + (prepared.instances[lead].flags >> lumen_text::GLYPH_CLUSTER_SHIFT) as usize;
                if prepared.instances[lead].opacity >= 1.0 {
                    continue;
                }
                let mut strokes = stroke_at[lead..index].iter().flatten();
                let Some(&(over, first)) = strokes.next() else {
                    continue;
                };
                let list = if over {
                    &mut state.over_strokes
                } else {
                    &mut state.under_strokes
                };
                for &(_, at) in strokes.filter(|(other, _)| *other == over) {
                    list[at].flags |= lumen_text::GLYPH_CLUSTER_FOLLOWER;
                    list[first].flags += 1 << lumen_text::GLYPH_CLUSTER_SHIFT;
                }
                let (lower, partner) = if over {
                    (&mut prepared.instances[lead], overs + first)
                } else {
                    (&mut list[first], fills + lead)
                };
                lower.flags |= lumen_text::GLYPH_PARTNER;
                lower.partner = partner as u32;
            }
        }
        let stroke_count = state.under_strokes.len() + state.over_strokes.len();
        let globals = lumen_text::GpuTextGlobals {
            target_size: [self.layer_size.width as f32, self.layer_size.height as f32],
            px_range: lumen_text::MSDF_PX_RANGE,
            glyph_count: (state.decorations.len() + prepared.instances.len() + stroke_count) as u32,
        };
        bound.write_buffer(self.globals_buffer, 0, bytemuck::bytes_of(&globals));
        bound.write_buffer(self.paints_buffer, 0, bytemuck::cast_slice(&state.paints));
        let [x, y, width, height] = layout.bounds;
        let (mut min, mut max) =
            ink_bounds.map_or(([x, y], [x + width, y + height]), |[x0, y0, x1, y1]| {
                (
                    [x0.min(x), y0.min(y)],
                    [x1.max(x + width), y1.max(y + height)],
                )
            });
        for decoration in &state.decorations {
            let [x0, y0, x1, y1] = decoration.rect_bounds();
            min = [min[0].min(x0), min[1].min(y0)];
            max = [max[0].max(x1), max[1].max(y1)];
        }
        // Strokes reach past the glyphs, and a pixel further when antialiased.
        let reach = state
            .strokes
            .iter()
            .flatten()
            .map(|stroke| lumen_text::stroke_reach(stroke.half_width) + 1.0)
            .fold(0.0, f32::max);
        min = min.map(|bound| bound - reach);
        max = max.map(|bound| bound + reach);
        let motion = DrawableMotion::new(shutter.as_ref(), min, max);
        let canvas = [self.size.width as f32, self.size.height as f32];
        let layer = [self.layer_size.width as f32, self.layer_size.height as f32];
        let style = TextStyleUniforms {
            transform: motion.layer_transform(&drawable, min, max, canvas, layer),
            motion: motion.to_gpu(),
        };
        bound.write_buffer(self.style_buffer, 0, bytemuck::bytes_of(&style));
        // Highlights and underlines are drawn under the glyphs and their strokes, and
        // strikethroughs over them.
        let (under, over) = state.decorations.split_at(before_glyphs);
        let mut offset = 0;
        for instances in [
            under,
            &state.under_strokes,
            &prepared.instances,
            &state.over_strokes,
            over,
        ] {
            if !instances.is_empty() {
                bound.write_buffer(
                    self.instances_buffer,
                    (offset * std::mem::size_of::<lumen_text::GpuGlyphInstance>()) as u64,
                    bytemuck::cast_slice(instances),
                );
            }
            offset += instances.len();
        }

        if let Some(started) = trace_started {
            trace_text_bind(
                self.node_id,
                upload_count,
                prepared.instances.len() + stroke_count + state.decorations.len(),
                layout_ms,
                atlas_ms,
                trace_now_ms() - started,
            );
        }
        Ok(true)
    }
}

/// The per-glyph params, which are evaluated for each glyph rather than with the others.
#[derive(Debug, Clone)]
pub(crate) struct GlyphParams {
    offset: Deferred<(f64, f64)>,
    scale: Deferred<(f64, f64)>,
    rotation: Deferred<f64>,
    opacity: Deferred<f64>,
}

impl GlyphParams {
    /// Moves the per-glyph params out of `params`, leaving their defaults.
    pub(crate) fn take(params: &mut TextParamsDelegate) -> Self {
        let defaults = TextParamsDelegate::default();
        Self {
            offset: std::mem::replace(&mut params.glyph_offset, defaults.glyph_offset),
            scale: std::mem::replace(&mut params.glyph_scale, defaults.glyph_scale),
            rotation: std::mem::replace(&mut params.glyph_rotation, defaults.glyph_rotation),
            opacity: std::mem::replace(&mut params.glyph_opacity, defaults.glyph_opacity),
        }
    }

    /// Whether `params` transform or fade any glyph.
    pub(crate) fn any(params: &TextParamsDelegate) -> bool {
        !Self::identity(
            &params.glyph_offset,
            &params.glyph_scale,
            &params.glyph_rotation,
            &params.glyph_opacity,
        )
    }

    fn is_identity(&self) -> bool {
        Self::identity(&self.offset, &self.scale, &self.rotation, &self.opacity)
    }

    fn identity(
        offset: &Deferred<(f64, f64)>,
        scale: &Deferred<(f64, f64)>,
        rotation: &Deferred<f64>,
        opacity: &Deferred<f64>,
    ) -> bool {
        matches!(offset, Deferred::Value(offset) if *offset == (0.0, 0.0))
            && matches!(scale, Deferred::Value(scale) if *scale == (1.0, 1.0))
            && matches!(rotation, Deferred::Value(rotation) if *rotation == 0.0)
            && matches!(opacity, Deferred::Value(opacity) if *opacity >= 1.0)
    }

    /// Transforms and fades each glyph instance by its params, evaluated with `expr` scoped
    /// to the instance's glyph. Returns the box `[min_x, min_y, max_x, max_y]` that the
    /// transformed glyphs cover before the text's own transform, if any were transformed.
    fn apply(
        &self,
        node_id: NodeId,
        layout: &lumen_text::TextLayout,
        instances: &mut [lumen_text::GpuGlyphInstance],
        sources: &[usize],
        expr: &mut ExpressionContext<'_>,
    ) -> crate::Result<Option<[f32; 4]>> {
        if self.is_identity() {
            return Ok(None);
        }
        let mut offset = PerGlyph::new(&self.offset, "glyph_offset");
        let mut scale = PerGlyph::new(&self.scale, "glyph_scale");
        let mut rotation = PerGlyph::new(&self.rotation, "glyph_rotation");
        let mut opacity = PerGlyph::new(&self.opacity, "glyph_opacity");
        let mut bounds: Option<[f32; 4]> = None;
        // The instance and cluster that the instances so far fade as one.
        let mut fading: Option<(usize, (u32, u32))> = None;
        for (index, &source) in sources.iter().enumerate() {
            let glyph = &layout.glyphs[source];
            expr.glyph = Some(GlyphScope {
                item: glyph.item,
                counts: layout.counts,
            });
            let [x, y] = glyph.pivot.map(f64::from);
            let transform = DrawableTransform {
                position: (x, y),
                scale: scale.eval(node_id, expr)?,
                rotation: rotation.eval(node_id, expr)?,
                anchor: (0.0, 0.0),
                opacity: opacity.eval(node_id, expr)?,
            }
            .to_gpu();
            let (offset_x, offset_y) = offset.eval(node_id, expr)?;
            let instance = &mut instances[index];
            instance.opacity *= transform.opacity;
            if transform.flags != 0 || (offset_x, offset_y) != (0.0, 0.0) {
                instance.flags |= lumen_text::GLYPH_TRANSFORMED;
                instance.linear = transform.linear;
                instance.offset = [
                    transform.offset[0] + offset_x as f32,
                    transform.offset[1] + offset_y as f32,
                ];
            }
            // Fading each instance of a cluster separately would count their overlap more than
            // once, so the cluster's first instance draws the others, composited at full alpha.
            // The lead draws them with its own transform and colour, so only instances on the
            // same line that match those join it.
            let key = (glyph.item.glyph, glyph.item.line);
            if transform.opacity < 1.0 {
                match fading {
                    Some((lead, lead_key))
                        if lead_key == key && same_draw(&instances[lead], &instances[index]) =>
                    {
                        instances[lead].flags += 1 << lumen_text::GLYPH_CLUSTER_SHIFT;
                        instances[index].flags |= lumen_text::GLYPH_CLUSTER_FOLLOWER;
                    }
                    _ => fading = Some((index, key)),
                }
            } else {
                fading = None;
            }
            let instance = &instances[index];
            if instance.flags & lumen_text::GLYPH_TRANSFORMED == 0 {
                continue;
            }
            let [x0, y0, x1, y1] = transformed_bounds(instance);
            bounds = Some(bounds.map_or([x0, y0, x1, y1], |bounds| {
                [
                    bounds[0].min(x0),
                    bounds[1].min(y0),
                    bounds[2].max(x1),
                    bounds[3].max(y1),
                ]
            }));
        }
        expr.glyph = None;
        Ok(bounds)
    }
}

/// The box `[min_x, min_y, max_x, max_y]` that a transformed glyph instance's rect covers
/// under its own transform.
fn transformed_bounds(instance: &lumen_text::GpuGlyphInstance) -> [f32; 4] {
    let [x, y, width, height] = instance.rect;
    let [column_x, column_y] = instance.linear;
    [
        [x, y],
        [x + width, y],
        [x, y + height],
        [x + width, y + height],
    ]
    .map(|[x, y]| {
        [
            column_x[0] * x + column_y[0] * y + instance.offset[0],
            column_x[1] * x + column_y[1] * y + instance.offset[1],
        ]
    })
    .into_iter()
    .fold(
        [
            f32::INFINITY,
            f32::INFINITY,
            f32::NEG_INFINITY,
            f32::NEG_INFINITY,
        ],
        |[x0, y0, x1, y1], [x, y]| [x0.min(x), y0.min(y), x1.max(x), y1.max(y)],
    )
}

/// Whether two glyph instances draw with the same transform, opacity and paint.
fn same_draw(a: &lumen_text::GpuGlyphInstance, b: &lumen_text::GpuGlyphInstance) -> bool {
    let transformed =
        |glyph: &lumen_text::GpuGlyphInstance| glyph.flags & lumen_text::GLYPH_TRANSFORMED != 0;
    transformed(a) == transformed(b)
        && (!transformed(a) || (a.linear == b.linear && a.offset == b.offset))
        && a.opacity == b.opacity
        && a.paint() == b.paint()
}

/// A per-glyph param, evaluated again only when an index it reads changes. A constant, or
/// an expression that reads no index, is evaluated once.
struct PerGlyph<'a, T> {
    param: &'a Deferred<T>,
    name: &'static str,
    /// Whether the param reads the glyph, word, line and span index.
    reads: [bool; 4],
    last: Option<([u32; 4], T)>,
}

impl<'a, T: DeferredValue + Copy> PerGlyph<'a, T> {
    fn new(param: &'a Deferred<T>, name: &'static str) -> Self {
        let mut reads = [false; 4];
        if let Deferred::Expr(expression) = param {
            crate::expr::functions::visit(expression.ast(), &mut |node| match node {
                ExprNode::Global(GlobalVar::Glyph) => reads[0] = true,
                ExprNode::Global(GlobalVar::Word) => reads[1] = true,
                ExprNode::Global(GlobalVar::Line) => reads[2] = true,
                ExprNode::Global(GlobalVar::Span) => reads[3] = true,
                _ => {}
            });
        }
        Self {
            param,
            name,
            reads,
            last: None,
        }
    }

    fn eval(&mut self, node_id: NodeId, expr: &ExpressionContext<'_>) -> crate::Result<T> {
        if let Deferred::Value(value) = self.param {
            return Ok(*value);
        }
        let item = expr.glyph.map(|scope| scope.item).unwrap_or_default();
        let key = [item.glyph, item.word, item.line, item.span];
        let key: [u32; 4] =
            std::array::from_fn(|index| if self.reads[index] { key[index] } else { 0 });
        if let Some((last, value)) = self.last
            && last == key
        {
            return Ok(value);
        }
        let value = self.param.eval(node_id, self.name, expr)?;
        self.last = Some((key, value));
        Ok(value)
    }
}

#[derive(Debug, Default)]
pub(crate) struct TextState {
    shaped: Option<(TextShapeKey, lumen_text::ShapedText)>,
    paints: Vec<GpuTextPaint>,
    /// Rect instances of the decorations: those under the glyphs, then those over them.
    decorations: Vec<lumen_text::GpuGlyphInstance>,
    dropping_glyphs: bool,
    overfull: Option<u64>,
    /// The stroke of each span, kept across frames like the buffers below.
    strokes: Vec<Option<SpanStroke>>,
    /// The half width of each span's stroke, or none when no span draws one.
    half_widths: Vec<f32>,
    /// Stroke instances drawn under the glyphs, and those drawn over them.
    under_strokes: Vec<lumen_text::GpuGlyphInstance>,
    over_strokes: Vec<lumen_text::GpuGlyphInstance>,
}

#[derive(Debug, PartialEq)]
struct TextShapeKey {
    style: TextLayoutStyle,
    /// The content, or `None` when it is constant.
    content: Option<TextContent>,
    /// The font features, or `None` when they are constant.
    font_features: Option<FontFeatures>,
    font_generation: u64,
}

/// What a text's layout reads besides its content: the style its spans inherit, and how its
/// lines wrap, space and align.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct TextLayoutStyle {
    pub(crate) font_family: String,
    pub(crate) font_size: f64,
    pub(crate) font_weight: i64,
    pub(crate) font_style: TextFontStyle,
    pub(crate) letter_spacing: f64,
    pub(crate) max_width: f64,
    pub(crate) line_height: f64,
    pub(crate) alignment_horizontal: TextAlignmentHorizontal,
    pub(crate) text_transform: TextTransform,
    pub(crate) paragraph_spacing: f64,
    pub(crate) limits: TextLimits,
}

/// How a text's lines are limited and fitted.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub(crate) struct TextLimits {
    pub(crate) max_lines: Option<i64>,
    pub(crate) max_height: Option<f64>,
    pub(crate) overflow: TextOverflow,
    pub(crate) fit: TextFit,
    pub(crate) min_font_size: f64,
}

impl TextLimits {
    pub(crate) fn eval(
        params: &TextParamsDelegate,
        node_id: NodeId,
        expr: &ExpressionContext<'_>,
    ) -> crate::Result<Self> {
        use crate::node::DelegateEvaluable;
        let ctx = |property_path| DelegateEvalContext {
            node_id,
            property_path,
            expr,
        };
        Ok(Self {
            max_lines: DelegateEvaluable::eval(&params.max_lines, &ctx("max_lines"))?,
            max_height: DelegateEvaluable::eval(&params.max_height, &ctx("max_height"))?,
            overflow: params.overflow.eval(&ctx("overflow"))?,
            fit: params.fit.eval(&ctx("fit"))?,
            min_font_size: params.min_font_size.eval(node_id, "min_font_size", expr)?,
        })
    }
}

impl TextLayoutStyle {
    pub(crate) fn take(params: &mut TextParams) -> Self {
        Self {
            font_family: std::mem::take(&mut params.font_family),
            font_size: params.font_size,
            font_weight: params.font_weight,
            font_style: params.font_style,
            letter_spacing: params.letter_spacing,
            max_width: params.max_width,
            line_height: params.line_height,
            alignment_horizontal: params.alignment_horizontal,
            limits: TextLimits {
                max_lines: params.max_lines,
                max_height: params.max_height,
                overflow: params.overflow,
                fit: params.fit,
                min_font_size: params.min_font_size,
            },
            text_transform: params.text_transform,
            paragraph_spacing: params.paragraph_spacing,
        }
    }

    /// A `max_width` or `line_height` of 0 or less is automatic.
    pub(crate) fn request(
        &self,
        content: &TextContent,
        font_features: &FontFeatures,
    ) -> lumen_text::TextLayoutRequest {
        let spans = match content {
            TextContent::Text(text) => vec![self.span(text, None, font_features)],
            TextContent::Spans(spans) => spans
                .iter()
                .map(|span| self.span(&span.text, Some(span), font_features))
                .collect(),
        };
        let mut request = lumen_text::TextLayoutRequest::from_spans(spans);
        request.max_width = (self.max_width > 0.0).then_some(self.max_width as f32);
        request.line_height = (self.line_height > 0.0).then_some(self.line_height as f32);
        request.paragraph_spacing = self.paragraph_spacing.max(0.0) as f32;
        request.align = match self.alignment_horizontal {
            TextAlignmentHorizontal::Center => lumen_text::TextAlign::Center,
            TextAlignmentHorizontal::Right => lumen_text::TextAlign::Right,
            TextAlignmentHorizontal::Justify => lumen_text::TextAlign::Justified,
            TextAlignmentHorizontal::Left => lumen_text::TextAlign::Left,
        };
        let limits = &self.limits;
        request.max_lines = limits
            .max_lines
            .map(|lines| lines.clamp(0, u32::MAX.into()) as u32);
        request.max_height = limits.max_height.map(|height| height.max(0.0) as f32);
        request.overflow = match limits.overflow {
            TextOverflow::Clip => lumen_text::TextOverflow::Clip,
            TextOverflow::Ellipsis => lumen_text::TextOverflow::Ellipsis,
        };
        request.min_fit_scale = (limits.fit == TextFit::Shrink)
            .then(|| (limits.min_font_size / self.font_size.max(1.0)) as f32);
        request
    }

    fn span(
        &self,
        text: &str,
        span: Option<&TextSpan>,
        font_features: &FontFeatures,
    ) -> lumen_text::TextSpan {
        let font_style = span
            .and_then(|span| span.font_style)
            .unwrap_or(self.font_style);
        lumen_text::TextSpan {
            text: text.to_string(),
            font_family: span
                .and_then(|span| span.font_family.as_deref())
                .unwrap_or(&self.font_family)
                .to_string(),
            font_size: span
                .and_then(|span| span.font_size)
                .unwrap_or(self.font_size) as f32,
            font_weight: span
                .and_then(|span| span.font_weight)
                .unwrap_or(self.font_weight)
                .clamp(1, 1000) as u16,
            font_style: match font_style {
                TextFontStyle::Italic => lumen_text::TextFontStyle::Italic,
                TextFontStyle::Oblique => lumen_text::TextFontStyle::Oblique,
                TextFontStyle::Normal => lumen_text::TextFontStyle::Normal,
            },
            letter_spacing: span
                .and_then(|span| span.letter_spacing)
                .unwrap_or(self.letter_spacing) as f32,
            font_features: font_features.merged(span.and_then(|span| span.font_features.as_ref())),
            transform: span
                .and_then(|span| span.text_transform)
                .unwrap_or(self.text_transform)
                .into(),
            baseline_shift: span.and_then(|span| span.baseline_shift).unwrap_or(0.0) as f32,
        }
    }
}

/// Takes what does not affect the layout out of `content`'s spans: their colors, returned in
/// order, and their decorations.
fn take_span_paints(content: &mut TextContent) -> Vec<Option<Paint>> {
    let TextContent::Spans(spans) = content else {
        return Vec::new();
    };
    spans
        .iter_mut()
        .map(|span| {
            span.underline = Override::Inherit;
            span.strikethrough = Override::Inherit;
            span.highlight = Override::Inherit;
            span.stroke = Override::Inherit;
            span.color.take()
        })
        .collect()
}

/// Takes what does not affect the layout out of `content`'s spans, returning spans that hold
/// only their colors, decorations and strokes.
fn take_span_styles(content: &mut TextContent) -> Vec<TextSpan> {
    let TextContent::Spans(spans) = content else {
        return Vec::new();
    };
    spans
        .iter_mut()
        .map(|span| TextSpan {
            color: span.color.take(),
            underline: std::mem::take(&mut span.underline),
            strikethrough: std::mem::take(&mut span.strikethrough),
            highlight: std::mem::take(&mut span.highlight),
            stroke: std::mem::take(&mut span.stroke),
            ..TextSpan::default()
        })
        .collect()
}

/// The node's decorations and stroke, which spans that leave theirs unset take.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct TextDecorations {
    pub(crate) underline: Option<TextLine>,
    pub(crate) strikethrough: Option<TextLine>,
    pub(crate) highlight: Option<TextHighlight>,
    pub(crate) stroke: Option<TextStroke>,
}

/// The node's decoration and stroke params, moved out of the others.
#[derive(Debug, Clone)]
pub(crate) struct DecorationParams {
    underline: Option<TextLineDelegate>,
    strikethrough: Option<TextLineDelegate>,
    highlight: Option<TextHighlightDelegate>,
    stroke: Option<TextStrokeDelegate>,
}

impl DecorationParams {
    /// Moves the node's decorations and stroke out of `params`, leaving them unset.
    pub(crate) fn take(params: &mut TextParamsDelegate) -> Self {
        Self {
            underline: params.underline.take(),
            strikethrough: params.strikethrough.take(),
            highlight: params.highlight.take(),
            stroke: params.stroke.take(),
        }
    }

    pub(crate) fn has_expressions(&self) -> bool {
        use crate::node::DelegateEvaluable;
        DelegateEvaluable::has_expressions(&self.underline)
            || DelegateEvaluable::has_expressions(&self.strikethrough)
            || DelegateEvaluable::has_expressions(&self.highlight)
            || DelegateEvaluable::has_expressions(&self.stroke)
    }

    pub(crate) fn eval(
        &self,
        node_id: NodeId,
        expr: &ExpressionContext<'_>,
    ) -> crate::Result<TextDecorations> {
        use crate::node::DelegateEvaluable;
        let ctx = |property_path| DelegateEvalContext {
            node_id,
            property_path,
            expr,
        };
        Ok(TextDecorations {
            underline: DelegateEvaluable::eval(&self.underline, &ctx("underline"))?,
            strikethrough: DelegateEvaluable::eval(&self.strikethrough, &ctx("strikethrough"))?,
            highlight: DelegateEvaluable::eval(&self.highlight, &ctx("highlight"))?,
            stroke: DelegateEvaluable::eval(&self.stroke, &ctx("stroke"))?,
        })
    }
}

/// The node's decorations: evaluated at compile when constant, else each frame.
#[derive(Debug)]
#[allow(
    clippy::large_enum_variant,
    reason = "each compiled text node holds one, so its size does not matter"
)]
pub(crate) enum NodeDecorations {
    Constant(TextDecorations),
    Animated(DecorationParams),
}

/// Appends the rect instances of the decorations of `shaped`, placed at `origin`, to
/// `instances`: highlights, then underlines, then strikethroughs. Returns how many come before
/// the strikethroughs, which are drawn over the glyphs.
///
/// Consecutive spans that take the node's decoration share one run, and a span with its own is
/// a run alone. A line without its own paint takes the paint index `text_paint` gives each
/// span; `push_paint` adds a decoration's own paint, with the span whose box it spans if it is
/// the span's own, and returns its index.
pub(crate) fn decorate<'a>(
    shaped: &lumen_text::ShapedText,
    node: &'a TextDecorations,
    spans: &'a [TextSpan],
    origin: [f32; 2],
    text_paint: impl Fn(usize) -> u32,
    mut push_paint: impl FnMut(&'a Paint, Option<usize>) -> u32,
    instances: &mut Vec<lumen_text::GpuGlyphInstance>,
) -> usize {
    let span_count = shaped.counts.span as usize;
    // A span's own decoration moves with its baseline shift; the node's stays on the line.
    let draw = |instances: &mut Vec<_>,
                kind,
                spans,
                own: bool,
                extent: (f64, f64),
                paint: &dyn Fn(usize) -> u32| {
        let decoration = lumen_text::TextDecoration {
            kind,
            spans,
            shifted: own,
            extent: [extent.0 as f32, extent.1 as f32],
        };
        shaped.decorate(&decoration, origin, paint, instances);
    };
    let mut node_paint = None;
    for_each_run(
        node.highlight.as_ref(),
        spans,
        span_count,
        |span| &span.highlight,
        |range, highlight, own| {
            let index = match own {
                Some(span) => push_paint(&highlight.paint, Some(span)),
                None => *node_paint.get_or_insert_with(|| push_paint(&highlight.paint, None)),
            };
            let kind = lumen_text::TextDecorationKind::Highlight {
                padding: [highlight.padding.0 as f32, highlight.padding.1 as f32],
                radius: highlight.radius as f32,
            };
            draw(
                instances,
                kind,
                range,
                own.is_some(),
                highlight.extent,
                &|_| index,
            );
        },
    );
    let mut before_glyphs = 0;
    for underline in [true, false] {
        if !underline {
            before_glyphs = instances.len();
        }
        let node_line = if underline {
            &node.underline
        } else {
            &node.strikethrough
        };
        let mut node_paint = None;
        for_each_run(
            node_line.as_ref(),
            spans,
            span_count,
            |span| {
                if underline {
                    &span.underline
                } else {
                    &span.strikethrough
                }
            },
            |range, line, own| {
                let decoration = lumen_text::TextLineDecoration {
                    style: match line.style {
                        TextLineStyle::Solid => lumen_text::TextLineStyle::Solid,
                        TextLineStyle::Double => lumen_text::TextLineStyle::Double,
                        TextLineStyle::Dotted => lumen_text::TextLineStyle::Dotted,
                        TextLineStyle::Dashed => lumen_text::TextLineStyle::Dashed,
                        TextLineStyle::Wavy => lumen_text::TextLineStyle::Wavy,
                    },
                    thickness: line.thickness.map(|thickness| thickness as f32),
                    offset: line.offset.map(|offset| offset as f32),
                };
                let kind = if underline {
                    lumen_text::TextDecorationKind::Underline(decoration)
                } else {
                    lumen_text::TextDecorationKind::Strikethrough(decoration)
                };
                let index = line.paint.as_ref().map(|paint| match own {
                    Some(span) => push_paint(paint, Some(span)),
                    None => *node_paint.get_or_insert_with(|| push_paint(paint, None)),
                });
                match index {
                    Some(index) => {
                        draw(instances, kind, range, own.is_some(), line.extent, &|_| {
                            index
                        })
                    }
                    None => draw(
                        instances,
                        kind,
                        range,
                        own.is_some(),
                        line.extent,
                        &text_paint,
                    ),
                }
            },
        );
    }
    before_glyphs
}

/// The number of paints that `params`' decorations with their own and strokes add, or `None`
/// when neither the node nor any span has a decoration or stroke.
pub(crate) fn decoration_paints(params: &TextParamsDelegate) -> Option<usize> {
    let mut lines: Vec<&TextLineDelegate> = params
        .underline
        .iter()
        .chain(&params.strikethrough)
        .collect();
    // Highlights and strokes each have a paint.
    let mut painted =
        usize::from(params.highlight.is_some()) + usize::from(params.stroke.is_some());
    if let TextContentDelegate::Spans(spans) = &params.content {
        for span in &spans.items {
            for line in [&span.underline, &span.strikethrough] {
                if let Override::Set(line) = line {
                    lines.push(line);
                }
            }
            painted += usize::from(matches!(span.highlight, Override::Set(_)))
                + usize::from(matches!(span.stroke, Override::Set(_)));
        }
    }
    (!lines.is_empty() || painted > 0)
        .then(|| painted + lines.iter().filter(|line| line.paint.is_some()).count())
}

/// Whether the node or any span of `params` has a stroke.
pub(crate) fn stroked(params: &TextParamsDelegate) -> bool {
    params.stroke.is_some()
        || matches!(&params.content, TextContentDelegate::Spans(spans)
            if spans.items.iter().any(|span| matches!(span.stroke, Override::Set(_))))
}

/// A span's stroke: how far it reaches to each side of the glyph edge, whether it is drawn
/// over the glyphs, and its paint index.
#[derive(Debug, Clone, Copy)]
pub(crate) struct SpanStroke {
    pub(crate) half_width: f32,
    over: bool,
    paint: u32,
}

/// Sets `strokes` to the stroke each of `shaped`'s spans draws, if any: the node's for spans
/// that leave theirs unset, each its own otherwise. Widths scale with the text's fit.
/// `push_paint` adds a stroke's paint, with the span whose box it spans if it is the span's own
/// and its half width, and returns its index.
pub(crate) fn span_strokes<'a>(
    node: Option<&'a TextStroke>,
    spans: &'a [TextSpan],
    shaped: &lumen_text::ShapedText,
    mut push_paint: impl FnMut(&'a TextStroke, Option<usize>, f32) -> u32,
    strokes: &mut Vec<Option<SpanStroke>>,
) {
    let span_count = shaped.counts.span as usize;
    strokes.clear();
    strokes.resize(span_count, None);
    let mut node_paint = None;
    for_each_run(
        node,
        spans,
        span_count,
        |span| &span.stroke,
        |range, stroke, own| {
            if !(stroke.width.is_finite() && stroke.width > 0.0) {
                return;
            }
            let half_width = stroke.half_width() as f32 * shaped.fit_scale;
            let paint = match own {
                Some(span) => push_paint(stroke, Some(span), half_width),
                None => *node_paint.get_or_insert_with(|| push_paint(stroke, None, half_width)),
            };
            strokes[range].fill(Some(SpanStroke {
                half_width,
                over: stroke.align == TextStrokeAlign::Center,
                paint,
            }));
        },
    );
}

/// Calls `run` with each run of the `span_count` spans that draws a decoration, its value and,
/// for a span's own, the span: consecutive spans that take the node's `inherited` value, and
/// each span with its own. `spans` holds the spans' decorations, which `own` reads; plain
/// string content has none, so takes the node's.
fn for_each_run<'a, T>(
    inherited: Option<&'a T>,
    spans: &'a [TextSpan],
    span_count: usize,
    own: impl Fn(&'a TextSpan) -> &'a Override<T>,
    mut run: impl FnMut(std::ops::Range<usize>, &'a T, Option<usize>),
) {
    let inherits = |span: usize| {
        spans
            .get(span)
            .is_none_or(|span| matches!(own(span), Override::Inherit))
    };
    let mut span = 0;
    while span < span_count {
        if inherits(span) {
            let start = span;
            while span < span_count && inherits(span) {
                span += 1;
            }
            if let Some(value) = inherited {
                run(start..span, value, None);
            }
            continue;
        }
        if let Override::Set(value) = own(&spans[span]) {
            run(span..span + 1, value, Some(span));
        }
        span += 1;
    }
}

/// Loads the families `style` and the spans of `content` name.
pub(crate) fn load_font_families(
    text_system: &mut lumen_text::TextSystem,
    media: Option<&dyn MediaStore>,
    style: &TextLayoutStyle,
    content: Option<&TextContent>,
) -> crate::Result<()> {
    load_font_family(text_system, media, &style.font_family)?;
    if let Some(TextContent::Spans(spans)) = content {
        for family in spans.iter().filter_map(|span| span.font_family.as_deref()) {
            load_font_family(text_system, media, family)?;
        }
    }
    Ok(())
}

/// The text system of `media`'s fonts, which rendering and text measurement share so both see
/// them. Do not hold the guard while evaluating expressions, which may measure text.
pub(crate) fn text_system<'a>(
    media: Option<&'a dyn MediaStore>,
) -> crate::Result<std::sync::MutexGuard<'a, lumen_text::TextSystem>> {
    static SHARED: LazyLock<TextSession> = LazyLock::new(TextSession::default);
    media
        .and_then(|store| store.text_session())
        .unwrap_or(&SHARED)
        .lock()
}

pub(crate) fn load_font_family(
    text_system: &mut lumen_text::TextSystem,
    media: Option<&dyn MediaStore>,
    font_family: &str,
) -> crate::Result<()> {
    if font_family.is_empty() {
        return Ok(());
    }
    let Some(store) = media else {
        return Ok(());
    };
    let current_id = store.font_resolver_id(font_family);
    if current_id.is_some() && text_system.family_resolver(font_family) == current_id.as_deref() {
        return Ok(());
    }
    let resolver = store.get_font_resolver(font_family);
    // An ID that no resolver comes with is remembered too, so later binds do not ask again.
    let Some(resolver_id) = resolver
        .as_ref()
        .map(|resolver| resolver.id().to_string())
        .or(current_id)
    else {
        return Ok(());
    };
    if text_system.family_resolver(font_family) == Some(resolver_id.as_str()) {
        return Ok(());
    }
    let files = match resolver {
        Some(resolver) => font_files(&*resolver)?,
        None => Vec::new(),
    };
    text_system
        .load_family(font_family, &resolver_id, files)
        .map_err(|_| {
            LumenError::Media(MediaError::Decode {
                media_source: font_family.to_string(),
                details: "its font data holds no readable face".to_string(),
            })
        })?;
    if !text_system.has_family(font_family) {
        tracing::warn!(
            target: "lumen_text",
            font_family,
            "no face of the font loaded for this family goes by its name; text falls back to other fonts"
        );
    }
    Ok(())
}

/// The data of `resolver`'s font files, shared with every text system still holding that resolver
/// ID's, so that jobs using one font hold and read it once.
fn font_files(resolver: &dyn FontResolver) -> crate::Result<Vec<Arc<Vec<u8>>>> {
    type Files = HashMap<String, Vec<Weak<Vec<u8>>>>;
    static FILES: LazyLock<Mutex<Files>> = LazyLock::new(Mutex::default);
    let lock = || {
        FILES.lock().map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: "font file cache lock was poisoned".to_string(),
            })
        })
    };
    if let Some(files) = lock()?
        .get(resolver.id())
        .and_then(|files| files.iter().map(Weak::upgrade).collect())
    {
        return Ok(files);
    }
    let files: Vec<_> = resolver
        .data()
        .map_err(LumenError::Media)?
        .into_iter()
        .map(Arc::new)
        .collect();
    let mut cache = lock()?;
    cache.retain(|_, files| files.iter().all(|file| file.strong_count() > 0));
    cache.insert(
        resolver.id().to_string(),
        files.iter().map(Arc::downgrade).collect(),
    );
    Ok(files)
}

fn trace_text_bind(
    node_id: NodeId,
    uploads: usize,
    glyph_count: usize,
    layout_ms: f64,
    atlas_ms: f64,
    total_ms: f64,
) {
    if uploads == 0 && total_ms < 1.0 {
        return;
    }
    static BIND_COUNT: AtomicU64 = AtomicU64::new(0);
    let bind_count = BIND_COUNT.fetch_add(1, Ordering::Relaxed) + 1;
    tracing::trace!(
        target: "lumen_text",
        bind = bind_count,
        node_id = node_id.0,
        atlas_uploads = uploads,
        glyphs = glyph_count,
        layout_ms,
        atlas_ms,
        bind_ms = total_ms,
        "text bind"
    );
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn trace_now_ms() -> f64 {
    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn trace_now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Default)]
    struct CountingStore {
        /// `None` serves no resolver under the ID it still gives.
        data: Option<Vec<u8>>,
        resolvers: AtomicU64,
        reads: Arc<AtomicU64>,
    }

    struct CountingResolver {
        id: &'static str,
        data: Vec<u8>,
        reads: Arc<AtomicU64>,
    }

    impl FontResolver for CountingResolver {
        fn id(&self) -> &str {
            self.id
        }

        fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(vec![self.data.clone()])
        }
    }

    impl MediaStore for CountingStore {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn crate::media::ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn crate::media::VideoFrameResolver>> {
            None
        }

        fn font_resolver_id(&self, family: &str) -> Option<String> {
            (family == "Known").then(|| "counting-store-test-font".to_string())
        }

        fn get_font_resolver(&self, _: &str) -> Option<Box<dyn FontResolver>> {
            self.resolvers.fetch_add(1, Ordering::Relaxed);
            let data = self.data.clone()?;
            Some(Box::new(CountingResolver {
                id: "counting-store-test-font",
                data,
                reads: Arc::clone(&self.reads),
            }))
        }
    }

    #[test]
    fn stores_are_asked_once_for_a_font_they_do_not_serve() {
        let store = CountingStore::default();
        let mut system = lumen_text::TextSystem::new();
        for _ in 0..3 {
            load_font_family(&mut system, Some(&store), "Known").unwrap();
        }
        assert_eq!(store.resolvers.load(Ordering::Relaxed), 1);
    }

    /// Serves one font under the default `font_resolver_id`, which gives no ID.
    #[derive(Debug, Default)]
    struct NoIdStore(Arc<AtomicU64>);

    impl MediaStore for NoIdStore {
        fn get_image_resolver(&self, _: &str) -> Option<Box<dyn crate::media::ImageResolver>> {
            None
        }

        fn get_video_resolver(&self, _: &str) -> Option<Box<dyn crate::media::VideoFrameResolver>> {
            None
        }

        fn get_font_resolver(&self, family: &str) -> Option<Box<dyn FontResolver>> {
            (family == "Known").then(|| {
                Box::new(CountingResolver {
                    id: "no-id-store-test-font",
                    data: lumen_text::ROBOTO_REGULAR_BYTES.to_vec(),
                    reads: Arc::clone(&self.0),
                }) as _
            })
        }
    }

    #[test]
    fn stores_without_resolver_ids_still_load_fonts_once() {
        let store = NoIdStore::default();
        let mut system = lumen_text::TextSystem::new();
        for _ in 0..3 {
            load_font_family(&mut system, Some(&store), "Known").unwrap();
            load_font_family(&mut system, Some(&store), "Unknown").unwrap();
        }
        assert_eq!(
            system.family_resolver("Known"),
            Some("no-id-store-test-font")
        );
        assert_eq!(system.family_resolver("Unknown"), None);
        assert_eq!(store.0.load(Ordering::Relaxed), 1);
    }

    #[test]
    fn text_systems_share_a_font_while_one_holds_it() {
        // Not Roboto, whose face would give way to the bundled one and not be held.
        let store = CountingStore {
            data: Some(
                include_bytes!("../../../../text/assets/inter/InterVariable-Latin.ttf").to_vec(),
            ),
            ..Default::default()
        };
        let reads = || store.reads.load(Ordering::Relaxed);
        let load = |system: &mut lumen_text::TextSystem| {
            load_font_family(system, Some(&store), "Known").unwrap();
        };

        let mut first = lumen_text::TextSystem::new();
        load(&mut first);
        load(&mut lumen_text::TextSystem::new());
        assert_eq!(reads(), 1);

        drop(first);
        load(&mut lumen_text::TextSystem::new());
        assert_eq!(reads(), 2);
    }
}
