#[cfg(all(feature = "skia-metal", target_os = "macos"))]
pub mod metal;
pub mod software;
#[cfg(feature = "skia-vulkan")]
pub mod vulkan;

use std::collections::HashMap;

use skia_safe::{
    BlendMode, Canvas, Color, ColorType, Data, Font, FontMgr, IPoint, ImageInfo, Paint, Point,
    RRect, Rect, Typeface, canvas::SaveLayerRec, images, paint::Style as PaintStyle,
};
use taffy::prelude::{
    AlignItems as TaffyAlignItems, AlignSelf as TaffyAlignSelf, AvailableSpace, Dimension,
    Display as TaffyDisplay, FlexDirection as TaffyFlexDirection,
    JustifyContent as TaffyJustifyContent, LengthPercentage as TaffyLengthPercentage,
    LengthPercentageAuto as TaffyLengthPercentageAuto, Rect as TaffyRect, Size as TaffySize,
    Style as TaffyStyle, TaffyTree,
};
use taffy::tree::NodeId as TaffyNodeId;

use crate::{
    backend::{FrameImage, FrameProvider, RenderBackend, RenderError, pixel_len},
    compile::{
        ClipPropertyIndex,
        CompiledClipNode, CompiledGroupNode, CompiledLayerItem, CompiledOperation,
        CompiledOperationKind, CompiledTimeline, CompiledTransform, VideoSourceRef,
    },
    expr::{ExprEvalCtx, ExprProp, Scalar, eval_expr, parse_expr},
    model::{
        ColorRgba, FitMode, LayoutAlignItems, LayoutAlignSelf, LayoutClip, LayoutDisplay,
        LayoutFlexDirection, LayoutJustifyContent, LayoutNode, LayoutNodeKind, LayoutNodeStyle,
        LayoutOverflow,
        Shape, ShapeClip, TextAlign, TextClip,
    },
};

const EMBEDDED_FONT: &[u8] = include_bytes!("../../../assets/roboto/Roboto-Regular.ttf");

// -- GPU backend state (only compiled with a GPU feature) ---------------------

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
#[allow(dead_code)]
enum GpuBackend {
    #[cfg(all(feature = "skia-metal", target_os = "macos"))]
    Metal(metal::MetalState),
    #[cfg(feature = "skia-vulkan")]
    Vulkan(vulkan::VulkanState),
}

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
struct GpuState {
    context: skia_safe::gpu::DirectContext,
    _backend: GpuBackend,
}

// -- SkiaRenderer -------------------------------------------------------------

pub struct SkiaRenderer {
    surface: skia_safe::Surface,
    #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
    gpu: Option<GpuState>,
    typeface: Typeface,
    font_cache: HashMap<u32, Font>,
    layout_cache: HashMap<usize, CachedLayoutClip>,
    width: u32,
    height: u32,
}

#[derive(Debug, Clone)]
struct CachedLayoutClip {
    width: f32,
    height: f32,
    tree: LayoutRenderTree,
}

#[derive(Debug, Clone)]
struct LayoutRenderTree {
    taffy: TaffyTree<()>,
    root: LayoutRenderNode,
    named_layouts: HashMap<String, (f32, f32, f32, f32)>,
}

#[derive(Debug, Clone)]
struct LayoutRenderNode {
    taffy_node: TaffyNodeId,
    style: LayoutNodeStyle,
    kind: LayoutRenderNodeKind,
    has_deferred_dims: bool,
}

#[derive(Debug, Clone)]
enum LayoutRenderNodeKind {
    Container { children: Vec<LayoutRenderNode> },
    Text(LayoutTextRender),
    Image(LayoutImageRender),
}

#[derive(Debug, Clone)]
struct LayoutTextRender {
    lines: Vec<String>,
    line_widths: Vec<f32>,
    font_size: f32,
    line_height: f32,
    color: ColorRgba,
    align: TextAlign,
}

#[derive(Debug, Clone)]
struct LayoutImageRender {
    source: String,
    fit: FitMode,
    corner_radius: f32,
}

#[derive(Debug, Clone, Copy)]
enum RenderPass {
    Content,
    Mask,
}

#[derive(Debug, Clone, Copy)]
enum SimpleMaskResult {
    Hidden,
    Clip(SimpleMaskClip),
}

#[derive(Debug, Clone, Copy)]
enum SimpleMaskClip {
    Rect(RRect),
    Oval(Rect),
}

impl RenderPass {
    fn blend_mode(self) -> BlendMode {
        match self {
            Self::Content => BlendMode::SrcOver,
            Self::Mask => BlendMode::SrcOver,
        }
    }
}

// Safety: SkiaRenderer is used single-threaded; the owner controls access.
// The GPU context and surface are not shared across threads concurrently.
unsafe impl Send for SkiaRenderer {}

// -- LayoutNodeExprCtx --------------------------------------------------------

struct LayoutNodeExprCtx {
    // node_id -> (computed_width, computed_height, computed_x, computed_y)
    layouts: HashMap<String, (f32, f32, f32, f32)>,
}

impl ExprEvalCtx for LayoutNodeExprCtx {
    fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
        let (w, h, x, y) = self.layouts.get(target)?;
        match property {
            ExprProp::Width => Some(*w),
            ExprProp::Height => Some(*h),
            ExprProp::X => Some(*x),
            ExprProp::Y => Some(*y),
        }
    }
}

struct RuntimeExprCtx<'a> {
    static_ctx: &'a ClipPropertyIndex,
    layout_ctx: Option<&'a LayoutNodeExprCtx>,
}

impl ExprEvalCtx for RuntimeExprCtx<'_> {
    fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
        if let Some(layout_ctx) = self.layout_ctx
            && let Some(v) = layout_ctx.resolve(target, property)
        {
            return Some(v);
        }
        self.static_ctx.resolve(target, property)
    }
}

// -- SkiaRenderer impl --------------------------------------------------------

impl SkiaRenderer {
    pub fn new(width: u32, height: u32) -> Result<Self, RenderError> {
        let typeface = load_typeface()?;

        // Try GPU backends (Metal on macOS, Vulkan on Linux), fall back to CPU raster.
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        {
            if let Some((surface, gpu_state)) = try_create_gpu(width, height) {
                return Ok(Self {
                    surface,
                    gpu: Some(gpu_state),
                    typeface,
                    font_cache: HashMap::new(),
                    layout_cache: HashMap::new(),
                    width,
                    height,
                });
            }
        }

        let surface = software::create_surface(width, height)?;

        Ok(Self {
            surface,
            #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
            gpu: None,
            typeface,
            font_cache: HashMap::new(),
            layout_cache: HashMap::new(),
            width,
            height,
        })
    }

