use std::{
    borrow::Cow,
    collections::HashMap,
    sync::{Arc, LazyLock, Mutex, Weak},
};

use crate::error::{LumenError, MediaFailure, MediaReason, RenderError};
use crate::expr::{ExpressionContext, GlyphScope};
use crate::gpu::BoundFrame;
use crate::media::{
    AssetKind, FontResolver, MediaIdentity, MediaStore, TextSession, asset_id, resolve_as, sniff,
};
use crate::node::vector::{
    drawable::{DrawableMotion, DrawablePlacement, DrawablePlacementDelegate, DrawableTransform},
    paint::Paint,
};
use crate::node::{
    Deferred, DelegateEvalContext, DelegateEvaluable, JsonNode, NodeId, NodeParams, Override,
    ParamValue,
};

use super::text_content::{
    FontFeatures, SpanLayout, SpanStyle, SpanStyleDelegate, TextContent, TextContentDelegate,
    TextHighlight, TextLine, TextLineDelegate, TextStroke, TextStrokeAlign,
};
use super::text_record::{GpuTextPaint, TextStyleUniforms};

/// A param enum and the conversion to its `lumen_text` twin, which has the same variants.
macro_rules! text_enum {
    (
        $(#$attr:tt)*
        $name:ident => $twin:path {
            $($(#$variant_attr:tt)* $variant:ident = $value:tt,)*
        }
    ) => {
        #[derive(
            Debug,
            Clone,
            Copy,
            PartialEq,
            Eq,
            Hash,
            Default,
            lumen_macros::NodeEnum,
            lumen_macros::Delegate,
        )]
        #[repr(i64)]
        $(#$attr)*
        pub enum $name {
            $($(#$variant_attr)* $variant = $value,)*
        }

        impl From<$name> for $twin {
            fn from(value: $name) -> Self {
                match value {
                    $($name::$variant => Self::$variant,)*
                }
            }
        }
    };
}
pub(super) use text_enum;

text_enum! {
    #[delegate(kind = "enum", expressions)]
    TextFontStyle => lumen_text::TextFontStyle {
        #[default]
        Normal = 0,
        Italic = 1,
        Oblique = 2,
    }
}

text_enum! {
    #[delegate(kind = "enum")]
    TextAlignmentHorizontal => lumen_text::TextAlign {
        #[default]
        Left = 0,
        Center = 1,
        Right = 2,
        Justify = 3,
    }
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

text_enum! {
    #[delegate(kind = "enum")]
    TextRenderMode => lumen_text::TextRenderMode {
        #[default]
        Raster = 0,
        Msdf = 1,
        Auto = 2,
    }
}

text_enum! {
    #[delegate(kind = "enum")]
    TextOverflow => lumen_text::TextOverflow {
        #[default]
        Clip = 0,
        Ellipsis = 1,
    }
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

text_enum! {
    #[delegate(kind = "enum", expressions)]
    TextTransform => lumen_text::TextTransform {
        #[default]
        None = 0,
        Uppercase = 1,
        Lowercase = 2,
        /// Uppercases the first letter of each word.
        Capitalize = 3,
    }
}

/// What a text shapes with: its content, the style its spans inherit, and how its lines wrap,
/// space and align. Evaluating these reads neither paint nor decorations, which may measure the
/// text.
#[derive(Debug, Clone, PartialEq, lumen_macros::Delegate)]
pub struct TextLayoutParams {
    /// Text to render: a string, or spans that each override the style these params give. A
    /// node draws at most 16384 glyphs unless the host sets another limit, counting a stroked
    /// glyph twice and each decoration rect once; whitespace is not drawn, and more is a render
    /// error.
    #[meta(multiline, recommended_rows = 4)]
    pub content: TextContent,
    /// Font family name, or `asset:<id>` for a font asset of the host's.
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
    /// Horizontal text alignment.
    #[meta(kind = "enum", enum_type = TextAlignmentHorizontal)]
    pub alignment_horizontal: TextAlignmentHorizontal,
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

impl Default for TextLayoutParams {
    fn default() -> Self {
        Self {
            content: TextContent::default(),
            font_family: lumen_text::DEFAULT_FONT_FAMILY.to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            max_width: 0.0,
            letter_spacing: 0.0,
            font_features: FontFeatures::default(),
            line_height: 0.0,
            alignment_horizontal: TextAlignmentHorizontal::Left,
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

/// The node's decorations and stroke, which spans that leave theirs unset take.
#[derive(Debug, Clone, Default, PartialEq, lumen_macros::Delegate)]
pub struct TextDecorationParams {
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
}

/// How a text's shaped lines are drawn and placed about `position`.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct TextDrawParams {
    /// Glyph rendering mode. MSDF reuses outline distance fields across sizes; Auto is MSDF from 512 px.
    #[meta(kind = "enum", enum_type = TextRenderMode)]
    pub render_mode: TextRenderMode,
    /// Text color, and that of spans without their own.
    #[meta(role = "color")]
    pub color: Paint,
    /// Box a span's own gradient `color` spans: the span's lines, or the whole text. The
    /// node's `color` always spans the whole text.
    #[meta(kind = "enum", enum_type = TextPaintSpace)]
    pub paint_space: TextPaintSpace,
    /// Vertical text alignment.
    #[meta(kind = "enum", enum_type = TextAlignmentVertical)]
    pub alignment_vertical: TextAlignmentVertical,
    /// Enables 4×4 supersampling when evaluating paint at each glyph pixel.
    #[meta()]
    pub paint_supersample: bool,
}

impl Default for TextDrawParams {
    fn default() -> Self {
        Self {
            render_mode: TextRenderMode::Raster,
            color: Paint::solid([255, 255, 255, 255]),
            paint_space: TextPaintSpace::Span,
            alignment_vertical: TextAlignmentVertical::Top,
            paint_supersample: true,
        }
    }
}

/// The per-glyph params, which are evaluated for each glyph rather than with the others.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct TextGlyphParams {
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
}

impl Default for TextGlyphParams {
    fn default() -> Self {
        Self {
            glyph_offset: (0.0, 0.0),
            glyph_scale: (1.0, 1.0),
            glyph_rotation: 0.0,
            glyph_opacity: 1.0,
        }
    }
}

/// Produces a text source.
#[derive(Debug, Clone, Default, lumen_macros::Delegate)]
pub struct TextParams {
    #[meta(flatten)]
    pub layout: TextLayoutParams,
    #[meta(flatten)]
    pub decorations: TextDecorationParams,
    #[meta(flatten)]
    pub draw: TextDrawParams,
    #[meta(
        flatten,
        doc(
            position = "Text origin in pixels.",
            scale = "Scale multiplier per axis, about the anchor. Raster glyphs are resampled, so\nuse MSDF rendering to keep scaled text sharp.",
            motion_blur = "Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the\nusual film look and 360 smears over the whole frame. Blurs the text's own movement,\nnot that of the `glyph_*` params."
        )
    )]
    pub placement: DrawablePlacement,
    #[meta(flatten)]
    pub glyph: TextGlyphParams,
}

/// Produces a text source.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "text", name = "Text", category = "source")]
pub struct Text {
    pub id: NodeId,
    #[params]
    pub params: TextParamsDelegate,
}

/// A text node reached along one path, which keeps its shaping across frames.
#[derive(Debug)]
pub(crate) struct TextInstance {
    pub(crate) node_id: NodeId,
    pub(crate) draw: TextDrawParamsDelegate,
    pub(crate) placement: DrawablePlacementDelegate,
    pub(crate) glyph: TextGlyphParamsDelegate,
    /// The layout, whose spans' styles are in `span_styles`.
    pub(crate) layout: Stable<TextLayoutParamsDelegate>,
    pub(crate) span_styles: Stable<Vec<SpanStyleDelegate>>,
    pub(crate) decorations: Stable<TextDecorationParamsDelegate>,
    /// Whether the node or any span has a decoration or stroke.
    pub(crate) decorated: bool,
    /// Whether the node or any span has a stroke.
    pub(crate) stroked: bool,
    /// The number of span paints: one for each span, or the one of string content.
    pub(crate) paint_count: usize,
    pub(crate) size: lumen_gpu::Size,
    /// The size of what the glyphs are drawn to: the layer of blurred text, else the output.
    pub(crate) layer_size: lumen_gpu::Size,
    /// Whether binding measures where the text draws, to size its output.
    pub(crate) measure_ink: bool,
    /// Whether the text's own motion is blurred. It is not on a path a `motion_blur` samples.
    pub(crate) motion_blur: bool,
    pub(crate) state: Arc<Mutex<TextState>>,
}

/// What a bound text draws this frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct TextDraw {
    pub(crate) globals: lumen_gpu::Res,
    pub(crate) style: lumen_gpu::Res,
    pub(crate) instances: lumen_gpu::Res,
    /// A paint for each span, or the one paint of string content, and then those of the
    /// decorations with their own and of the strokes.
    pub(crate) paints: lumen_gpu::Res,
    pub(crate) glyph_count: u32,
    pub(crate) opacity: f64,
    pub(crate) canvas_bounds: Option<[f32; 4]>,
}

impl crate::gpu::frame_graph::RecordNode for Text {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        super::text_record::text_programs(programs)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        super::text_record::record_text(self, cx)
    }
}

impl TextInstance {
    /// Fails when the text draws `drawn` glyph, stroke and decoration instances, more than the
    /// host's limit of `max`.
    fn check_capacity(&self, drawn: usize, max: usize) -> crate::Result<()> {
        if drawn <= max {
            return Ok(());
        }
        Err(RenderError::NodeEvaluation {
            node_id: self.node_id,
            node_kind: <Text as JsonNode>::KIND,
            details: format!(
                "text draws {drawn} glyphs, strokes and decorations, more than the limit of \
                 {max} a text node can draw; split it across nodes, shorten it, or raise the \
                 host's text glyph limit (`--max-text-glyphs` of lumen-local and lumen-server, \
                 `set_max_text_glyphs` of the renderer)"
            ),
        }
        .into())
    }

    /// Evaluates the text with `expr`, placing its glyphs in the atlas and writing what it draws
    /// to `rec`'s buffers. When it shapes again, the families it asks for that no font answers to
    /// are added to `fallbacks`.
    pub(crate) fn bind(
        &self,
        mut expr: ExpressionContext<'_>,
        media: Option<&dyn MediaStore>,
        bound: &mut BoundFrame,
        rec: &mut lumen_gpu::Recorder,
        fallbacks: &mut Vec<String>,
        max_glyphs: usize,
    ) -> crate::Result<TextDraw> {
        let trace_started =
            tracing::enabled!(target: "lumen_text", tracing::Level::TRACE).then(trace_now_ms);
        let fields = DelegateEvalContext {
            node_id: self.node_id,
            expr: &expr,
        };
        let draw = self.draw.eval(&fields)?;
        let mut placement = self.placement.eval(&fields)?;
        if !self.motion_blur {
            placement.motion_blur = 0.0;
        }
        let (position_x, position_y) = placement.position;
        let drawable = placement.transform();
        let shutter = self.placement.shutter(&placement, self.node_id, &expr)?;
        let layout = self.layout.get(&fields)?;
        let span_styles = self
            .span_styles
            .get(&fields)
            .map_err(|error| error.in_field("content"))?;
        let decorations = self.decorations.get(&fields)?;

        let mut text_system = text_system(media)?;
        load_font_families(&mut text_system, media, &layout)?;
        let mut state = self.state.lock().map_err(|_| {
            LumenError::Render(RenderError::Internal {
                details: format!("text node {} state lock was poisoned", self.node_id.0),
            })
        })?;
        let state = &mut *state;

        let font_generation = text_system.font_generation();
        let mut layout_ms = 0.0;
        if state
            .shaped
            .as_ref()
            .is_none_or(|(key, _)| !key.holds(&layout, &span_styles, font_generation))
        {
            let layout_started = trace_started.map(|_| trace_now_ms());
            fallbacks.extend(
                used_font_families(&layout)
                    .filter(|family| !family.is_empty() && !text_system.has_family(family))
                    .map(str::to_string),
            );
            let shaped = text_system.shape_changed(&layout.request(&span_styles));
            let key = TextShapeKey {
                layout: match layout {
                    Cow::Owned(layout) => Some(layout),
                    Cow::Borrowed(_) => None,
                },
                baseline_shifts: span_styles.iter().map(|span| span.baseline_shift).collect(),
                font_generation,
            };
            state.ink = if self.measure_ink {
                text_system.ink_bounds(&shaped)
            } else {
                None
            };
            state.shaped = Some((key, shaped));
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
            match draw.alignment_vertical {
                TextAlignmentVertical::Top => position_y as f32,
                TextAlignmentVertical::Middle => position_y as f32 - height * 0.5,
                TextAlignmentVertical::Bottom => position_y as f32 - height,
            },
        ];
        let layout = shaped.place(origin);
        let spans: &[SpanStyle] = &span_styles;
        // A span's own gradient spans its box under `TextPaintSpace::Span`, and other paints
        // the whole text.
        let text_paint = |paint: &Paint, span: Option<usize>| {
            let mut gpu = paint.to_gpu();
            gpu.paint_supersample = u32::from(draw.paint_supersample);
            let own_box = span
                .filter(|_| {
                    draw.paint_space == TextPaintSpace::Span
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
        state.strokes.clear();
        state.half_widths.clear();
        if self.decorated {
            let own;
            (before_glyphs, own) = decorate(
                shaped,
                &decorations,
                spans,
                origin,
                self.paint_count,
                &mut state.decorations,
            );
            decoration_paints.extend(own.into_iter().map(|(paint, span)| (paint, span, 0.0)));
            self.check_capacity(state.decorations.len(), max_glyphs)?;
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
        let mode = lumen_text::TextRenderMode::from(draw.render_mode);
        let atlas_started = trace_started.map(|_| trace_now_ms());
        let mut atlas = bound
            .text_atlas
            .as_ref()
            .expect("frame atlas")
            .lock()
            .map_err(|_| RenderError::Internal {
                details: "text atlas lock was poisoned".to_string(),
            })?;
        let mut prepared = text_system.prepare_glyphs(
            &mut atlas,
            &layout,
            mode,
            &state.half_widths,
            &mut state.overfull,
        );
        let atlas_size = atlas.size();
        drop(atlas);
        drop(text_system);
        let atlas_ms = atlas_started.map_or(0.0, |started| trace_now_ms() - started);

        // Glyphs the atlas dropped are still drawn text, so they count toward the limit.
        self.check_capacity(
            state.decorations.len()
                + prepared.instances.len()
                + prepared.strokes.len()
                + prepared.dropped_glyphs,
            max_glyphs,
        )?;
        let dropped_glyphs = prepared.dropped_glyphs;
        let dropping = dropped_glyphs > 0;
        bound.text_dropped_glyphs |= dropping;
        if dropping != state.dropping_glyphs {
            if dropping {
                tracing::warn!(
                    target: "lumen_text",
                    node_id = self.node_id.0,
                    dropped_glyphs,
                    atlas_size,
                    "text glyphs dropped: glyph atlas full"
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
        state.paints.clear();
        for span in 0..self.paint_count {
            let own = spans.get(span).and_then(|span| span.color.as_ref());
            state
                .paints
                .push(text_paint(own.unwrap_or(&draw.color), own.and(Some(span))));
        }
        for (paint, span, half_width) in decoration_paints {
            let mut paint = text_paint(paint, span);
            paint.paint.stroke = half_width.to_bits();
            state.paints.push(paint);
        }
        for upload in prepared.uploads {
            let [width, height] = upload.size;
            bound.add_atlas_region(
                upload.pixels,
                upload.origin,
                lumen_gpu::Size::new(width, height),
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
        state.stroke_at.clear();
        if !prepared.strokes.is_empty() {
            state.stroke_at.resize(prepared.instances.len(), None);
        }
        for (mut instance, &glyph) in prepared.strokes.into_iter().zip(&prepared.stroke_glyphs) {
            let fill = &prepared.instances[glyph];
            let span = layout.glyphs[prepared.sources[glyph]].item.span as usize;
            let stroke = state.strokes[span].expect("a stroked glyph's span has a stroke");
            instance.set_paint(stroke.paint);
            instance.opacity = fill.opacity;
            if fill.flags & lumen_text::GLYPH_TRANSFORMED != 0 {
                instance.flags |= lumen_text::GLYPH_TRANSFORMED;
                instance.linear = fill.linear;
                instance.offset = fill.offset;
                ink_bounds = Some(union_bounds(ink_bounds, transformed_bounds(&instance)));
            }
            let list = if stroke.over {
                &mut state.over_strokes
            } else {
                &mut state.under_strokes
            };
            state.stroke_at[glyph] = Some((stroke.over, list.len()));
            list.push(instance);
        }
        link_fading_strokes(
            &mut prepared.instances,
            &mut state.under_strokes,
            &mut state.over_strokes,
            &state.stroke_at,
            before_glyphs,
        );
        let stroke_count = state.under_strokes.len() + state.over_strokes.len();
        let globals = lumen_text::GpuTextGlobals {
            target_size: [self.layer_size.width as f32, self.layer_size.height as f32],
            px_range: lumen_text::MSDF_PX_RANGE,
            glyph_count: (state.decorations.len() + prepared.instances.len() + stroke_count) as u32,
        };
        let (min, max) = draw_bounds(
            layout.bounds,
            ink_bounds,
            &state.decorations,
            &state.strokes,
        );
        let mut canvas_bounds = None;
        if self.measure_ink {
            let stroke_reach = state
                .strokes
                .iter()
                .flatten()
                .map(|stroke| lumen_text::stroke_reach(stroke.half_width))
                .fold(0.0, f32::max);
            let decorations = &state.decorations;
            canvas_bounds = state.ink.map(|ink| {
                let (min, max) = decorations.iter().fold(
                    (
                        [
                            origin[0] + ink[0] - stroke_reach,
                            origin[1] + ink[1] - stroke_reach,
                        ],
                        [
                            origin[0] + ink[2] + stroke_reach,
                            origin[1] + ink[3] + stroke_reach,
                        ],
                    ),
                    |(min, max), rect| {
                        let [x0, y0, x1, y1] = rect.rect_bounds();
                        (
                            [min[0].min(x0), min[1].min(y0)],
                            [max[0].max(x1), max[1].max(y1)],
                        )
                    },
                );
                let transform = DrawableTransform {
                    opacity: 1.0,
                    ..drawable
                };
                let [x0, y0, x1, y1] = transform.to_gpu().canvas_bounds(min, max);
                [x0 - 1.0, y0 - 1.0, x1 + 1.0, y1 + 1.0]
            });
        }
        let motion = DrawableMotion::new(shutter.as_ref(), min, max);
        let canvas = [self.size.width as f32, self.size.height as f32];
        let layer = [self.layer_size.width as f32, self.layer_size.height as f32];
        let style = TextStyleUniforms {
            transform: motion.layer_transform(&drawable, min, max, canvas, layer),
            motion: motion.to_gpu(),
        };
        // Highlights and underlines are drawn under the glyphs and their strokes, and
        // strikethroughs over them.
        let (under, over) = state.decorations.split_at(before_glyphs);
        let lists = [
            under,
            &state.under_strokes,
            &prepared.instances,
            &state.over_strokes,
            over,
        ];
        let instances = rec.storage_parts(&lists.map(bytemuck::cast_slice::<_, u8>));
        lists
            .into_iter()
            .for_each(|list| bound.add_glyph_instances(list));

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
        Ok(TextDraw {
            globals: rec.uniform(&globals),
            style: rec.uniform(&style),
            instances,
            paints: rec.storage_parts(&[bytemuck::cast_slice(&state.paints)]),
            glyph_count: globals.glyph_count,
            opacity: drawable.opacity,
            canvas_bounds,
        })
    }
}

/// A faded glyph's lower layer reads its partner's coverage, so the pair fades as one, and the
/// strokes of a fading cluster fade as one like its fills. `stroke_at` is
/// [`TextState::stroke_at`], and `before_glyphs` counts the decorations drawn under `under`.
fn link_fading_strokes(
    fills: &mut [lumen_text::GpuGlyphInstance],
    under: &mut [lumen_text::GpuGlyphInstance],
    over: &mut [lumen_text::GpuGlyphInstance],
    stroke_at: &[Option<(bool, usize)>],
    before_glyphs: usize,
) {
    if stroke_at.is_empty() || !fills.iter().any(|fill| fill.opacity < 1.0) {
        return;
    }
    let first_fill = before_glyphs + under.len();
    let first_over = first_fill + fills.len();
    let mut index = 0;
    while index < fills.len() {
        let lead = index;
        index += 1 + (fills[lead].flags >> lumen_text::GLYPH_CLUSTER_SHIFT) as usize;
        if fills[lead].opacity >= 1.0 {
            continue;
        }
        let mut strokes = stroke_at[lead..index].iter().flatten();
        let Some(&(is_over, first)) = strokes.next() else {
            continue;
        };
        let list = if is_over { &mut *over } else { &mut *under };
        for &(_, at) in strokes.filter(|(other, _)| *other == is_over) {
            list[at].flags |= lumen_text::GLYPH_CLUSTER_FOLLOWER;
            list[first].flags += 1 << lumen_text::GLYPH_CLUSTER_SHIFT;
        }
        let (lower, partner) = if is_over {
            (&mut fills[lead], first_over + first)
        } else {
            (&mut list[first], first_fill + lead)
        };
        lower.flags |= lumen_text::GLYPH_PARTNER;
        lower.partner = partner as u32;
    }
}

/// The corners a text may draw between before its own transform: its advance box, the ink of
/// transformed glyphs, its decorations and the reach of its strokes.
fn draw_bounds(
    [x, y, width, height]: [f32; 4],
    ink_bounds: Option<[f32; 4]>,
    decorations: &[lumen_text::GpuGlyphInstance],
    strokes: &[Option<SpanStroke>],
) -> ([f32; 2], [f32; 2]) {
    let (mut min, mut max) =
        ink_bounds.map_or(([x, y], [x + width, y + height]), |[x0, y0, x1, y1]| {
            (
                [x0.min(x), y0.min(y)],
                [x1.max(x + width), y1.max(y + height)],
            )
        });
    for decoration in decorations {
        let [x0, y0, x1, y1] = decoration.rect_bounds();
        min = [min[0].min(x0), min[1].min(y0)];
        max = [max[0].max(x1), max[1].max(y1)];
    }
    // Strokes reach past the glyphs, and a pixel further when antialiased.
    let reach = strokes
        .iter()
        .flatten()
        .map(|stroke| lumen_text::stroke_reach(stroke.half_width) + 1.0)
        .fold(0.0, f32::max);
    (
        min.map(|bound| bound - reach),
        max.map(|bound| bound + reach),
    )
}

impl TextGlyphParamsDelegate {
    /// Whether these params leave every glyph as it is.
    pub(crate) fn is_identity(&self) -> bool {
        matches!(&self.glyph_offset, Deferred::Value(offset) if *offset == (0.0, 0.0))
            && matches!(&self.glyph_scale, Deferred::Value(scale) if *scale == (1.0, 1.0))
            && matches!(&self.glyph_rotation, Deferred::Value(rotation) if *rotation == 0.0)
            && matches!(&self.glyph_opacity, Deferred::Value(opacity) if *opacity >= 1.0)
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
        let mut offset = PerGlyph::new(&self.glyph_offset, "glyph_offset");
        let mut scale = PerGlyph::new(&self.glyph_scale, "glyph_scale");
        let mut rotation = PerGlyph::new(&self.glyph_rotation, "glyph_rotation");
        let mut opacity = PerGlyph::new(&self.glyph_opacity, "glyph_opacity");
        let mut bounds: Option<[f32; 4]> = None;
        // The instance and cluster that the instances so far fade as one.
        let mut fading: Option<(usize, (u32, u32))> = None;
        for (index, &source) in sources.iter().enumerate() {
            let glyph = &layout.glyphs[source];
            let item = glyph.item;
            expr.glyph = Some(GlyphScope {
                item,
                counts: layout.counts,
            });
            let [x, y] = glyph.pivot.map(f64::from);
            let transform = DrawableTransform {
                position: (x, y),
                scale: scale.eval(item, node_id, expr)?,
                rotation: rotation.eval(item, node_id, expr)?,
                anchor: (0.0, 0.0),
                opacity: opacity.eval(item, node_id, expr)?,
            }
            .to_gpu();
            let (offset_x, offset_y) = offset.eval(item, node_id, expr)?;
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
            bounds = Some(union_bounds(bounds, transformed_bounds(instance)));
        }
        expr.glyph = None;
        Ok(bounds)
    }
}

/// The box `[min_x, min_y, max_x, max_y]` around `bounds`, if any, and `other`.
fn union_bounds(bounds: Option<[f32; 4]>, other: [f32; 4]) -> [f32; 4] {
    bounds.map_or(other, |bounds| {
        [
            bounds[0].min(other[0]),
            bounds[1].min(other[1]),
            bounds[2].max(other[2]),
            bounds[3].max(other[3]),
        ]
    })
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

impl<'a, T: ParamValue + Copy> PerGlyph<'a, T> {
    fn new(param: &'a Deferred<T>, name: &'static str) -> Self {
        let reads = match param {
            Deferred::Expr(expression) => {
                let reads = expression.reads;
                [reads.glyph, reads.word, reads.line, reads.span]
            }
            Deferred::Value(_) => [false; 4],
        };
        Self {
            param,
            name,
            reads,
            last: None,
        }
    }

    fn eval(
        &mut self,
        item: lumen_text::TextItem,
        node_id: NodeId,
        expr: &ExpressionContext<'_>,
    ) -> crate::Result<T> {
        if let Deferred::Value(value) = self.param {
            return Ok(*value);
        }
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
    shaped: Option<(TextShapeKey, Arc<lumen_text::ShapedText>)>,
    /// The ink of `shaped`, relative to where it is placed, when the node measures it.
    ink: Option<[f32; 4]>,
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
    /// Where each glyph's stroke is, if any: whether over the glyphs, and its place in that list.
    /// Empty when no glyph has a stroke.
    stroke_at: Vec<Option<(bool, usize)>>,
}

#[derive(Debug)]
struct TextShapeKey {
    /// The layout, or `None` when it is constant.
    layout: Option<TextLayoutParams>,
    baseline_shifts: Vec<Option<f64>>,
    font_generation: u64,
}

impl TextShapeKey {
    /// Whether this is the key of text evaluated to these values.
    fn holds(
        &self,
        layout: &TextLayoutParams,
        span_styles: &[SpanStyle],
        font_generation: u64,
    ) -> bool {
        self.font_generation == font_generation
            && self.layout.as_ref().is_none_or(|own| own == layout)
            && self
                .baseline_shifts
                .iter()
                .copied()
                .eq(span_styles.iter().map(|span| span.baseline_shift))
    }
}

/// A value evaluated once when it has no expressions, or else on each bind.
#[derive(Debug)]
pub(crate) enum Stable<D: DelegateEvaluable> {
    Constant(D::Evaluated),
    Animated(D),
}

impl<D> Stable<D>
where
    D: DelegateEvaluable,
    D::Evaluated: Clone,
{
    pub(crate) fn new(delegate: D, ctx: &DelegateEvalContext<'_>) -> crate::Result<Self> {
        Ok(if delegate.has_expressions() {
            Self::Animated(delegate)
        } else {
            Self::Constant(delegate.eval(ctx)?)
        })
    }

    fn get(&self, ctx: &DelegateEvalContext<'_>) -> crate::Result<Cow<'_, D::Evaluated>> {
        Ok(match self {
            Self::Constant(value) => Cow::Borrowed(value),
            Self::Animated(delegate) => Cow::Owned(delegate.eval(ctx)?),
        })
    }
}

impl TextLayoutParams {
    /// The request that shapes this text, with each span's baseline shift from `span_styles`.
    /// A `max_width` or `line_height` of 0 or less is automatic.
    pub(crate) fn request(&self, span_styles: &[SpanStyle]) -> lumen_text::TextLayoutRequest {
        let spans = self
            .content
            .0
            .iter()
            .enumerate()
            .map(|(index, span)| {
                let shift = span_styles.get(index).and_then(|span| span.baseline_shift);
                self.span(span, shift)
            })
            .collect();
        let mut request = lumen_text::TextLayoutRequest::from_spans(spans);
        request.max_width = Some(self.max_width as f32).filter(|width| *width > 0.0);
        request.line_height = (self.line_height > 0.0).then_some(self.line_height as f32);
        request.paragraph_spacing = self.paragraph_spacing.max(0.0) as f32;
        request.align = self.alignment_horizontal.into();
        request.max_lines = self
            .max_lines
            .map(|lines| lines.clamp(0, u32::MAX.into()) as u32);
        request.max_height = self.max_height.map(|height| height.max(0.0) as f32);
        request.overflow = self.overflow.into();
        request.min_fit_scale = (self.fit == TextFit::Shrink)
            .then(|| (self.min_font_size / self.font_size.max(1.0)) as f32);
        request
    }

    fn span(&self, span: &SpanLayout, baseline_shift: Option<f64>) -> lumen_text::TextSpan {
        lumen_text::TextSpan {
            text: span.text.clone(),
            font_family: span
                .font_family
                .as_deref()
                .unwrap_or(&self.font_family)
                .to_string(),
            font_size: span.font_size.unwrap_or(self.font_size) as f32,
            font_weight: span.font_weight.unwrap_or(self.font_weight).clamp(1, 1000) as u16,
            font_style: span.font_style.unwrap_or(self.font_style).into(),
            letter_spacing: span.letter_spacing.unwrap_or(self.letter_spacing) as f32,
            font_features: self.font_features.merged(span.font_features.as_ref()),
            transform: span.text_transform.unwrap_or(self.text_transform).into(),
            baseline_shift: baseline_shift.unwrap_or(0.0) as f32,
        }
    }
}

/// Appends the rect instances of the decorations of `shaped`, placed at `origin`, to
/// `instances`: highlights, then underlines, then strikethroughs. Returns how many come before
/// the strikethroughs, which are drawn over the glyphs.
///
/// Consecutive spans that take the node's decoration share one run, and a span with its own is
/// a run alone. A line without its own paint takes its span's paint, among the `paint_count`
/// of the spans. Spans without a color of their own share the first one's paint, so a line
/// runs along them as one rect. Each decoration's own paint is returned with the span whose box
/// it spans if it is the span's own, and has the index `paint_count` and its place in the
/// returned paints.
pub(crate) fn decorate<'a>(
    shaped: &lumen_text::ShapedText,
    node: &'a TextDecorationParams,
    spans: &'a [SpanStyle],
    origin: [f32; 2],
    paint_count: usize,
    instances: &mut Vec<lumen_text::GpuGlyphInstance>,
) -> (usize, Vec<(&'a Paint, Option<usize>)>) {
    let plain = spans
        .iter()
        .position(|span| span.color.is_none())
        .unwrap_or(0) as u32;
    let text_paint = |span: usize| match spans.get(span) {
        Some(SpanStyle { color: Some(_), .. }) => span as u32,
        _ => plain,
    };
    let mut paints = Vec::new();
    let mut push_paint = |paint: &'a Paint, span: Option<usize>| {
        paints.push((paint, span));
        (paint_count + paints.len() - 1) as u32
    };
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
                    style: line.style.into(),
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
    (before_glyphs, paints)
}

/// The number of paints that `params`' decorations with their own and strokes add, or `None`
/// when neither the node nor any span has a decoration or stroke.
pub(crate) fn decoration_paints(params: &TextParamsDelegate) -> Option<usize> {
    let node = &params.decorations;
    let mut lines: Vec<&TextLineDelegate> =
        node.underline.iter().chain(&node.strikethrough).collect();
    // Highlights and strokes each have a paint.
    let mut painted = usize::from(node.highlight.is_some()) + usize::from(node.stroke.is_some());
    if let TextContentDelegate::Spans(spans) = &params.layout.content {
        for span in spans {
            let style = &span.style;
            for line in [&style.underline, &style.strikethrough] {
                if let Override::Set(line) = line {
                    lines.push(line);
                }
            }
            painted += usize::from(matches!(style.highlight, Override::Set(_)))
                + usize::from(matches!(style.stroke, Override::Set(_)));
        }
    }
    (!lines.is_empty() || painted > 0)
        .then(|| painted + lines.iter().filter(|line| line.paint.is_some()).count())
}

/// Whether the node or any span of `params` has a stroke.
pub(crate) fn stroked(params: &TextParamsDelegate) -> bool {
    params.decorations.stroke.is_some()
        || matches!(&params.layout.content, TextContentDelegate::Spans(spans)
            if spans.iter().any(|span| matches!(span.style.stroke, Override::Set(_))))
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
    spans: &'a [SpanStyle],
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
/// each span with its own. `spans` holds the spans' styles, which `own` reads; plain string
/// content has none, so takes the node's.
fn for_each_run<'a, T>(
    inherited: Option<&'a T>,
    spans: &'a [SpanStyle],
    span_count: usize,
    own: impl Fn(&'a SpanStyle) -> &'a Override<T>,
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

/// Loads the families `layout` and its spans name.
pub(crate) fn load_font_families(
    text_system: &mut lumen_text::TextSystem,
    media: Option<&dyn MediaStore>,
    layout: &TextLayoutParams,
) -> crate::Result<()> {
    for family in used_font_families(layout) {
        load_font_family(text_system, media, family)?;
    }
    Ok(())
}

/// The families text in `layout` is shaped in: each span's, and the node's when a span leaves its
/// own unset.
pub(crate) fn used_font_families(layout: &TextLayoutParams) -> impl Iterator<Item = &str> {
    let spans = &layout.content.0;
    let inherits = spans.is_empty() || spans.iter().any(|span| span.font_family.is_none());
    inherits
        .then_some(layout.font_family.as_str())
        .into_iter()
        .chain(spans.iter().filter_map(|span| span.font_family.as_deref()))
}

/// The families `text` names as literals, each with its param path, such as
/// `content[1].font_family`. The node's own is left out when every span names its own.
pub(crate) fn literal_font_families(text: &Text) -> Vec<(String, &str)> {
    let layout = &text.params.layout;
    let spans = match &layout.content {
        TextContentDelegate::Spans(spans) => spans.as_slice(),
        TextContentDelegate::Text(_) => &[],
    };
    let mut families = Vec::new();
    if (spans.is_empty() || spans.iter().any(|span| span.layout.font_family.is_none()))
        && let Deferred::Value(family) = &layout.font_family
    {
        families.push(("font_family".to_string(), family.as_str()));
    }
    for (index, span) in spans.iter().enumerate() {
        if let Some(Deferred::Value(family)) = &span.layout.font_family {
            families.push((format!("content[{index}].font_family"), family.as_str()));
        }
    }
    families
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
    let (identity, resolver) = match resolve_as::<dyn FontResolver>(store, font_family) {
        Ok(resolved) => resolved,
        // A family the store doesn't have falls back to other fonts, unless it names an asset.
        Err(error)
            if asset_id(font_family).is_none()
                && error.failure().is_some_and(|failure| {
                    matches!(failure.reason, MediaReason::NotFound { .. })
                }) =>
        {
            return Ok(());
        }
        Err(error) => return Err(LumenError::Media(error)),
    };
    if text_system.family_resolver(font_family) == Some(identity.as_str()) {
        return Ok(());
    }
    let files = font_files(font_family, &identity, &*resolver)?;
    let heads: Vec<Vec<u8>> = files
        .iter()
        .map(|file| file.iter().take(16).copied().collect())
        .collect();
    text_system
        .load_family(font_family, identity.as_str(), files)
        .map_err(|_| LumenError::Media(unreadable_font(font_family, &heads).into()))?;
    Ok(())
}

/// Why the files of `family`, whose first bytes are `heads`, hold no readable face.
fn unreadable_font(family: &str, heads: &[Vec<u8>]) -> MediaFailure {
    let wanted = [AssetKind::Font];
    for head in heads {
        if head.starts_with(b"wOFF") || head.starts_with(b"wOF2") {
            return MediaFailure::unsupported(
                family,
                "WOFF fonts are not loaded; use a TrueType or OpenType file",
            )
            .wanting(&wanted);
        }
        if let Some(found) = sniff(head).other_than(AssetKind::Font) {
            return MediaFailure::wrong_kind(family, &wanted, found, "the file is not a font");
        }
    }
    MediaFailure::decode(family, "its font data holds no readable face").wanting(&wanted)
}

/// The data of `resolver`'s font files, shared with every text system still holding that family
/// and identity's, so that jobs using one font hold and read it once.
fn font_files(
    family: &str,
    identity: &MediaIdentity,
    resolver: &dyn FontResolver,
) -> crate::Result<Vec<Arc<Vec<u8>>>> {
    type Files = HashMap<(String, String), Vec<Weak<Vec<u8>>>>;
    static FILES: LazyLock<Mutex<Files>> = LazyLock::new(Mutex::default);
    let lock = || {
        FILES.lock().map_err(|_| {
            LumenError::Render(RenderError::Internal {
                details: "font file cache lock was poisoned".to_string(),
            })
        })
    };
    let key = (family.to_string(), identity.as_str().to_string());
    if let Some(files) = lock()?
        .get(&key)
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
    cache.insert(key, files.iter().map(Arc::downgrade).collect());
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
    tracing::trace!(
        target: "lumen_text",
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
    use std::{sync::OnceLock, time::Instant};

    static START: OnceLock<Instant> = OnceLock::new();
    START.get_or_init(Instant::now).elapsed().as_secs_f64() * 1000.0
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn trace_now_ms() -> f64 {
    js_sys::Date::now()
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};

    use super::*;
    use crate::error::MediaError;
    use crate::media::{MediaResolver, ResolvedMedia};

    #[test]
    fn a_font_file_that_cant_be_read_fails_by_what_the_file_is() {
        let png = vec![b"\x89PNG\r\n\x1a\n".to_vec()];
        let cases = [
            (vec![b"wOFF\0\x01\0\0".to_vec()], "media.unsupported", None),
            (vec![b"wOF2".to_vec()], "media.unsupported", None),
            (png, "media.wrong_kind", Some(AssetKind::Image)),
            (vec![b"plain text".to_vec()], "media.decode", None),
        ];
        for (heads, code, found) in cases {
            let failure = unreadable_font("Brand", &heads);
            assert_eq!(failure.code(), code, "{failure}");
            assert_eq!(failure.found, found, "{failure}");
            assert_eq!(failure.wanted, [AssetKind::Font]);
            assert_eq!(failure.source, "Brand");
        }
    }

    /// Serves Inter as "Known", or every family if `every_family`, all as one identity.
    #[derive(Debug, Default)]
    struct CountingStore {
        every_family: bool,
        reads: Arc<AtomicU64>,
    }

    impl CountingStore {
        /// An identity of this store alone, so tests running together share no font.
        fn identity(&self) -> MediaIdentity {
            MediaIdentity::of_key(&format!("counting-store-{:p}", Arc::as_ptr(&self.reads)))
        }
    }

    struct CountingResolver {
        family: String,
        reads: Arc<AtomicU64>,
    }

    impl FontResolver for CountingResolver {
        fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
            self.reads.fetch_add(1, Ordering::Relaxed);
            Ok(vec![match self.family.as_str() {
                "Known" | "Inter" => INTER.to_vec(),
                _ => lumen_text::ROBOTO_REGULAR_BYTES.to_vec(),
            }])
        }
    }

    impl MediaStore for CountingStore {
        fn resolve(&self, family: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
            if kind != AssetKind::Font || !(self.every_family || family == "Known") {
                return Err(MediaError::source_not_found(family, "not served"));
            }
            Ok(ResolvedMedia::new(
                self.identity(),
                MediaResolver::Font(Box::new(CountingResolver {
                    family: family.to_string(),
                    reads: Arc::clone(&self.reads),
                })),
            ))
        }
    }

    const INTER: &[u8] = include_bytes!("../../../../text/assets/inter/InterVariable-Latin.ttf");

    #[test]
    fn a_font_loads_once_per_identity_and_a_missing_one_falls_back() {
        let store = CountingStore::default();
        let mut system = lumen_text::TextSystem::new();
        for _ in 0..3 {
            load_font_family(&mut system, Some(&store), "Known").unwrap();
            load_font_family(&mut system, Some(&store), "Unknown").unwrap();
        }
        assert_eq!(
            system.family_resolver("Known"),
            Some(store.identity().as_str())
        );
        assert_eq!(system.family_resolver("Unknown"), None);
        assert_eq!(store.reads.load(Ordering::Relaxed), 1);

        let missing = load_font_family(&mut system, Some(&store), "asset:brand").unwrap_err();
        assert_eq!(missing.code(), "media.not_found");
    }

    #[test]
    fn an_identity_only_shares_a_font_within_its_family() {
        let store = CountingStore {
            every_family: true,
            ..Default::default()
        };
        let mut system = lumen_text::TextSystem::new();
        load_font_family(&mut system, Some(&store), "Inter").unwrap();
        load_font_family(&mut system, Some(&store), "Other").unwrap();
        assert!(system.has_family("Inter"));
        assert_eq!(store.reads.load(Ordering::Relaxed), 2);
    }

    #[test]
    fn text_systems_share_a_font_while_one_holds_it() {
        let store = CountingStore::default();
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
