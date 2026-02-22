#[cfg(all(feature = "skia-metal", target_os = "macos"))]
pub mod metal;
pub mod software;
#[cfg(feature = "skia-vulkan")]
pub mod vulkan;

use std::collections::HashMap;

use skia_safe::{
    BlendMode, Canvas, Color, Color4f, ColorType, Data, Font, FontMgr, IPoint, ImageInfo, Paint,
    Point, RRect, Rect, Typeface, canvas::SaveLayerRec, image_filters, images,
    paint::Style as PaintStyle,
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
        ClipPropertyIndex, CompiledClipNode, CompiledClipShadow, CompiledGroupNode,
        CompiledLayerItem, CompiledOperation, CompiledOperationKind, CompiledTimeline,
        CompiledTransform, VideoSourceRef,
    },
    expr::{ExprEvalCtx, ExprProp, Scalar, eval_expr, parse_expr},
    model::{
        ColorRgba, FitMode, LayoutAlignItems, LayoutAlignSelf, LayoutClip, LayoutDisplay,
        LayoutFlexDirection, LayoutJustifyContent, LayoutNode, LayoutNodeKind, LayoutNodeStyle,
        LayoutOverflow, Shape, ShapeClip, TextAlign, TextClip,
    },
};

const EMBEDDED_FONT: &[u8] = include_bytes!("../../../assets/roboto/Roboto-Regular.ttf");

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
                let drew_mask =
                    self.render_mask_layer(timeline, mask, frame, provider, expr_ctx)?;
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

        let mut has_shadow_layer = false;
        if let RenderPass::Content = pass {
            if let Some(shadow) = group.shadow.filter(|shadow| shadow_is_visible(*shadow)) {
                if let Some(filter) = image_filters::drop_shadow(
                    (shadow.offset_x, shadow.offset_y),
                    (shadow.blur_sigma, shadow.blur_sigma),
                    to_sk_color4f(shadow.color),
                    None,
                    None,
                    image_filters::CropRect::default(),
                ) {
                    let mut paint = Paint::default();
                    paint.set_blend_mode(pass.blend_mode());
                    paint.set_image_filter(filter);
                    self.surface
                        .canvas()
                        .save_layer(&SaveLayerRec::default().paint(&paint));
                    has_shadow_layer = true;
                }
            }
        }

        let mut has_simple_mask_clip = false;
        let mut complex_mask = None;
        if let Some(mask) = group.mask.as_deref() {
            if let Some(simple_mask) = self.try_simple_mask(timeline, mask, frame, expr_ctx)? {
                match simple_mask {
                    SimpleMaskResult::Hidden => {
                        if has_shadow_layer {
                            self.surface.canvas().restore();
                        }
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
                let drew_mask =
                    self.render_mask_layer(timeline, mask, frame, provider, expr_ctx)?;
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
        if has_shadow_layer {
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
        if operation.shadow.is_some_and(shadow_is_visible) {
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

        if let RenderPass::Content = pass {
            if let Some(shadow) = operation.shadow.filter(|shadow| shadow_is_visible(*shadow)) {
                if let Some(filter) = image_filters::drop_shadow(
                    (shadow.offset_x, shadow.offset_y),
                    (shadow.blur_sigma, shadow.blur_sigma),
                    to_sk_color4f(shadow.color),
                    None,
                    None,
                    image_filters::CropRect::default(),
                ) {
                    let mut paint = Paint::default();
                    paint.set_blend_mode(blend_mode);
                    paint.set_image_filter(filter);
                    self.surface
                        .canvas()
                        .save_layer(&SaveLayerRec::default().paint(&paint));
                    let drew = self.draw_operation_content(
                        operation,
                        frame,
                        provider,
                        transform,
                        opacity,
                        blend_mode,
                        expr_ctx.static_ctx,
                    )?;
                    self.surface.canvas().restore();
                    return Ok(drew);
                }
            }
        }

        self.draw_operation_content(
            operation,
            frame,
            provider,
            transform,
            opacity,
            blend_mode,
            expr_ctx.static_ctx,
        )
    }

    fn draw_operation_content(
        &mut self,
        operation: &CompiledOperation,
        frame: u64,
        provider: &mut dyn FrameProvider,
        transform: CompiledTransform,
        opacity: f32,
        blend_mode: BlendMode,
        clip_index: &ClipPropertyIndex,
    ) -> Result<bool, RenderError> {
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
            CompiledOperationKind::Layout(layout) => self.draw_layout_operation(
                operation, transform, opacity, layout, provider, blend_mode, clip_index,
            ),
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
        clip_index: &ClipPropertyIndex,
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
                clip_index,
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
                clip_index,
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
                        timeline.clip_index(),
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