    pub fn is_gpu_accelerated(&self) -> bool {
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        {
            return self.gpu.is_some();
        }
        #[cfg(not(any(feature = "skia-metal", feature = "skia-vulkan")))]
        {
            false
        }
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), RenderError> {
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        if let Some(ref mut gpu) = self.gpu {
            self.surface = create_gpu_surface(&mut gpu.context, width, height)?;
            self.width = width;
            self.height = height;
            return Ok(());
        }

        self.surface = software::create_surface(width, height)?;
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn render_layer_item(
        &mut self,
        timeline: &CompiledTimeline,
        item: &CompiledLayerItem,
        frame: u64,
        provider: &mut dyn FrameProvider,
        pass: RenderPass,
        expr_ctx: &RuntimeExprCtx<'_>,
    ) -> Result<bool, RenderError> {
        match item {
            CompiledLayerItem::Clip(clip) => {
                self.render_clip_node(timeline, clip, frame, provider, pass, expr_ctx)
            }
            CompiledLayerItem::Group(group) => {
                self.render_group_node(timeline, group, frame, provider, pass, expr_ctx)
            }
        }
    }

    fn render_clip_node(
        &mut self,
        timeline: &CompiledTimeline,
        clip: &CompiledClipNode,
        frame: u64,
        provider: &mut dyn FrameProvider,
        pass: RenderPass,
        expr_ctx: &RuntimeExprCtx<'_>,
    ) -> Result<bool, RenderError> {
        let operation = timeline
            .operation(clip.operation_index)
            .ok_or(RenderError::MissingOperation(clip.operation_index))?;

        if !operation.contains_frame(frame) {
            return Ok(false);
        }

        if let Some(mask) = clip.mask.as_deref() {
            if let Some(simple_mask) = self.try_simple_mask(timeline, mask, frame, expr_ctx)? {
                return match simple_mask {
                    SimpleMaskResult::Hidden => Ok(false),
                    SimpleMaskResult::Clip(mask_clip) => {
                        self.surface.canvas().save();

                        apply_simple_mask_clip(self.surface.canvas(), mask_clip);
                        let drew_content =
                            self.draw_operation(operation, frame, provider, pass, expr_ctx)?;
                        self.surface.canvas().restore();
                        Ok(drew_content)
                    }
                };
            }
        }

        if clip.mask.is_some() {
            self.surface.canvas().save_layer(&SaveLayerRec::default());
        }

        let drew_content = self.draw_operation(operation, frame, provider, pass, expr_ctx)?;

        let mut drew_output = drew_content;
        if let Some(mask) = clip.mask.as_deref() {
            if !drew_content {
                drew_output = false;
            } else {
                let drew_mask = self.render_mask_layer(timeline, mask, frame, provider, expr_ctx)?;
                if !drew_mask {
                    clear_current_layer(self.surface.canvas());
                    drew_output = false;
                }
            }
            self.surface.canvas().restore();
        }

        Ok(drew_output)
    }

    fn render_group_node(
        &mut self,
        timeline: &CompiledTimeline,
        group: &CompiledGroupNode,
        frame: u64,
        provider: &mut dyn FrameProvider,
        pass: RenderPass,
        expr_ctx: &RuntimeExprCtx<'_>,
    ) -> Result<bool, RenderError> {
        if group.opacity <= 0.0 {
            return Ok(false);
        }

        self.surface.canvas().save();
        self.surface
            .canvas()
            .translate(Point::new(group.transform.x, group.transform.y));
        if group.transform.rotation_degrees != 0.0 {
            self.surface
                .canvas()
                .rotate(group.transform.rotation_degrees, None);
        }

        let mut has_simple_mask_clip = false;
        let mut complex_mask = None;
        if let Some(mask) = group.mask.as_deref() {
            if let Some(simple_mask) = self.try_simple_mask(timeline, mask, frame, expr_ctx)? {
                match simple_mask {
                    SimpleMaskResult::Hidden => {
                        self.surface.canvas().restore();
                        return Ok(false);
                    }
                    SimpleMaskResult::Clip(mask_clip) => {
                        self.surface.canvas().save();

                        apply_simple_mask_clip(self.surface.canvas(), mask_clip);
                        has_simple_mask_clip = true;
                    }
                }
            } else {
                complex_mask = Some(mask);
            }
        }

        let group_opacity = group.opacity.clamp(0.0, 1.0);
        let use_group_layer = !opacity_is_fully_opaque(group_opacity) || complex_mask.is_some();
        if use_group_layer {
            self.surface
                .canvas()
                .save_layer_alpha_f(None, group_opacity);
        }

        let mut drew_any = false;
        for item in &group.items {
            drew_any |= self.render_layer_item(timeline, item, frame, provider, pass, expr_ctx)?;
        }

        let mut drew_output = drew_any;
        if let Some(mask) = complex_mask {
            if !drew_any {
                drew_output = false;
            } else {
                let drew_mask = self.render_mask_layer(timeline, mask, frame, provider, expr_ctx)?;
                if !drew_mask {
                    clear_current_layer(self.surface.canvas());
                    drew_output = false;
                }
            }
        }

        if use_group_layer {
            self.surface.canvas().restore();
        }
        if has_simple_mask_clip {
            self.surface.canvas().restore();
        }
        self.surface.canvas().restore();

        Ok(drew_output)
    }

    fn render_mask_layer(
        &mut self,
        timeline: &CompiledTimeline,
        mask: &CompiledLayerItem,
        frame: u64,
        provider: &mut dyn FrameProvider,
        expr_ctx: &RuntimeExprCtx<'_>,
    ) -> Result<bool, RenderError> {
        let mut paint = Paint::default();
        paint.set_blend_mode(BlendMode::DstIn);

        self.surface
            .canvas()
            .save_layer(&SaveLayerRec::default().paint(&paint));

        let drew_mask =
            self.render_layer_item(timeline, mask, frame, provider, RenderPass::Mask, expr_ctx)?;
        self.surface.canvas().restore();

        Ok(drew_mask)
    }

    fn try_simple_mask(
        &self,
        timeline: &CompiledTimeline,
        mask: &CompiledLayerItem,
        frame: u64,
        expr_ctx: &RuntimeExprCtx<'_>,
    ) -> Result<Option<SimpleMaskResult>, RenderError> {
        let CompiledLayerItem::Clip(mask_clip) = mask else {
            return Ok(None);
        };

        if mask_clip.mask.is_some() {
            return Ok(None);
        }

        let operation = timeline
            .operation(mask_clip.operation_index)
            .ok_or(RenderError::MissingOperation(mask_clip.operation_index))?;
        if !operation.contains_frame(frame) {
            return Ok(Some(SimpleMaskResult::Hidden));
        }

        let opacity = operation.resolved_opacity_with_ctx(frame, expr_ctx);
        if opacity <= 0.0 {
            return Ok(Some(SimpleMaskResult::Hidden));
        }
        if !opacity_is_fully_opaque(opacity) {
            return Ok(None);
        }

        let transform = operation.resolved_transform_with_ctx(frame, expr_ctx);
        if transform.rotation_degrees != 0.0 {
            return Ok(None);
        }

        match &operation.kind {
            CompiledOperationKind::Solid { color } => {
                if color.a() < 255 {
                    return Ok(None);
                }
                let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);
                Ok(Some(SimpleMaskResult::Clip(SimpleMaskClip::Rect(
                    RRect::new_rect_xy(
                        Rect::from_xywh(
                            rect.x as f32,
                            rect.y as f32,
                            rect.width as f32,
                            rect.height as f32,
                        ),
                        0.0,
                        0.0,
                    ),
                ))))
            }
            CompiledOperationKind::Shape(shape) => {
                let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);
                let sk_rect = Rect::from_xywh(
                    rect.x as f32,
                    rect.y as f32,
                    rect.width as f32,
                    rect.height as f32,
                );

                match shape.shape {
                    Shape::Rectangle { fill, radius } => {
                        if fill.a() < 255 {
                            return Ok(None);
                        }

                        let radius = radius.max(0.0);
                        Ok(Some(SimpleMaskResult::Clip(SimpleMaskClip::Rect(
                            RRect::new_rect_xy(sk_rect, radius, radius),
                        ))))
                    }
                    Shape::Ellipse { fill } => {
                        if fill.a() < 255 {
                            return Ok(None);
                        }
                        Ok(Some(SimpleMaskResult::Clip(SimpleMaskClip::Oval(sk_rect))))
                    }
                }
            }
            CompiledOperationKind::Text(_)
            | CompiledOperationKind::Image(_)
            | CompiledOperationKind::Video(_)
            | CompiledOperationKind::Layout(_) => Ok(None),
        }
    }

    fn draw_operation(
        &mut self,
        operation: &CompiledOperation,
        frame: u64,
        provider: &mut dyn FrameProvider,
        pass: RenderPass,
        expr_ctx: &RuntimeExprCtx<'_>,
    ) -> Result<bool, RenderError> {
        let opacity = operation.resolved_opacity_with_ctx(frame, expr_ctx);
        if opacity <= 0.0 {
            return Ok(false);
        }
        let transform = operation.resolved_transform_with_ctx(frame, expr_ctx);
        let blend_mode = pass.blend_mode();

        match &operation.kind {
            CompiledOperationKind::Solid { color } => {
                draw_solid(
                    self.surface.canvas(),
                    transform,
                    opacity,
                    *color,
                    blend_mode,
                );
                Ok(true)
            }
            CompiledOperationKind::Shape(shape) => {
                draw_shape(self.surface.canvas(), transform, opacity, shape, blend_mode);
                Ok(true)
            }
            CompiledOperationKind::Text(text) => {
                draw_text(
                    self.surface.canvas(),
                    &self.typeface,
                    &mut self.font_cache,
                    transform,
                    opacity,
                    text,
                    blend_mode,
                );
                Ok(true)
            }
            CompiledOperationKind::Image(image) => {
                if let Some(frame_image) = provider.image(image.source_id.as_str())? {
                    draw_image(
                        self.surface.canvas(),
                        transform,
                        opacity,
                        image.fit,
                        image.corner_radius,
                        &frame_image,
                        blend_mode,
                    );
                    return Ok(true);
                }
                Ok(false)
            }
            CompiledOperationKind::Video(video) => {
                if let Some(source_frame) = resolve_video_frame(operation, video, frame)? {
                    if let Some(frame_image) =
                        provider.video_frame(video.source_id.as_str(), source_frame)?
                    {
                        draw_image(
                            self.surface.canvas(),
                            transform,
                            opacity,
                            video.fit,
                            video.corner_radius,
                            &frame_image,
                            blend_mode,
                        );
                        return Ok(true);
                    }
                }
                Ok(false)
            }
            CompiledOperationKind::Layout(layout) => self
                .draw_layout_operation(operation, transform, opacity, layout, provider, blend_mode),
        }
    }

    fn draw_layout_operation(
        &mut self,
        operation: &CompiledOperation,
        transform: CompiledTransform,
        opacity: f32,
        layout: &LayoutClip,
        provider: &mut dyn FrameProvider,
        blend_mode: BlendMode,
    ) -> Result<bool, RenderError> {
        let width = transform.width.unwrap_or(1.0).max(1.0);
        let height = transform.height.unwrap_or(1.0).max(1.0);
        let cacheable =
            operation.animation.width.is_empty() && operation.animation.height.is_empty();

        if !cacheable {
            let tree = build_layout_render_tree(
                &self.typeface,
                &mut self.font_cache,
                layout,
                width,
                height,
            )?;
            return draw_layout_render_tree(
                self.surface.canvas(),
                &self.typeface,
                &mut self.font_cache,
                transform,
                opacity,
                blend_mode,
                &tree,
                provider,
            );
        }

        let key = layout_cache_key(operation);
        let should_rebuild = self
            .layout_cache
            .get(&key)
            .map(|cached| !approx_eq(cached.width, width) || !approx_eq(cached.height, height))
            .unwrap_or(true);

        if should_rebuild {
            if self.layout_cache.len() > 256 {
                self.layout_cache.clear();
            }
            let tree = build_layout_render_tree(
                &self.typeface,
                &mut self.font_cache,
                layout,
                width,
                height,
            )?;
            self.layout_cache.insert(
                key,
                CachedLayoutClip {
                    width,
                    height,
                    tree,
                },
            );
        }

        if let Some(cached) = self.layout_cache.get(&key) {
            return draw_layout_render_tree(
                self.surface.canvas(),
                &self.typeface,
                &mut self.font_cache,
                transform,
                opacity,
                blend_mode,
                &cached.tree,
                provider,
            );
        }

        Ok(false)
    }

    fn collect_frame_layout_expr_ctx(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
    ) -> Result<LayoutNodeExprCtx, RenderError> {
        let mut layouts: HashMap<String, (f32, f32, f32, f32)> = HashMap::new();
        let static_expr_ctx = RuntimeExprCtx {
            static_ctx: timeline.clip_index(),
            layout_ctx: None,
        };

        for layer in timeline.layers() {
            for item in &layer.items {
                self.collect_layout_nodes_from_item(
                    timeline,
                    item,
                    frame,
                    &static_expr_ctx,
                    &mut layouts,
                )?;
            }
        }

        Ok(LayoutNodeExprCtx { layouts })
    }

    fn collect_layout_nodes_from_item(
        &mut self,
        timeline: &CompiledTimeline,
        item: &CompiledLayerItem,
        frame: u64,
        expr_ctx: &RuntimeExprCtx<'_>,
        layouts: &mut HashMap<String, (f32, f32, f32, f32)>,
    ) -> Result<(), RenderError> {
        match item {
            CompiledLayerItem::Clip(clip) => {
                let operation = timeline
                    .operation(clip.operation_index)
                    .ok_or(RenderError::MissingOperation(clip.operation_index))?;
                if operation.contains_frame(frame)
                    && let CompiledOperationKind::Layout(layout_clip) = &operation.kind
                {
                    let transform = operation.resolved_transform_with_ctx(frame, expr_ctx);
                    let width = transform.width.unwrap_or(1.0).max(1.0);
                    let height = transform.height.unwrap_or(1.0).max(1.0);
                    let tree = build_layout_render_tree(
                        &self.typeface,
                        &mut self.font_cache,
                        layout_clip,
                        width,
                        height,
                    )?;
                    for (id, values) in &tree.named_layouts {
                        layouts.insert(id.clone(), *values);
                    }
                }
                if let Some(mask) = clip.mask.as_deref() {
                    self.collect_layout_nodes_from_item(timeline, mask, frame, expr_ctx, layouts)?;
                }
            }
            CompiledLayerItem::Group(group) => {
                for child in &group.items {
                    self.collect_layout_nodes_from_item(timeline, child, frame, expr_ctx, layouts)?;
                }
                if let Some(mask) = group.mask.as_deref() {
                    self.collect_layout_nodes_from_item(timeline, mask, frame, expr_ctx, layouts)?;
                }
            }
        }

        Ok(())
    }

    fn readback_rgba(&mut self) -> Result<Vec<u8>, RenderError> {
        self.readback_into(Vec::new())
    }

    fn readback_into(&mut self, mut buffer: Vec<u8>) -> Result<Vec<u8>, RenderError> {
        // Flush GPU work before readback
        #[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
        if let Some(ref mut gpu) = self.gpu {
            gpu.context.flush_and_submit();
        }

        let info = ImageInfo::new(
            (self.width as i32, self.height as i32),
            ColorType::RGBA8888,
            skia_safe::AlphaType::Unpremul,
            None,
        );
        let row_bytes = self.width as usize * 4;
        let required = pixel_len(self.width, self.height)?;
        buffer.resize(required, 0);

        let success = self
            .surface
            .read_pixels(&info, &mut buffer, row_bytes, IPoint::new(0, 0));
        if !success {
            return Err(RenderError::SurfaceCreation("readPixels failed".into()));
        }

        Ok(buffer)
    }
}

