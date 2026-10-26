use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
};

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;

use crate::error::{LumenError, RenderError};
use crate::expr::{ExprNode, ExpressionContext, GlobalVar, GlyphScope};
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};
use crate::media::MediaStore;
use crate::node::vector::{
    drawable::{DeferredTransform, DrawableMotion, DrawableTransform},
    paint::Paint,
    renderer::TextStyleUniforms,
};
use crate::node::{Deferred, DeferredValue, NodeId, NodeParamEvalContext, NodeParams, PortRef};

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Hash, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
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
pub enum TextRenderMode {
    #[default]
    Raster = 0,
    Msdf = 1,
    Auto = 2,
}

/// Produces a text source.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct TextParams {
    /// Text content to render.
    #[meta(multiline, recommended_rows = 4)]
    pub content: String,
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
    /// and can read its `glyph`, `word` and `line` index and their counts.
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
    /// Text color.
    #[meta(role = "color")]
    pub color: Paint,
    /// Horizontal text alignment.
    #[meta(kind = "enum", enum_type = TextAlignmentHorizontal)]
    pub alignment_horizontal: TextAlignmentHorizontal,
    /// Vertical text alignment.
    #[meta(kind = "enum", enum_type = TextAlignmentVertical)]
    pub alignment_vertical: TextAlignmentVertical,
    /// Enables 4×4 supersampling when evaluating paint at each glyph pixel.
    #[meta()]
    pub paint_supersample: bool,
}

impl Default for TextParams {
    fn default() -> Self {
        Self {
            content: String::new(),
            font_family: lumen_text::DEFAULT_FONT_FAMILY.to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            render_mode: TextRenderMode::Raster,
            max_width: 0.0,
            letter_spacing: 0.0,
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
            alignment_horizontal: TextAlignmentHorizontal::Left,
            alignment_vertical: TextAlignmentVertical::Top,
            paint_supersample: true,
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
    /// The node's params, less the per-glyph ones in `glyph`.
    pub(crate) params: TextParamsDelegate,
    pub(crate) glyph: GlyphParams,
    pub(crate) atlas_texture: lumen_gpu::TextureId,
    pub(crate) globals_buffer: lumen_gpu::BufferId,
    pub(crate) instances_buffer: lumen_gpu::BufferId,
    pub(crate) paint_buffer: lumen_gpu::BufferId,
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
        let trace_started = crate::log_level_enabled(tracing::Level::TRACE).then(trace_now_ms);
        let mut expr = ctx.expr_context(self.node_id, "params");
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &expr,
        })?;
        let mut paint = evaluated.color.to_gpu([255, 255, 255, 255]);
        paint.paint_supersample = u32::from(evaluated.paint_supersample);
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
        let mut request = layout_request(
            evaluated.content,
            evaluated.font_family,
            evaluated.font_size as f32,
            evaluated.font_weight,
            evaluated.font_style,
            evaluated.max_width as f32,
            evaluated.alignment_horizontal,
        );
        apply_line_spacing(
            &mut request,
            evaluated.letter_spacing,
            evaluated.line_height,
        );

