use std::{
    collections::HashMap,
    sync::atomic::{AtomicU64, Ordering},
    sync::{Arc, Mutex, OnceLock},
};

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
use std::time::Instant;

use crate::error::{LumenError, RenderError};
use crate::gpu::{BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode};
use crate::media::MediaStore;
use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef, vector::paint::Paint};

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

/// Produces a text raster source.
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
    /// Font weight.
    #[meta(min = 100, max = 900, step = 100)]
    pub font_weight: i64,
    /// Font style.
    #[meta(kind = "enum", enum_type = TextFontStyle)]
    pub font_style: TextFontStyle,
    /// Maximum line width in pixels. Use 0 for automatic width.
    #[meta(min = 0, step = 1)]
    pub max_width: f64,
    /// Text origin in pixels.
    #[meta()]
    pub position: (f64, f64),
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
            max_width: 0.0,
            position: (0.0, 0.0),
            color: Paint::solid([255, 255, 255, 255]),
            alignment_horizontal: TextAlignmentHorizontal::Left,
            alignment_vertical: TextAlignmentVertical::Top,
            paint_supersample: true,
        }
    }
}

/// Produces a text raster source.
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
    pub(crate) params: TextParamsDelegate,
    pub(crate) atlas_texture: lumen_gpu::TextureId,
    pub(crate) globals_buffer: lumen_gpu::BufferId,
    pub(crate) instances_buffer: lumen_gpu::BufferId,
    pub(crate) paint_buffer: lumen_gpu::BufferId,
    pub(crate) max_glyphs: usize,
    pub(crate) size: lumen_gpu::Size,
    pub(crate) state: Arc<Mutex<TextState>>,
}

impl GpuCompiledNode for CompiledText {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let trace_started = crate::log_level_enabled(tracing::Level::TRACE).then(trace_now_ms);
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let mut paint = evaluated.color.to_gpu([255, 255, 255, 255]);
        paint.paint_supersample = u32::from(evaluated.paint_supersample);
        let (position_x, position_y) = evaluated.position;
        let request = layout_request(
            evaluated.content,
            evaluated.font_family,
            evaluated.font_size as f32,
            evaluated.font_weight,
            evaluated.font_style,
            evaluated.max_width as f32,
            evaluated.alignment_horizontal,
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
            state.shaped = Some((shape_key.clone(), text_system.shape(&request)));
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
        let frame_key = TextFrameKey {
            shape: shape_key,
            origin_bits: origin.map(f32::to_bits),
            paint: evaluated.color,
            paint_supersample: evaluated.paint_supersample,
            output_width: self.size.width,
            output_height: self.size.height,
        };
        if !state.pending && state.committed.as_ref() == Some(&frame_key) {
            return Ok(());
        }

        let layout = shaped.place(origin, [1.0; 4]);
        let atlas_started = trace_started.map(|_| trace_now_ms());
        let prepared = text_system.prepare_raster_atlas(&mut state.atlas, &layout, self.max_glyphs);
        drop(text_system);
        let atlas_ms = atlas_started.map_or(0.0, |started| trace_now_ms() - started);

        let dropping = prepared.dropped_glyphs > 0;
        if dropping != state.dropping_glyphs {
            let atlas = state.atlas.config();
            if dropping {
                tracing::warn!(
                    target: "lumen_text",
                    node_id = self.node_id.0,
                    dropped_glyphs = prepared.dropped_glyphs,
                    max_glyphs = self.max_glyphs,
                    atlas_width = atlas.width,
                    atlas_height = atlas.height,
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
        let globals = lumen_text::GpuTextGlobals {
            target_size: [self.size.width as f32, self.size.height as f32],
            px_range: state.atlas.config().px_range as f32,
            glyph_count: prepared.instances.len() as u32,
            paint_bounds: layout.bounds,
        };
        bound.write_buffer(self.globals_buffer, 0, bytemuck::bytes_of(&globals));
        bound.write_buffer(self.paint_buffer, 0, bytemuck::bytes_of(&paint));
        if !prepared.instances.is_empty() {
            bound.write_buffer(
                self.instances_buffer,
                0,
                bytemuck::cast_slice(&prepared.instances),
            );
        }

        state.pending = true;
        bound.bind_text(TextBinding {
            state: Arc::clone(&self.state),
            atlas_frame: prepared.frame,
            frame_key,
        });

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

/// Text state owned by one compiled node. Atlas residency and the skipped-frame
/// key only advance once the frame's uploads are applied.
#[derive(Debug)]
pub(crate) struct TextState {
    atlas: lumen_text::RasterAtlas,
    shaped: Option<(TextShapeKey, lumen_text::ShapedText)>,
    pending: bool,
    committed: Option<TextFrameKey>,
    dropping_glyphs: bool,
}

impl TextState {
    pub(crate) fn new(atlas: lumen_text::AtlasConfig) -> Self {
        Self {
            atlas: lumen_text::RasterAtlas::new(atlas),
            shaped: None,
            pending: false,
            committed: None,
            dropping_glyphs: false,
        }
    }
}

#[derive(Debug, Clone)]
pub(crate) struct TextBinding {
    state: Arc<Mutex<TextState>>,
    atlas_frame: u64,
    frame_key: TextFrameKey,
}

impl TextBinding {
    pub(crate) fn commit(&self) -> crate::Result<()> {
        let mut state = self.state.lock().map_err(|_| RenderError::Gpu {
            details: "text state lock was poisoned".to_string(),
        })?;
        state.atlas.commit(self.atlas_frame);
        state.committed = Some(self.frame_key.clone());
        state.pending = false;
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct TextShapeKey {
    content: String,
    font_family: String,
    font_size_bits: u32,
    font_weight: u16,
    font_style: TextFontStyle,
    max_width_bits: u32,
    alignment_horizontal: TextAlignmentHorizontal,
    font_generation: u64,
}

#[derive(Debug, Clone, PartialEq)]
struct TextFrameKey {
    shape: TextShapeKey,
    origin_bits: [u32; 2],
    paint: Paint,
    paint_supersample: bool,
    output_width: u32,
    output_height: u32,
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

    let Some(resolver) = store.get_font_resolver(font_family) else {
        return Ok(());
    };
    let resolver_id = resolver.id().to_string();
    if loaded_fonts()?
        .get(font_family)
        .is_some_and(|loaded_id| loaded_id == &resolver_id)
    {
        return Ok(());
    }
    for data in resolver.data().map_err(LumenError::Media)? {
        text_system.load_font_data(data);
    }
    loaded_fonts()?.insert(font_family.to_string(), resolver_id);
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