impl RenderBackend for SkiaRenderer {
    fn render_frame(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        if frame >= timeline.total_frames() {
            return Err(RenderError::FrameOutOfRange {
                frame,
                total_frames: timeline.total_frames(),
            });
        }

        if self.width != timeline.canvas.width || self.height != timeline.canvas.height {
            self.resize(timeline.canvas.width, timeline.canvas.height)?;
        }

        // Clear with background color
        let bg = timeline.canvas.background;
        self.surface.canvas().clear(to_sk_color(bg, 1.0));

        let layout_expr_ctx = self.collect_frame_layout_expr_ctx(timeline, frame)?;
        let expr_ctx = RuntimeExprCtx {
            static_ctx: timeline.clip_index(),
            layout_ctx: Some(&layout_expr_ctx),
        };

        for layer in timeline.layers() {
            for item in &layer.items {
                let _ = self.render_layer_item(
                    timeline,
                    item,
                    frame,
                    provider,
                    RenderPass::Content,
                    &expr_ctx,
                )?;
            }
        }

        self.readback_rgba()
    }
}

impl SkiaRenderer {
    /// Like `render_frame` but reuses the provided buffer for pixel readback,
    /// avoiding a per-frame allocation.
    pub fn render_frame_reuse(
        &mut self,
        timeline: &CompiledTimeline,
        frame: u64,
        provider: &mut dyn FrameProvider,
        buffer: Vec<u8>,
    ) -> Result<Vec<u8>, RenderError> {
        if frame >= timeline.total_frames() {
            return Err(RenderError::FrameOutOfRange {
                frame,
                total_frames: timeline.total_frames(),
            });
        }

        if self.width != timeline.canvas.width || self.height != timeline.canvas.height {
            self.resize(timeline.canvas.width, timeline.canvas.height)?;
        }

        let bg = timeline.canvas.background;
        self.surface.canvas().clear(to_sk_color(bg, 1.0));

        let layout_expr_ctx = self.collect_frame_layout_expr_ctx(timeline, frame)?;
        let expr_ctx = RuntimeExprCtx {
            static_ctx: timeline.clip_index(),
            layout_ctx: Some(&layout_expr_ctx),
        };

        for layer in timeline.layers() {
            for item in &layer.items {
                let _ = self.render_layer_item(
                    timeline,
                    item,
                    frame,
                    provider,
                    RenderPass::Content,
                    &expr_ctx,
                )?;
            }
        }

        self.readback_into(buffer)
    }
}

