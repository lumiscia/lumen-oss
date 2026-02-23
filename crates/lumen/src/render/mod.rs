pub mod backend;
pub mod context;

use std::collections::HashMap;

use std::sync::Arc;

use skia_safe::{Color, Paint, surfaces};
use thiserror::Error;

use crate::clip::shape::ShapeKind;
use crate::clip::style::{StyleContext, StyleProperty, StyleValue};
use crate::clip::{Clip, ClipType};
use crate::dependency::DependencyPlan;
use crate::expr::{Expression, ExpressionId, ExpressionScope, ExpressionValue};
use crate::render::backend::{RenderError as BackendRenderError, read_surface_rgba};
use crate::render::context::{FrameContext, RendererContext};
use crate::scene::Scene;

pub type ResultMap = HashMap<ExpressionId, ExpressionValue>;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderPipelineStage {
    ValidateFrameRange,
    BuildFrameContext,
    ClearSurface,
    CollectExpressions,
    EvaluateExpressions,
    CompositeLayers,
    ReadSurface,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenderStageBoundary {
    Enter,
    Exit,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RenderStageEvent {
    pub frame: u32,
    pub stage: RenderPipelineStage,
    pub boundary: RenderStageBoundary,
}

pub trait RenderStageObserver: Send + Sync {
    fn on_event(&self, event: RenderStageEvent);
}

#[derive(Clone, Default)]
pub struct RenderStageTracer {
    observer: Option<Arc<dyn RenderStageObserver>>,
}

impl RenderStageTracer {
    pub fn disabled() -> Self {
        Self::default()
    }

    pub fn with_observer(observer: Arc<dyn RenderStageObserver>) -> Self {
        Self {
            observer: Some(observer),
        }
    }

    pub fn stage_scope(&self, frame: u32, stage: RenderPipelineStage) -> RenderStageScope<'_> {
        self.emit(RenderStageEvent {
            frame,
            stage,
            boundary: RenderStageBoundary::Enter,
        });
        RenderStageScope {
            tracer: self,
            frame,
            stage,
        }
    }

    pub fn emit(&self, event: RenderStageEvent) {
        if let Some(observer) = self.observer.as_ref() {
            observer.on_event(event);
        }
    }
}

pub struct RenderStageScope<'a> {
    tracer: &'a RenderStageTracer,
    frame: u32,
    stage: RenderPipelineStage,
}

impl Drop for RenderStageScope<'_> {
    fn drop(&mut self) {
        self.tracer.emit(RenderStageEvent {
            frame: self.frame,
            stage: self.stage,
            boundary: RenderStageBoundary::Exit,
        });
    }
}