        let mut text_system = text_system()?;
        load_font_family(&mut text_system, ctx.media(), &request.font_family)?;
        let mut state = self.state.lock().map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: format!("text node {} state lock was poisoned", self.node_id.0),
            })
        })?;
        let state = &mut *state;

        let shape_key = TextShapeKey {
            content: request.content.clone(),
            font_family: request.font_family.clone(),
            font_size_bits: request.font_size.to_bits(),
            font_weight: request.font_weight,
            font_style: evaluated.font_style,
            max_width_bits: request.max_width.unwrap_or(0.0).to_bits(),
            letter_spacing_bits: request.letter_spacing.to_bits(),
            line_height_bits: request.line_height.unwrap_or(0.0).to_bits(),
            alignment_horizontal: evaluated.alignment_horizontal,
            font_generation: text_system.font_generation(),
        };
        let mut layout_ms = 0.0;
        if state
            .shaped
            .as_ref()
            .is_none_or(|(key, _)| *key != shape_key)
        {
            let layout_started = trace_started.map(|_| trace_now_ms());
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
        let mut prepared = text_system.prepare_glyphs(
            &mut atlas,
            &layout,
            mode,
            self.max_glyphs,
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

        let dropping = prepared.dropped_glyphs > 0;
        if dropping != state.dropping_glyphs {
            if dropping {
                tracing::warn!(
                    target: "lumen_text",
                    node_id = self.node_id.0,
                    dropped_glyphs = prepared.dropped_glyphs,
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
        let ink_bounds = self.glyph.apply(
            self.node_id,
            &layout,
            &mut prepared.instances,
            &prepared.sources,
            &mut expr,
        )?;
        let globals = lumen_text::GpuTextGlobals {
            target_size: [self.layer_size.width as f32, self.layer_size.height as f32],
            px_range: lumen_text::MSDF_PX_RANGE,
            glyph_count: prepared.instances.len() as u32,
            paint_bounds: layout.bounds,
        };
        bound.write_buffer(self.globals_buffer, 0, bytemuck::bytes_of(&globals));
        let [x, y, width, height] = layout.bounds;
        let (min, max) =
            ink_bounds.map_or(([x, y], [x + width, y + height]), |[x0, y0, x1, y1]| {
                (
                    [x0.min(x), y0.min(y)],
                    [x1.max(x + width), y1.max(y + height)],
                )
            });
        let motion = DrawableMotion::new(shutter.as_ref(), min, max);
        let canvas = [self.size.width as f32, self.size.height as f32];
        let layer = [self.layer_size.width as f32, self.layer_size.height as f32];
        let style = TextStyleUniforms {
            fill_paint: paint,
            transform: motion.layer_transform(&drawable, min, max, canvas, layer),
            motion: motion.to_gpu(),
        };
        bound.write_buffer(self.paint_buffer, 0, bytemuck::bytes_of(&style));
        if !prepared.instances.is_empty() {
            bound.write_buffer(
                self.instances_buffer,
                0,
                bytemuck::cast_slice(&prepared.instances),
            );
        }

        if let Some(started) = trace_started {
            trace_text_bind(
                self.node_id,
                upload_count,
                prepared.instances.len(),
                layout_ms,
                atlas_ms,
                trace_now_ms() - started,
            );
        }
        Ok(())
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

    fn is_identity(&self) -> bool {
        matches!(self.offset, Deferred::Value(offset) if offset == (0.0, 0.0))
            && matches!(self.scale, Deferred::Value(scale) if scale == (1.0, 1.0))
            && matches!(self.rotation, Deferred::Value(rotation) if rotation == 0.0)
            && matches!(self.opacity, Deferred::Value(opacity) if opacity >= 1.0)
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
        for (instance, &source) in instances.iter_mut().zip(sources) {
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
            instance.color[3] *= transform.opacity;
            if transform.flags == 0 && (offset_x, offset_y) == (0.0, 0.0) {
                continue;
            }
            instance.flags |= lumen_text::GLYPH_TRANSFORMED;
            instance.linear = transform.linear;
            instance.offset = [
                transform.offset[0] + offset_x as f32,
                transform.offset[1] + offset_y as f32,
            ];
            let [x, y, width, height] = instance.rect;
            for [x, y] in [
                [x, y],
                [x + width, y],
                [x, y + height],
                [x + width, y + height],
            ] {
                let [column_x, column_y] = instance.linear;
                let point = [
                    column_x[0] * x + column_y[0] * y + instance.offset[0],
                    column_x[1] * x + column_y[1] * y + instance.offset[1],
                ];
                let [x0, y0, x1, y1] =
                    bounds.get_or_insert([point[0], point[1], point[0], point[1]]);
                (*x0, *y0) = (x0.min(point[0]), y0.min(point[1]));
                (*x1, *y1) = (x1.max(point[0]), y1.max(point[1]));
            }
        }
        expr.glyph = None;
        Ok(bounds)
    }
}

/// A per-glyph param, evaluated again only when an index it reads changes. A constant, or
/// an expression that reads no index, is evaluated once.
struct PerGlyph<'a, T> {
    param: &'a Deferred<T>,
    name: &'static str,
    /// Whether the param reads the glyph, word and line index.
    reads: [bool; 3],
    last: Option<([u32; 3], T)>,
}

impl<'a, T: DeferredValue + Copy> PerGlyph<'a, T> {
    fn new(param: &'a Deferred<T>, name: &'static str) -> Self {
        let mut reads = [false; 3];
        if let Deferred::Expr(expression) = param {
            crate::expr::functions::visit(&expression.ast, &mut |node| match node {
                ExprNode::Global(GlobalVar::Glyph) => reads[0] = true,
                ExprNode::Global(GlobalVar::Word) => reads[1] = true,
                ExprNode::Global(GlobalVar::Line) => reads[2] = true,
                // A referenced property may read any of them.
                ExprNode::PropertyValue(..) => reads = [true; 3],
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
        let key = [item.glyph, item.word, item.line];
        let key: [u32; 3] =
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
    dropping_glyphs: bool,
    overfull: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextShapeKey {
    content: String,
    font_family: String,
    font_size_bits: u32,
    font_weight: u16,
    font_style: TextFontStyle,
    max_width_bits: u32,
    letter_spacing_bits: u32,
    line_height_bits: u32,
    alignment_horizontal: TextAlignmentHorizontal,
    font_generation: u64,
}

pub(crate) fn layout_request(
    content: String,
    font_family: String,
    font_size: f32,
    font_weight: i64,
    font_style: TextFontStyle,
    max_width: f32,
    alignment_horizontal: TextAlignmentHorizontal,
) -> lumen_text::TextLayoutRequest {
    let mut request = lumen_text::TextLayoutRequest::new(content);
    request.font_family = font_family;
    request.font_size = font_size;
    request.font_weight = font_weight.clamp(1, 1000) as u16;
    request.font_style = match font_style {
        TextFontStyle::Italic => lumen_text::TextFontStyle::Italic,
        TextFontStyle::Oblique => lumen_text::TextFontStyle::Oblique,
        TextFontStyle::Normal => lumen_text::TextFontStyle::Normal,
    };
    request.max_width = (max_width > 0.0).then_some(max_width);
    request.align = match alignment_horizontal {
        TextAlignmentHorizontal::Center => lumen_text::TextAlign::Center,
        TextAlignmentHorizontal::Right => lumen_text::TextAlign::Right,
        TextAlignmentHorizontal::Justify => lumen_text::TextAlign::Justified,
        TextAlignmentHorizontal::Left => lumen_text::TextAlign::Left,
    };
    request
}

/// A `line_height` of 0 or less keeps the automatic line height.
pub(crate) fn apply_line_spacing(
    request: &mut lumen_text::TextLayoutRequest,
    letter_spacing: f64,
    line_height: f64,
) {
    request.letter_spacing = letter_spacing as f32;
    request.line_height = (line_height > 0.0).then_some(line_height as f32);
}

/// Shared by rendering and text measurement so both see the same loaded fonts.
/// Do not hold the guard while evaluating expressions, which may measure text.
pub(crate) fn text_system() -> crate::Result<std::sync::MutexGuard<'static, lumen_text::TextSystem>>
{
    static TEXT_SYSTEM: OnceLock<Mutex<lumen_text::TextSystem>> = OnceLock::new();
    TEXT_SYSTEM
        .get_or_init(|| Mutex::new(lumen_text::TextSystem::new()))
        .lock()
        .map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: "text system lock was poisoned".to_string(),
            })
        })
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

    let mut loaded = loaded_fonts()?;
    if let Some(resolver_id) = store.font_resolver_id(font_family)
        && loaded.get(font_family) == Some(&resolver_id)
    {
        return Ok(());
    }
    let Some(resolver) = store.get_font_resolver(font_family) else {
        return Ok(());
    };
    let resolver_id = resolver.id().to_string();
    if loaded.get(font_family) == Some(&resolver_id) {
        return Ok(());
    }
    for data in resolver.data().map_err(LumenError::Media)? {
        text_system.load_font_data(data);
    }
    loaded.insert(font_family.to_string(), resolver_id);
    Ok(())
}

fn loaded_fonts() -> crate::Result<std::sync::MutexGuard<'static, HashMap<String, String>>> {
    static LOADED_FONTS: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    LOADED_FONTS
        .get_or_init(|| Mutex::new(HashMap::new()))
        .lock()
        .map_err(|_| {
            LumenError::Render(RenderError::Gpu {
                details: "loaded fonts lock was poisoned".to_string(),
            })
        })
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