// -- GPU context creation (only with GPU features) ----------------------------

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
fn try_create_gpu(width: u32, height: u32) -> Option<(skia_safe::Surface, GpuState)> {
    #[cfg(all(feature = "skia-metal", target_os = "macos"))]
    {
        if let Some(result) = metal::try_create(width, height) {
            return Some(result);
        }
    }

    #[cfg(feature = "skia-vulkan")]
    {
        if let Some(result) = vulkan::try_create(width, height) {
            return Some(result);
        }
    }

    None
}

#[cfg(any(feature = "skia-metal", feature = "skia-vulkan"))]
fn create_gpu_surface(
    context: &mut skia_safe::gpu::DirectContext,
    width: u32,
    height: u32,
) -> Result<skia_safe::Surface, RenderError> {
    use skia_safe::gpu;
    let info = ImageInfo::new_n32_premul((width as i32, height as i32), None);
    gpu::surfaces::render_target(
        context,
        gpu::Budgeted::Yes,
        &info,
        None,
        gpu::SurfaceOrigin::TopLeft,
        None,
        false,
        None,
    )
    .ok_or_else(|| RenderError::SurfaceCreation("failed to create GPU render target".into()))
}

// -- Helpers ------------------------------------------------------------------

fn load_typeface() -> Result<Typeface, RenderError> {
    let font_mgr = FontMgr::new();
    let font_data = Data::new_copy(EMBEDDED_FONT);
    font_mgr
        .new_from_data(&font_data, None)
        .ok_or_else(|| RenderError::Text("failed to load embedded Roboto font".into()))
}

fn resolve_video_frame(
    operation: &crate::compile::CompiledOperation,
    _video: &VideoSourceRef,
    frame: u64,
) -> Result<Option<u64>, RenderError> {
    operation
        .resolve_video_source_frame(frame)
        .map_err(Into::into)
}

fn clear_current_layer(canvas: &Canvas) {
    canvas.draw_color(Color::from_argb(0, 0, 0, 0), BlendMode::Clear);
}

fn apply_simple_mask_clip(canvas: &Canvas, clip: SimpleMaskClip) {
    match clip {
        SimpleMaskClip::Rect(rrect) => {
            canvas.clip_rrect(rrect, None, Some(true));
        }
        SimpleMaskClip::Oval(rect) => {
            let path = skia_safe::Path::oval(rect, None);
            canvas.clip_path(&path, None, Some(true));
        }
    }
}

fn opacity_is_fully_opaque(opacity: f32) -> bool {
    opacity >= (1.0 - f32::EPSILON)
}