#[derive(Debug, Error)]
pub enum RenderError {
    #[error("frame {frame} is out of range for scene duration {duration}")]
    OutOfRange { frame: u32, duration: u32 },
    #[error(transparent)]
    Backend(#[from] BackendRenderError),
}

pub fn render_scene(
    scene: &Scene,
    frame: u32,
    renderer_ctx: &mut RendererContext,
) -> Result<Vec<u8>, RenderError> {
    render_scene_with_tracer(scene, frame, renderer_ctx, &RenderStageTracer::disabled())
}

pub fn render_scene_with_tracer(
    scene: &Scene,
    frame: u32,
    renderer_ctx: &mut RendererContext,
    tracer: &RenderStageTracer,
) -> Result<Vec<u8>, RenderError> {
    {
        let _scope = tracer.stage_scope(frame, RenderPipelineStage::ValidateFrameRange);
        validate_frame_range(scene, frame)?;
    }

    let frame_ctx = {
        let _scope = tracer.stage_scope(frame, RenderPipelineStage::BuildFrameContext);
        build_frame_context(scene, frame)
    };

    {
        let _scope = tracer.stage_scope(frame, RenderPipelineStage::ClearSurface);
        renderer_ctx.clear();
    }

    let expressions = {
        let _scope = tracer.stage_scope(frame, RenderPipelineStage::CollectExpressions);
        collect_expressions(scene)?
    };
    let dependency_plan = DependencyPlan::build(&expressions);
    let expression_scope = build_expression_scope(scene, frame);
    renderer_ctx.set_expression_scope(expression_scope.clone());
    {
        let _scope = tracer.stage_scope(frame, RenderPipelineStage::EvaluateExpressions);
        let _results = evaluate_expressions(&expressions, &dependency_plan, &expression_scope)?;
    }

    {
        let _scope = tracer.stage_scope(frame, RenderPipelineStage::CompositeLayers);
        composite_layers(scene, frame, &frame_ctx, renderer_ctx)?;
    }

    let _scope = tracer.stage_scope(frame, RenderPipelineStage::ReadSurface);
    read_surface_rgba(renderer_ctx).map_err(RenderError::from)
}

fn validate_frame_range(scene: &Scene, frame: u32) -> Result<(), RenderError> {
    if frame >= scene.duration_frames {
        return Err(RenderError::OutOfRange {
            frame,
            duration: scene.duration_frames,
        });
    }

    Ok(())
}

fn build_frame_context(scene: &Scene, frame: u32) -> FrameContext {
    let frame_rate = scene.frame_rate.as_f32();
    let time_seconds = if frame_rate > 0.0 {
        f64::from(frame) / f64::from(frame_rate)
    } else {
        0.0
    };

    FrameContext {
        frame: frame as u64,
        time_seconds,
        width: scene.width,
        height: scene.height,
        device_scale: 1.0,
    }
}

fn collect_expressions(scene: &Scene) -> Result<Vec<Expression>, RenderError> {
    let mut expressions = Vec::new();

    for layer in &scene.layers {
        collect_style_property_expressions(
            &mut expressions,
            format!("layer:{}:opacity", layer.id),
            &layer.opacity,
        )?;
    }

    Ok(expressions)
}

fn collect_style_property_expressions(
    expressions: &mut Vec<Expression>,
    prefix: String,
    property: &StyleProperty<f32>,
) -> Result<(), RenderError> {
    match property {
        StyleProperty::Value(StyleValue::Expression(style_expression)) => {
            expressions.push(
                Expression::parse(ExpressionId(prefix), style_expression.expr.as_str()).map_err(
                    |_| BackendRenderError::Unsupported("failed to parse style expression"),
                )?,
            );
        }
        StyleProperty::Sequence(sequence) => {
            for (index, keyframe) in sequence.keyframes().iter().enumerate() {
                if let StyleValue::Expression(style_expression) = &keyframe.value {
                    expressions.push(
                        Expression::parse(
                            ExpressionId(format!("{prefix}:keyframe:{index}")),
                            style_expression.expr.as_str(),
                        )
                        .map_err(|_| {
                            BackendRenderError::Unsupported("failed to parse style expression")
                        })?,
                    );
                }
            }
        }
        StyleProperty::Value(StyleValue::Literal(_)) => {}
    }

    Ok(())
}

fn evaluate_expressions(
    expressions: &[Expression],
    dependency_plan: &DependencyPlan,
    scope: &ExpressionScope,
) -> Result<ResultMap, RenderError> {
    if expressions.is_empty() {
        return Ok(ResultMap::new());
    }

    let expression_by_id = expressions
        .iter()
        .map(|expression| (expression.id.clone(), expression))
        .collect::<HashMap<_, _>>();
    let mut results = ResultMap::new();

    for expression_id in &dependency_plan.evaluation_order {
        let Some(expression) = expression_by_id.get(expression_id) else {
            continue;
        };
        let value = expression
            .evaluate(scope)
            .map_err(|_| BackendRenderError::Unsupported("failed to evaluate expression"))?;
        results.insert(expression_id.clone(), value);
    }

    Ok(results)
}

fn build_expression_scope(scene: &Scene, frame: u32) -> ExpressionScope {
    let mut scope = ExpressionScope::default();
    let style_ctx = StyleContext::new(frame);

    for layer in &scene.layers {
        for clip in &layer.clips {
            insert_clip_scope_properties(scene, clip, frame, &style_ctx, &mut scope);
        }
    }

    scope
}

fn insert_clip_scope_properties(
    scene: &Scene,
    clip: &ClipType,
    frame: u32,
    style_ctx: &StyleContext<'_>,
    scope: &mut ExpressionScope,
) {
    if !clip.contains_frame(frame) {
        return;
    }

    let Some(clip_id) = clip.id().map(ToOwned::to_owned) else {
        return;
    };

    let scene_width = scene.width as f32;
    let scene_height = scene.height as f32;

    let (x, y, width, height, opacity) = match clip {
        ClipType::Group(group) => (
            0.0,
            0.0,
            scene_width,
            scene_height,
            group.style.opacity.resolve_or(style_ctx, 1.0),
        ),
        ClipType::Layout(layout) => {
            let geometry = layout.geometry.resolve_with_context(
                style_ctx,
                scene_width * 0.05,
                scene_height * 0.05,
                scene_width * 0.9,
                scene_height * 0.9,
                0.0,
                0.0,
            );
            (
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                layout.style.opacity.resolve_or(style_ctx, 1.0),
            )
        }
        ClipType::Image(image) => {
            let geometry = image.geometry.resolve_with_context(
                style_ctx,
                scene_width * 0.1,
                scene_height * 0.1,
                scene_width * 0.4,
                scene_height * 0.3,
                0.0,
                0.0,
            );
            (
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                image.style.opacity.resolve_or(style_ctx, 1.0),
            )
        }
        ClipType::Video(video) => {
            let geometry = video.geometry.resolve_with_context(
                style_ctx,
                scene_width * 0.1,
                scene_height * 0.5,
                scene_width * 0.4,
                scene_height * 0.3,
                0.0,
                0.0,
            );
            (
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                video.style.opacity.resolve_or(style_ctx, 1.0),
            )
        }
        ClipType::Shape(shape) => {
            let geometry = shape.geometry.resolve_with_context(
                style_ctx,
                scene_width * 0.5,
                scene_height * 0.5,
                scene_width * 0.25,
                scene_height * 0.25,
                0.5,
                0.5,
            );
            let opacity = match &shape.kind {
                ShapeKind::Rectangle(style) => style.base.opacity.resolve_or(style_ctx, 1.0),
                ShapeKind::Ellipse(style) => style.base.opacity.resolve_or(style_ctx, 1.0),
                ShapeKind::Polygon(style) => style.base.opacity.resolve_or(style_ctx, 1.0),
            };
            (
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                opacity,
            )
        }
        ClipType::Text(text) => {
            let (default_width, default_height) = text
                .style
                .resolve_placeholder(frame, text.content.as_str())
                .bounds();
            let geometry = text.geometry.resolve_with_context(
                style_ctx,
                scene_width * 0.15,
                scene_height * 0.15,
                default_width,
                default_height,
                0.0,
                0.0,
            );
            (
                geometry.x,
                geometry.y,
                geometry.width,
                geometry.height,
                text.style.base.opacity.resolve_or(style_ctx, 1.0),
            )
        }
    };

    scope.clip_properties.insert(
        (clip_id.clone(), crate::expr::ExpressionProperty::X),
        ExpressionValue::Number(x),
    );
    scope.clip_properties.insert(
        (clip_id.clone(), crate::expr::ExpressionProperty::Y),
        ExpressionValue::Number(y),
    );
    scope.clip_properties.insert(
        (clip_id.clone(), crate::expr::ExpressionProperty::Width),
        ExpressionValue::Number(width),
    );
    scope.clip_properties.insert(
        (clip_id.clone(), crate::expr::ExpressionProperty::Height),
        ExpressionValue::Number(height),
    );
    scope.clip_properties.insert(
        (clip_id, crate::expr::ExpressionProperty::Opacity),
        ExpressionValue::Number(opacity),
    );
}

fn composite_layers(
    scene: &Scene,
    frame: u32,
    frame_ctx: &FrameContext,
    renderer_ctx: &mut RendererContext,
) -> Result<(), RenderError> {
    for layer in &scene.layers {
        if !layer.visible {
            continue;
        }

        let expression_scope = renderer_ctx.expression_scope().clone();
        let style_ctx = StyleContext::with_scope(frame, &expression_scope);
        let opacity = layer.opacity.resolve_or(&style_ctx, 1.0).clamp(0.0, 1.0);
        if opacity <= 0.0 {
            continue;
        }

        let mut layer_surface =
            surfaces::raster_n32_premul((scene.width as i32, scene.height as i32)).ok_or(
                BackendRenderError::Unsupported("failed to create layer surface"),
            )?;
        let mut layer_overlay_surface =
            surfaces::raster_n32_premul((scene.width as i32, scene.height as i32)).ok_or(
                BackendRenderError::Unsupported("failed to create layer surface"),
            )?;
        layer_surface.canvas().clear(Color::from_argb(0, 0, 0, 0));
        layer_overlay_surface
            .canvas()
            .clear(Color::from_argb(0, 0, 0, 0));

        let main_surface = std::mem::replace(&mut renderer_ctx.surface, layer_surface);
        let main_overlay_surface =
            std::mem::replace(&mut renderer_ctx.overlay_surface, layer_overlay_surface);

        for clip in &layer.clips {
            clip.draw(frame, frame_ctx, renderer_ctx)?;
        }

        let mut rendered_layer_surface = std::mem::replace(&mut renderer_ctx.surface, main_surface);
        let _ = std::mem::replace(&mut renderer_ctx.overlay_surface, main_overlay_surface);

        let layer_image = rendered_layer_surface.image_snapshot();
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_alpha_f(opacity);
        paint.set_blend_mode(layer.blend_mode.into());
        renderer_ctx
            .canvas()
            .draw_image(&layer_image, (0.0, 0.0), Some(&paint));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use crate::clip::shape::{ShapeClip, ShapeKind};
    use crate::clip::style::{
        BaseStyle, Fill, Keyframe, RectStyle, Sequence, ShadowStyle, StyleExpression,
        StyleProperty, StyleValue, TransformStyle,
    };
    use crate::clip::{ClipGeometry, ClipMeta, ClipType};
    use crate::render::context::RendererContext;
    use crate::scene::{BlendMode, Layer, Scene};
    use crate::time::Rational;

    use std::sync::{Arc, Mutex};

    use super::{
        RenderError, RenderPipelineStage, RenderStageBoundary, RenderStageEvent,
        RenderStageObserver, RenderStageTracer, render_scene, render_scene_with_tracer,
    };

    fn literal<T>(value: T) -> StyleProperty<T> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    fn base_style() -> BaseStyle {
        BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: skia_safe::BlendMode::SrcOver,
            blur: literal(0.0),
            shadows: Vec::<ShadowStyle>::new(),
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
            mask: None,
        }
    }

    fn solid_clip(id: &str, width: u32, height: u32, color: [u8; 4]) -> ClipType {
        ClipType::Shape(ShapeClip {
            meta: ClipMeta {
                id: Some(id.to_owned()),
                start_frame: 0,
                end_frame: 120,
            },
            geometry: ClipGeometry {
                x: literal(width as f32 * 0.5),
                y: literal(height as f32 * 0.5),
                width: literal(width as f32),
                height: literal(height as f32),
                anchor_x: literal(0.5),
                anchor_y: literal(0.5),
            },
            kind: ShapeKind::Rectangle(RectStyle {
                base: base_style(),
                width: literal(width as f32),
                height: literal(height as f32),
                corner_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
                fill: Some(Fill::Solid {
                    color: [
                        literal(color[0]),
                        literal(color[1]),
                        literal(color[2]),
                        literal(color[3]),
                    ],
                }),
                stroke: None,
            }),
        })
    }

    fn pixel_at(pixels: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let idx = ((y * width + x) * 4) as usize;
        [
            pixels[idx],
            pixels[idx + 1],
            pixels[idx + 2],
            pixels[idx + 3],
        ]
    }

    fn scene_with_overlay(overlay_opacity: StyleProperty<f32>, overlay_visible: bool) -> Scene {
        let width = 4;
        let height = 4;

        Scene {
            width,
            height,
            frame_rate: Rational::new(30, 1),
            duration_frames: 30,
            layers: vec![
                Layer {
                    id: "background".to_owned(),
                    clips: vec![solid_clip("bg", width, height, [255, 0, 0, 255])],
                    blend_mode: BlendMode::Normal,
                    opacity: literal(1.0),
                    visible: true,
                },
                Layer {
                    id: "overlay".to_owned(),
                    clips: vec![solid_clip("fg", width, height, [0, 0, 255, 255])],
                    blend_mode: BlendMode::Normal,
                    opacity: overlay_opacity,
                    visible: overlay_visible,
                },
            ],
        }
    }

    #[derive(Default)]
    struct RecordingObserver {
        events: Mutex<Vec<RenderStageEvent>>,
    }

    impl RenderStageObserver for RecordingObserver {
        fn on_event(&self, event: RenderStageEvent) {
            self.events.lock().expect("lock events").push(event);
        }
    }

    #[test]
    fn stage_scope_emits_enter_and_exit_boundaries() {
        let observer = Arc::new(RecordingObserver::default());
        let tracer = RenderStageTracer::with_observer(observer.clone());

        {
            let _scope = tracer.stage_scope(4, RenderPipelineStage::CompositeLayers);
        }

        let events = observer.events.lock().expect("lock events").clone();
        assert_eq!(
            events,
            vec![
                RenderStageEvent {
                    frame: 4,
                    stage: RenderPipelineStage::CompositeLayers,
                    boundary: RenderStageBoundary::Enter,
                },
                RenderStageEvent {
                    frame: 4,
                    stage: RenderPipelineStage::CompositeLayers,
                    boundary: RenderStageBoundary::Exit,
                },
            ]
        );
    }

    #[test]
    fn render_scene_with_tracer_emits_stage_boundaries() {
        let scene = scene_with_overlay(literal(1.0), true);
        let mut renderer_ctx = RendererContext::new(scene.width, scene.height, scene.frame_rate)
            .expect("renderer context");
        let observer = Arc::new(RecordingObserver::default());
        let tracer = RenderStageTracer::with_observer(observer.clone());

        let _pixels =
            render_scene_with_tracer(&scene, 0, &mut renderer_ctx, &tracer).expect("render frame");

        let events = observer.events.lock().expect("lock events").clone();
        assert_eq!(events.len(), 14);
        assert_eq!(
            events.first(),
            Some(&RenderStageEvent {
                frame: 0,
                stage: RenderPipelineStage::ValidateFrameRange,
                boundary: RenderStageBoundary::Enter,
            })
        );
        assert_eq!(
            events.last(),
            Some(&RenderStageEvent {
                frame: 0,
                stage: RenderPipelineStage::ReadSurface,
                boundary: RenderStageBoundary::Exit,
            })
        );
    }

    #[test]
    fn invisible_layer_contributes_no_pixels() {
        let scene = scene_with_overlay(literal(1.0), false);
        let mut renderer_ctx = RendererContext::new(scene.width, scene.height, scene.frame_rate)
            .expect("renderer context");

        let pixels = render_scene(&scene, 0, &mut renderer_ctx).expect("render frame");

        assert_eq!(pixel_at(&pixels, scene.width, 1, 1), [255, 0, 0, 255]);
    }

    #[test]
    fn blend_mode_normal_at_half_opacity_blends_layers() {
        let scene = scene_with_overlay(literal(0.5), true);
        let mut renderer_ctx = RendererContext::new(scene.width, scene.height, scene.frame_rate)
            .expect("renderer context");

        let pixels = render_scene(&scene, 0, &mut renderer_ctx).expect("render frame");
        let px = pixel_at(&pixels, scene.width, 2, 2);

        assert!((126..=129).contains(&px[0]));
        assert_eq!(px[1], 0);
        assert!((126..=129).contains(&px[2]));
        assert_eq!(px[3], 255);
    }

    #[test]
    fn layer_opacity_expression_can_reference_clip_properties() {
        let opacity = StyleProperty::Value(StyleValue::Expression(StyleExpression::new(
            "clip('bg').opacity * 0.5",
        )));
        let scene = scene_with_overlay(opacity, true);
        let mut renderer_ctx = RendererContext::new(scene.width, scene.height, scene.frame_rate)
            .expect("renderer context");

        let pixels = render_scene(&scene, 0, &mut renderer_ctx).expect("render frame");
        let px = pixel_at(&pixels, scene.width, 0, 0);

        assert!((126..=129).contains(&px[0]));
        assert_eq!(px[1], 0);
        assert!((126..=129).contains(&px[2]));
        assert_eq!(px[3], 255);
    }
    #[test]
    fn layer_opacity_keyframes_animate_across_frames() {
        let opacity = StyleProperty::Sequence(Sequence::new(vec![
            Keyframe::new(0, StyleValue::Literal(0.0f32)),
            Keyframe::new(10, StyleValue::Literal(1.0f32)),
        ]));
        let scene = scene_with_overlay(opacity, true);
        let mut renderer_ctx = RendererContext::new(scene.width, scene.height, scene.frame_rate)
            .expect("renderer context");

        let start = render_scene(&scene, 0, &mut renderer_ctx).expect("render frame 0");
        let mid = render_scene(&scene, 5, &mut renderer_ctx).expect("render frame 5");
        let end = render_scene(&scene, 10, &mut renderer_ctx).expect("render frame 10");

        assert_eq!(pixel_at(&start, scene.width, 0, 0), [255, 0, 0, 255]);

        let mid_px = pixel_at(&mid, scene.width, 0, 0);
        assert!((126..=129).contains(&mid_px[0]));
        assert_eq!(mid_px[1], 0);
        assert!((126..=129).contains(&mid_px[2]));
        assert_eq!(mid_px[3], 255);

        assert_eq!(pixel_at(&end, scene.width, 0, 0), [0, 0, 255, 255]);
    }

    #[test]
    fn out_of_range_frame_returns_error() {
        let mut scene = scene_with_overlay(literal(1.0), true);
        scene.duration_frames = 5;
        let mut renderer_ctx = RendererContext::new(scene.width, scene.height, scene.frame_rate)
            .expect("renderer context");

        let err =
            render_scene(&scene, 5, &mut renderer_ctx).expect_err("frame should be out of range");

        assert!(matches!(
            err,
            RenderError::OutOfRange {
                frame: 5,
                duration: 5
            }
        ));
    }
}