#[derive(Debug, Clone, Copy)]
struct ClipBounds {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

fn layout_cache_key(operation: &CompiledOperation) -> usize {
    operation as *const CompiledOperation as usize
}

fn approx_eq(left: f32, right: f32) -> bool {
    (left - right).abs() <= 0.5
}

fn scalar_opt_to_f32(s: &Option<Scalar>) -> Option<f32> {
    match s {
        None => None,
        Some(Scalar::Literal(v)) => Some(*v),
        Some(Scalar::Expr(_)) => None, // deferred — callers handle Expr separately
    }
}

fn build_layout_render_tree(
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    layout: &LayoutClip,
    width: f32,
    height: f32,
) -> Result<LayoutRenderTree, RenderError> {
    let mut taffy = TaffyTree::<()>::new();
    let mut node_id_map: HashMap<String, TaffyNodeId> = HashMap::new();
    let root =
        build_layout_render_node(&mut taffy, typeface, font_cache, &layout.root, &mut node_id_map)?;

    let available = TaffySize {
        width: AvailableSpace::Definite(width),
        height: AvailableSpace::Definite(height),
    };
    taffy
        .compute_layout(root.taffy_node, available)
        .map_err(|err| RenderError::SurfaceCreation(format!("taffy compute failed: {err}")))?;

    // Second pass if any nodes have deferred dimension exprs
    let first_pass_layouts = collect_named_layouts(&taffy, &node_id_map);
    let ctx = LayoutNodeExprCtx {
        layouts: first_pass_layouts,
    };
    let any_changed = apply_deferred_layout_dims(&mut taffy, &root, &ctx);
    if any_changed {
        taffy
            .compute_layout(root.taffy_node, available)
            .map_err(|err| RenderError::SurfaceCreation(format!("taffy recompute failed: {err}")))?;
    }

    let named_layouts = collect_named_layouts(&taffy, &node_id_map);

    Ok(LayoutRenderTree {
        taffy,
        root,
        named_layouts,
    })
}

fn collect_named_layouts(
    taffy: &TaffyTree<()>,
    node_id_map: &HashMap<String, TaffyNodeId>,
) -> HashMap<String, (f32, f32, f32, f32)> {
    let mut named_layouts: HashMap<String, (f32, f32, f32, f32)> = HashMap::new();
    for (id, taffy_id) in node_id_map {
        if let Ok(lay) = taffy.layout(*taffy_id) {
            named_layouts.insert(
                id.clone(),
                (lay.size.width, lay.size.height, lay.location.x, lay.location.y),
            );
        }
    }
    named_layouts
}

fn build_layout_render_node(
    taffy: &mut TaffyTree<()>,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    node: &LayoutNode,
    node_id_map: &mut HashMap<String, TaffyNodeId>,
) -> Result<LayoutRenderNode, RenderError> {
    let base_style = layout_style_to_taffy(&node.style);

    let has_deferred_dims = [
        &node.style.width,
        &node.style.height,
        &node.style.min_width,
        &node.style.min_height,
        &node.style.max_width,
        &node.style.max_height,
    ]
    .iter()
    .any(|s| matches!(s, Some(Scalar::Expr(_))));

    match &node.kind {
        LayoutNodeKind::Container { children } => {
            let mut rendered_children = Vec::with_capacity(children.len());
            let mut child_nodes = Vec::with_capacity(children.len());
            for child in children {
                let rendered_child =
                    build_layout_render_node(taffy, typeface, font_cache, child, node_id_map)?;
                child_nodes.push(rendered_child.taffy_node);
                rendered_children.push(rendered_child);
            }
            let taffy_node = taffy
                .new_with_children(base_style, &child_nodes)
                .map_err(|err| RenderError::SurfaceCreation(format!("taffy node failed: {err}")))?;
            if let Some(id) = &node.id {
                node_id_map.insert(id.clone(), taffy_node);
            }
            Ok(LayoutRenderNode {
                taffy_node,
                style: node.style.clone(),
                has_deferred_dims,
                kind: LayoutRenderNodeKind::Container {
                    children: rendered_children,
                },
            })
        }
        LayoutNodeKind::Text(text_node) => {
            let measured = measure_layout_text_block(typeface, font_cache, text_node, &node.style);
            let width = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.width),
                scalar_opt_to_f32(&node.style.min_width),
                scalar_opt_to_f32(&node.style.max_width),
                measured.width,
            );
            let height = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.height),
                scalar_opt_to_f32(&node.style.min_height),
                scalar_opt_to_f32(&node.style.max_height),
                measured.height,
            );

            let mut leaf_style = base_style;
            leaf_style.size = TaffySize {
                width: Dimension::length(width),
                height: Dimension::length(height),
            };

            let taffy_node = taffy
                .new_leaf(leaf_style)
                .map_err(|err| RenderError::SurfaceCreation(format!("taffy leaf failed: {err}")))?;
            if let Some(id) = &node.id {
                node_id_map.insert(id.clone(), taffy_node);
            }
            Ok(LayoutRenderNode {
                taffy_node,
                style: node.style.clone(),
                has_deferred_dims,
                kind: LayoutRenderNodeKind::Text(LayoutTextRender {
                    lines: measured.lines,
                    line_widths: measured.line_widths,
                    font_size: text_node.font_size.max(1.0),
                    line_height: measured.line_height,
                    color: text_node.color,
                    align: text_node.align,
                }),
            })
        }
        LayoutNodeKind::Image(image_node) => {
            let intrinsic_width = scalar_opt_to_f32(&node.style.width)
                .or_else(|| scalar_opt_to_f32(&node.style.max_width))
                .or_else(|| scalar_opt_to_f32(&node.style.min_width))
                .unwrap_or(1.0)
                .max(1.0);
            let intrinsic_height = scalar_opt_to_f32(&node.style.height)
                .or_else(|| scalar_opt_to_f32(&node.style.max_height))
                .or_else(|| scalar_opt_to_f32(&node.style.min_height))
                .unwrap_or(1.0)
                .max(1.0);
            let width = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.width),
                scalar_opt_to_f32(&node.style.min_width),
                scalar_opt_to_f32(&node.style.max_width),
                intrinsic_width,
            );
            let height = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.height),
                scalar_opt_to_f32(&node.style.min_height),
                scalar_opt_to_f32(&node.style.max_height),
                intrinsic_height,
            );

            let mut leaf_style = base_style;
            leaf_style.size = TaffySize {
                width: Dimension::length(width),
                height: Dimension::length(height),
            };
            let taffy_node = taffy
                .new_leaf(leaf_style)
                .map_err(|err| RenderError::SurfaceCreation(format!("taffy leaf failed: {err}")))?;
            if let Some(id) = &node.id {
                node_id_map.insert(id.clone(), taffy_node);
            }
            Ok(LayoutRenderNode {
                taffy_node,
                style: node.style.clone(),
                has_deferred_dims,
                kind: LayoutRenderNodeKind::Image(LayoutImageRender {
                    source: image_node.source.clone(),
                    fit: image_node.fit,
                    corner_radius: image_node.corner_radius,
                }),
            })
        }
    }
}

fn apply_deferred_layout_dims(
    taffy: &mut TaffyTree<()>,
    node: &LayoutRenderNode,
    ctx: &LayoutNodeExprCtx,
) -> bool {
    let mut any_changed = false;

    // Recurse into children first
    if let LayoutRenderNodeKind::Container { children } = &node.kind {
        for child in children {
            any_changed |= apply_deferred_layout_dims(taffy, child, ctx);
        }
    }

    if !node.has_deferred_dims {
        return any_changed;
    }

    let Ok(current_style) = taffy.style(node.taffy_node) else {
        return any_changed;
    };
    let mut updated_style = current_style.clone();
    let mut node_changed = false;

    // Resolve a single Scalar::Expr field against ctx
    let try_resolve = |s: &Option<Scalar>| -> Option<f32> {
        match s {
            Some(Scalar::Expr(expr_str)) => {
                let parsed = parse_expr(expr_str.as_str()).ok()?;
                eval_expr(&parsed, ctx).ok()
            }
            _ => None,
        }
    };

    // Helper: check if a Dimension is significantly different from a resolved f32.
    // Uses resolve_to_option since Dimension is a newtype (taffy 0.9), not an enum.
    let is_significant_change = |dim: Dimension, v: f32| -> bool {
        match dim.into_option() {
            Some(x) => (x - v).abs() > 0.5,
            None => true, // auto → any concrete value is a change
        }
    };

    // size.width
    if let Some(v) = try_resolve(&node.style.width) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.size.width, v) {
            updated_style.size.width = new_dim;
            node_changed = true;
        }
    }
    // size.height
    if let Some(v) = try_resolve(&node.style.height) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.size.height, v) {
            updated_style.size.height = new_dim;
            node_changed = true;
        }
    }
    // min_size.width
    if let Some(v) = try_resolve(&node.style.min_width) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.min_size.width, v) {
            updated_style.min_size.width = new_dim;
            node_changed = true;
        }
    }
    // min_size.height
    if let Some(v) = try_resolve(&node.style.min_height) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.min_size.height, v) {
            updated_style.min_size.height = new_dim;
            node_changed = true;
        }
    }
    // max_size.width
    if let Some(v) = try_resolve(&node.style.max_width) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.max_size.width, v) {
            updated_style.max_size.width = new_dim;
            node_changed = true;
        }
    }
    // max_size.height
    if let Some(v) = try_resolve(&node.style.max_height) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.max_size.height, v) {
            updated_style.max_size.height = new_dim;
            node_changed = true;
        }
    }

    if node_changed {
        let _ = taffy.set_style(node.taffy_node, updated_style);
        any_changed = true;
    }

    any_changed
}

fn draw_layout_render_tree(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    transform: CompiledTransform,
    opacity: f32,
    blend_mode: BlendMode,
    tree: &LayoutRenderTree,
    provider: &mut dyn FrameProvider,
) -> Result<bool, RenderError> {
    let target_width = transform.width.unwrap_or(1.0).max(1.0);
    let target_height = transform.height.unwrap_or(1.0).max(1.0);
    let clip_bounds = ClipBounds {
        left: transform.x,
        top: transform.y,
        right: transform.x + target_width,
        bottom: transform.y + target_height,
    };

    if transform.rotation_degrees != 0.0 {
        canvas.save();
        let cx = transform.x + target_width * 0.5;
        let cy = transform.y + target_height * 0.5;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    let drew = draw_layout_render_node(
        canvas,
        typeface,
        font_cache,
        tree,
        &tree.root,
        transform.x,
        transform.y,
        opacity,
        blend_mode,
        provider,
        &clip_bounds,
    )?;

    if transform.rotation_degrees != 0.0 {
        canvas.restore();
    }

    Ok(drew)
}

#[allow(clippy::too_many_arguments)]
fn draw_layout_render_node(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    tree: &LayoutRenderTree,
    node: &LayoutRenderNode,
    origin_x: f32,
    origin_y: f32,
    opacity: f32,
    blend_mode: BlendMode,
    provider: &mut dyn FrameProvider,
    clip_bounds: &ClipBounds,
) -> Result<bool, RenderError> {
    let layout = tree
        .taffy
        .layout(node.taffy_node)
        .map_err(|err| RenderError::SurfaceCreation(format!("taffy layout failed: {err}")))?;
    let x = origin_x + layout.location.x;
    let y = origin_y + layout.location.y;
    let width = layout.size.width.max(0.0);
    let height = layout.size.height.max(0.0);
    if width <= 0.0 || height <= 0.0 {
        return Ok(false);
    }
    if x >= clip_bounds.right
        || y >= clip_bounds.bottom
        || x + width <= clip_bounds.left
        || y + height <= clip_bounds.top
    {
        return Ok(false);
    }

    let mut drew_any = draw_layout_background(
        canvas,
        &node.style,
        x,
        y,
        width,
        height,
        opacity,
        blend_mode,
    );


    let mut clipped_children = false;
    if node.style.overflow == LayoutOverflow::Hidden {
        if matches!(node.kind, LayoutRenderNodeKind::Container { .. }) {
            canvas.save();
            if node.style.corner_radius > 0.0 {
                let radius = node.style.corner_radius.min(width.min(height) * 0.5);
                let rrect = RRect::new_rect_xy(Rect::from_xywh(x, y, width, height), radius, radius);
                canvas.clip_rrect(rrect, None, Some(true));
            } else {
                canvas.clip_rect(Rect::from_xywh(x, y, width, height), None, Some(true));
            }
            clipped_children = true;
        }
    }
    match &node.kind {
        LayoutRenderNodeKind::Container { children } => {
            for child in children {
                drew_any |= draw_layout_render_node(
                    canvas, typeface, font_cache, tree, child, x, y, opacity,
                    blend_mode, provider, clip_bounds,
                )?;
            }
        }
        LayoutRenderNodeKind::Text(text) => {
            drew_any |= draw_layout_text_lines(
                canvas, typeface, font_cache, x, y, width, opacity, blend_mode, text,
            );
        }
        LayoutRenderNodeKind::Image(image) => {
            if let Some(frame_image) = provider.image(image.source.as_str())? {
                draw_image(
                    canvas,
                    CompiledTransform {
                        x,
                        y,
                        width: Some(width.max(1.0)),
                        height: Some(height.max(1.0)),
                        rotation_degrees: 0.0,
                    },
                    opacity,
                    image.fit,
                    image.corner_radius,
                    &frame_image,
                    blend_mode,
                );
                drew_any = true;
            }
        }
    }

    if clipped_children {
        canvas.restore();
    }

    Ok(drew_any)
}

fn draw_layout_background(
    canvas: &Canvas,
    style: &LayoutNodeStyle,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    opacity: f32,
    blend_mode: BlendMode,
) -> bool {
    let Some(background) = style.background else {
        return false;
    };
    if width <= 0.0 || height <= 0.0 {
        return false;
    }

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(background, opacity));
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    paint.set_blend_mode(blend_mode);

    if style.corner_radius > 0.0 {
        let radius = style.corner_radius.min(width.min(height) * 0.5);
        let rrect = RRect::new_rect_xy(Rect::from_xywh(x, y, width, height), radius, radius);
        canvas.draw_rrect(rrect, &paint);
    } else {
        canvas.draw_rect(Rect::from_xywh(x, y, width, height), &paint);
    }

    background.a() > 0 && opacity > 0.0
}

fn draw_layout_text_lines(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    x: f32,
    y: f32,
    width: f32,
    opacity: f32,
    blend_mode: BlendMode,
    text: &LayoutTextRender,
) -> bool {
    if width <= 0.0 {
        return false;
    }
    let font = font_cache
        .entry(text.font_size.to_bits())
        .or_insert_with(|| Font::from_typeface(typeface, text.font_size.max(1.0)));

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(text.color, opacity));
    paint.set_anti_alias(true);
    paint.set_blend_mode(blend_mode);

    let mut y_cursor = y;
    for (line, line_width) in text.lines.iter().zip(text.line_widths.iter().copied()) {
        let line_x = match text.align {
            TextAlign::Left => x,
            TextAlign::Center => x + (width - line_width) * 0.5,
            TextAlign::Right => x + (width - line_width),
        };
        canvas.draw_str(
            line.as_str(),
            Point::new(line_x, y_cursor + text.font_size),
            font,
            &paint,
        );
        y_cursor += text.line_height;
    }

    !text.lines.is_empty() && text.color.a() > 0 && opacity > 0.0
}

fn layout_style_to_taffy(style: &LayoutNodeStyle) -> TaffyStyle {
    let mut taffy = TaffyStyle::default();
    taffy.display = match style.display {
        LayoutDisplay::Flex => TaffyDisplay::Flex,
        LayoutDisplay::None => TaffyDisplay::None,
    };
    taffy.flex_direction = match style.flex_direction {
        LayoutFlexDirection::Row => TaffyFlexDirection::Row,
        LayoutFlexDirection::Column => TaffyFlexDirection::Column,
    };
    taffy.justify_content = Some(match style.justify_content {
        LayoutJustifyContent::FlexStart => TaffyJustifyContent::FlexStart,
        LayoutJustifyContent::Center => TaffyJustifyContent::Center,
        LayoutJustifyContent::FlexEnd => TaffyJustifyContent::FlexEnd,
        LayoutJustifyContent::SpaceBetween => TaffyJustifyContent::SpaceBetween,
        LayoutJustifyContent::SpaceAround => TaffyJustifyContent::SpaceAround,
        LayoutJustifyContent::SpaceEvenly => TaffyJustifyContent::SpaceEvenly,
    });
    taffy.align_items = Some(match style.align_items {
        LayoutAlignItems::Stretch => TaffyAlignItems::Stretch,
        LayoutAlignItems::FlexStart => TaffyAlignItems::FlexStart,
        LayoutAlignItems::Center => TaffyAlignItems::Center,
        LayoutAlignItems::FlexEnd => TaffyAlignItems::FlexEnd,
    });
    taffy.align_self = match style.align_self {
        LayoutAlignSelf::Auto => None,
        LayoutAlignSelf::Stretch => Some(TaffyAlignSelf::Stretch),
        LayoutAlignSelf::FlexStart => Some(TaffyAlignSelf::FlexStart),
        LayoutAlignSelf::Center => Some(TaffyAlignSelf::Center),
        LayoutAlignSelf::FlexEnd => Some(TaffyAlignSelf::FlexEnd),
    };
    taffy.flex_grow = style.flex_grow;
    taffy.flex_shrink = style.flex_shrink;
    taffy.size = TaffySize {
        width: to_taffy_dimension(&style.width),
        height: to_taffy_dimension(&style.height),
    };
    taffy.min_size = TaffySize {
        width: to_taffy_dimension(&style.min_width),
        height: to_taffy_dimension(&style.min_height),
    };
    taffy.max_size = TaffySize {
        width: to_taffy_dimension(&style.max_width),
        height: to_taffy_dimension(&style.max_height),
    };
    taffy.padding = TaffyRect {
        left: TaffyLengthPercentage::length(style.padding.left),
        right: TaffyLengthPercentage::length(style.padding.right),
        top: TaffyLengthPercentage::length(style.padding.top),
        bottom: TaffyLengthPercentage::length(style.padding.bottom),
    };
    taffy.margin = TaffyRect {
        left: TaffyLengthPercentageAuto::length(style.margin.left),
        right: TaffyLengthPercentageAuto::length(style.margin.right),
        top: TaffyLengthPercentageAuto::length(style.margin.top),
        bottom: TaffyLengthPercentageAuto::length(style.margin.bottom),
    };
    taffy.gap = TaffySize {
        width: TaffyLengthPercentage::length(style.gap),
        height: TaffyLengthPercentage::length(style.gap),
    };
    taffy
}

fn to_taffy_dimension(value: &Option<Scalar>) -> Dimension {
    match value {
        None => Dimension::auto(),
        Some(Scalar::Literal(v)) => Dimension::length(v.max(0.0)),
        Some(Scalar::Expr(_)) => Dimension::auto(), // deferred, first-pass uses auto
    }
}

fn resolve_layout_dimension(
    preferred: Option<f32>,
    min: Option<f32>,
    max: Option<f32>,
    fallback: f32,
) -> f32 {
    let mut resolved = preferred.unwrap_or(fallback).max(0.0);
    if let Some(min) = min {
        resolved = resolved.max(min.max(0.0));
    }
    if let Some(max) = max {
        resolved = resolved.min(max.max(0.0));
    }
    resolved.max(1.0)
}

#[derive(Debug, Clone)]
struct MeasuredLayoutTextBlock {
    lines: Vec<String>,
    line_widths: Vec<f32>,
    width: f32,
    height: f32,
    line_height: f32,
}

fn measure_layout_text_block(
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    text_node: &crate::model::LayoutTextNode,
    style: &LayoutNodeStyle,
) -> MeasuredLayoutTextBlock {
    let font_size = text_node.font_size.max(1.0);
    let font = font_cache
        .entry(font_size.to_bits())
        .or_insert_with(|| Font::from_typeface(typeface, font_size));

    let (_, metrics) = font.metrics();
    let default_line_height = (metrics.descent - metrics.ascent + metrics.leading).max(font_size);
    let line_height = text_node
        .line_height
        .unwrap_or(default_line_height)
        .max(1.0);
    let wrap_width = scalar_opt_to_f32(&style.width)
        .or_else(|| scalar_opt_to_f32(&style.max_width))
        .map(|value| value.max(1.0));
    let lines = wrap_text_for_layout(font, text_node.text.as_str(), wrap_width);
    let line_widths = lines
        .iter()
        .map(|line| measure_font_width(font, line.as_str()))
        .collect::<Vec<_>>();
    let max_line_width = line_widths.iter().copied().fold(0.0_f32, f32::max).max(1.0);
    let width = wrap_width
        .map(|limit| max_line_width.min(limit))
        .unwrap_or(max_line_width)
        .max(1.0);
    let height = (line_height * lines.len().max(1) as f32).max(line_height);

    MeasuredLayoutTextBlock {
        lines,
        line_widths,
        width,
        height,
        line_height,
    }
}

fn wrap_text_for_layout(font: &Font, text: &str, max_width: Option<f32>) -> Vec<String> {
    let Some(max_width) = max_width else {
        let lines: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
        return if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
    };

    let mut wrapped: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        wrapped.extend(wrap_layout_paragraph(font, paragraph, max_width));
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
}

fn wrap_layout_paragraph(font: &Font, paragraph: &str, max_width: f32) -> Vec<String> {
    let words: Vec<&str> = paragraph
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in words {
        if current.is_empty() {
            if measure_font_width(font, word) <= max_width {
                current.push_str(word);
                continue;
            }
            lines.extend(hard_break_word(font, word, max_width));
            continue;
        }

        let candidate = format!("{current} {word}");
        if measure_font_width(font, candidate.as_str()) <= max_width {
            current = candidate;
            continue;
        }

        lines.push(std::mem::take(&mut current));
        if measure_font_width(font, word) <= max_width {
            current = word.to_string();
            continue;
        }
        lines.extend(hard_break_word(font, word, max_width));
        current.clear();
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn hard_break_word(font: &Font, word: &str, max_width: f32) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for character in word.chars() {
        let candidate = format!("{current}{character}");
        if !current.is_empty() && measure_font_width(font, candidate.as_str()) > max_width {
            tokens.push(current);
            current = character.to_string();
            continue;
        }
        if current.is_empty() && measure_font_width(font, candidate.as_str()) > max_width {
            tokens.push(character.to_string());
            continue;
        }
        current.push(character);
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    if tokens.is_empty() {
        tokens.push(word.to_string());
    }
    tokens
}

fn measure_font_width(font: &Font, text: &str) -> f32 {
    let (width, _) = font.measure_str(text, None);
    width
}

// -- Drawing primitives -------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct LayoutRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn layout_rect(
    transform: CompiledTransform,
    source_width: f64,
    source_height: f64,
    fit: FitMode,
) -> LayoutRect {
    let target_width = transform
        .width
        .map(|v| v as f64)
        .unwrap_or(source_width)
        .max(1.0);
    let target_height = transform
        .height
        .map(|v| v as f64)
        .unwrap_or(source_height)
        .max(1.0);

    let (draw_width, draw_height) = match fit {
        FitMode::Fill => (target_width, target_height),
        FitMode::Contain => {
            let scale = (target_width / source_width).min(target_height / source_height);
            (source_width * scale, source_height * scale)
        }
        FitMode::Cover => {
            let scale = (target_width / source_width).max(target_height / source_height);
            (source_width * scale, source_height * scale)
        }
    };

    let x = transform.x as f64 + (target_width - draw_width) / 2.0;
    let y = transform.y as f64 + (target_height - draw_height) / 2.0;

    LayoutRect {
        x,
        y,
        width: draw_width,
        height: draw_height,
    }
}

fn alpha_scaled(alpha: u8, opacity: f32) -> u8 {
    ((alpha as f32) * opacity).round() as u8
}

fn to_sk_color(c: ColorRgba, opacity: f32) -> Color {
    Color::from_argb(alpha_scaled(c.a(), opacity), c.r(), c.g(), c.b())
}

fn draw_solid(
    canvas: &Canvas,
    transform: CompiledTransform,
    opacity: f32,
    color: ColorRgba,
    blend_mode: BlendMode,
) {
    let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(color, opacity));
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    paint.set_blend_mode(blend_mode);

    canvas.draw_rect(
        Rect::from_xywh(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
        &paint,
    );
}

fn draw_shape(
    canvas: &Canvas,
    transform: CompiledTransform,
    opacity: f32,
    shape: &ShapeClip,
    blend_mode: BlendMode,
) {
    let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);
    let sk_rect = Rect::from_xywh(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );

    match shape.shape {
        Shape::Rectangle { fill, radius } => {
            let mut paint = Paint::default();
            paint.set_color(to_sk_color(fill, opacity));
            paint.set_anti_alias(true);
            paint.set_style(PaintStyle::Fill);
            paint.set_blend_mode(blend_mode);

            if radius > 0.0 {
                let rrect = RRect::new_rect_xy(sk_rect, radius, radius);
                canvas.draw_rrect(rrect, &paint);
            } else {
                canvas.draw_rect(sk_rect, &paint);
            }
        }
        Shape::Ellipse { fill } => {
            let mut paint = Paint::default();
            paint.set_color(to_sk_color(fill, opacity));
            paint.set_anti_alias(true);
            paint.set_style(PaintStyle::Fill);
            paint.set_blend_mode(blend_mode);

            canvas.draw_oval(sk_rect, &paint);
        }
    }
}

fn draw_text(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    transform: CompiledTransform,
    opacity: f32,
    text: &TextClip,
    blend_mode: BlendMode,
) {
    let font_size = text.font_size.max(1.0);
    let font = font_cache
        .entry(font_size.to_bits())
        .or_insert_with(|| Font::from_typeface(typeface, font_size));

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(text.color, opacity));
    paint.set_anti_alias(true);
    paint.set_blend_mode(blend_mode);

    let (_, metrics) = font.metrics();
    let line_height = (metrics.descent - metrics.ascent + metrics.leading).max(font_size);

    let lines: Vec<&str> = text.text.lines().collect();
    let lines = if lines.is_empty() { vec![""] } else { lines };

    let line_widths: Vec<f32> = lines
        .iter()
        .map(|line| {
            let (width, _) = font.measure_str(line, None);
            width
        })
        .collect();

    let width_max = line_widths.iter().cloned().fold(0.0f32, f32::max);
    let target_width = transform.width.unwrap_or(width_max.max(1.0));

    let mut y_cursor = transform.y;
    let has_rotation = transform.rotation_degrees != 0.0;

    if has_rotation {
        canvas.save();
        let cx = transform.x + target_width / 2.0;
        let cy = transform.y;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    for (i, line) in lines.iter().enumerate() {
        let x = match text.align {
            TextAlign::Left => transform.x,
            TextAlign::Center => transform.x + (target_width - line_widths[i]) / 2.0,
            TextAlign::Right => transform.x + (target_width - line_widths[i]),
        };

        canvas.draw_str(line, Point::new(x, y_cursor + font_size), font, &paint);
        y_cursor += line_height;
    }

    if has_rotation {
        canvas.restore();
    }
}

fn draw_image(
    canvas: &Canvas,
    transform: CompiledTransform,
    opacity: f32,
    fit: FitMode,
    corner_radius: f32,
    image: &FrameImage,
    blend_mode: BlendMode,
) {
    let info = ImageInfo::new(
        (image.width as i32, image.height as i32),
        ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );

    let row_bytes = image.width as usize * 4;
    // SAFETY: `image.rgba` is borrowed from the FrameProvider and outlives
    // both the `Data` and the `sk_image` created from it – they are local
    // to this function and dropped before it returns.
    let data = unsafe { Data::new_bytes(&image.rgba) };
    let sk_image = match images::raster_from_data(&info, data, row_bytes) {
        Some(img) => img,
        None => return,
    };

    let rect = layout_rect(transform, image.width as f64, image.height as f64, fit);
    let dst_rect = Rect::from_xywh(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );

    let mut paint = Paint::default();
    paint.set_alpha_f(opacity);
    paint.set_blend_mode(blend_mode);

    // Fast path: the common case (video/image layers) doesn't need a local
    // canvas transform or clip stack.
    if transform.rotation_degrees == 0.0 && corner_radius <= 0.0 {
        canvas.draw_image_rect(&sk_image, None, dst_rect, &paint);
        return;
    }

    let scale_x = rect.width as f32 / image.width as f32;
    let scale_y = rect.height as f32 / image.height as f32;

    canvas.save();

    if transform.rotation_degrees != 0.0 {
        let cx = rect.x as f32 + rect.width as f32 / 2.0;
        let cy = rect.y as f32 + rect.height as f32 / 2.0;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    canvas.translate(Point::new(rect.x as f32, rect.y as f32));
    canvas.scale((scale_x, scale_y));

    if corner_radius > 0.0 {
        let min_scale = scale_x.abs().min(scale_y.abs()).max(f32::EPSILON);
        let source_radius =
            (corner_radius / min_scale).min((image.width.min(image.height) as f32) * 0.5);
        let clip = RRect::new_rect_xy(
            Rect::from_xywh(0.0, 0.0, image.width as f32, image.height as f32),
            source_radius,
            source_radius,
        );
        canvas.clip_rrect(clip, None, Some(true));
    }

    canvas.draw_image(&sk_image, Point::new(0.0, 0.0), Some(&paint));
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_fit_keeps_aspect_ratio() {
        let rect = layout_rect(
            CompiledTransform {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                height: Some(100.0),
                rotation_degrees: 0.0,
            },
            100.0,
            100.0,
            FitMode::Contain,
        );
        assert_eq!(rect.width, 100.0);
        assert_eq!(rect.height, 100.0);
        assert_eq!(rect.x, 50.0);
    }

    #[test]
    fn cover_fit_expands_aspect_ratio() {
        let rect = layout_rect(
            CompiledTransform {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                height: Some(100.0),
                rotation_degrees: 0.0,
            },
            100.0,
            100.0,
            FitMode::Cover,
        );
        assert_eq!(rect.width, 200.0);
        assert_eq!(rect.height, 200.0);
        assert_eq!(rect.y, -50.0);
    }

    #[test]
    fn renderer_creates_successfully() {
        let renderer = SkiaRenderer::new(320, 240);
        assert!(renderer.is_ok());
    }
}
