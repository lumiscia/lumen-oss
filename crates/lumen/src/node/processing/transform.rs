use skia_safe::{FilterMode, Matrix, Point, Rect, SamplingOptions};

use crate::{
    gpu_image::{GpuImageFrame, RectI},
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum TransformSampling {
    Nearest = 0,
    Linear = 1,
}

impl TransformSampling {
    fn from_int(value: i64) -> Self {
        match value {
            x if x == Self::Nearest as i64 => Self::Nearest,
            _ => Self::Linear,
        }
    }
}

#[derive(Debug, Clone, Node)]
pub struct Transform {
    pub id: NodeId,

    #[property(expected = Float)]
    pub scale_x: NodeProperty,
    #[property(expected = Float)]
    pub scale_y: NodeProperty,
    #[property(expected = Float)]
    pub translate_x: NodeProperty,
    #[property(expected = Float)]
    pub translate_y: NodeProperty,
    #[property(expected = Float)]
    pub rotate: NodeProperty,
    #[property(expected = Float)]
    pub pivot_x: NodeProperty,
    #[property(expected = Float)]
    pub pivot_y: NodeProperty,
    #[property(expected = Int)]
    pub sampling: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            scale_x: NodeProperty::Float(1.0),
            scale_y: NodeProperty::Float(1.0),
            translate_x: NodeProperty::Float(0.0),
            translate_y: NodeProperty::Float(0.0),
            rotate: NodeProperty::Float(0.0),
            pivot_x: NodeProperty::Float(0.0),
            pivot_y: NodeProperty::Float(0.0),
            sampling: NodeProperty::Int(TransformSampling::Linear as i64),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Transform {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();
        let source_data = source.data_rect();

        let scale_x = self.resolve_scale_x(ctx)? as f32;
        let scale_y = self.resolve_scale_y(ctx)? as f32;
        let translate_x = self.resolve_translate_x(ctx)? as f32;
        let translate_y = self.resolve_translate_y(ctx)? as f32;
        let rotate = self.resolve_rotate(ctx)? as f32;
        let pivot_x = self.resolve_pivot_x(ctx)? as f32;
        let pivot_y = self.resolve_pivot_y(ctx)? as f32;
        let sampling_mode = TransformSampling::from_int(self.resolve_sampling(ctx)?);

        if Self::is_identity_transform(scale_x, scale_y, translate_x, translate_y, rotate) {
            return source.snapshot();
        }

        let (image, source_width, source_height) = match source.image_parts() {
            Some(parts) => parts,
            None => return source.snapshot(),
        };

        if source_width == 0 || source_height == 0 {
            return source.snapshot();
        }

        let (pivot_x, pivot_y) = Self::resolved_pivot(pivot_x, pivot_y, source_format);
        let transform = Self::build_transform_matrix(
            scale_x,
            scale_y,
            translate_x,
            translate_y,
            rotate,
            pivot_x,
            pivot_y,
        );
        let output_format = Self::map_rect(transform, source_format);
        let output_data = Self::map_rect(transform, source_data)
            .intersect(&output_format)
            .unwrap_or(RectI::new(output_format.x, output_format.y, 0, 0));

        let sampling = match sampling_mode {
            TransformSampling::Nearest => SamplingOptions::default(),
            TransformSampling::Linear => SamplingOptions::from(FilterMode::Linear),
        };
        let source_rect = Rect::from_xywh(
            source_format.x as f32,
            source_format.y as f32,
            source_format.width as f32,
            source_format.height as f32,
        );
        let storage_rect = Rect::from_xywh(0.0, 0.0, source_width as f32, source_height as f32);

        render_to_surface_ephemeral(
            output_format.width.max(1),
            output_format.height.max(1),
            ctx,
            output_format,
            output_data,
            source_alpha,
            ClearMode::Transparent,
            |canvas| {
                canvas.translate((-output_format.x as f32, -output_format.y as f32));
                canvas.concat(&transform);
                canvas.draw_image_rect_with_sampling_options(
                    &image,
                    Some((&storage_rect, skia_safe::canvas::SrcRectConstraint::Fast)),
                    source_rect,
                    sampling,
                    &skia_safe::Paint::default(),
                );
            },
        )
    }

    pub fn is_identity(&self) -> bool {
        let Some(scale_x) = Self::literal_float(&self.scale_x) else {
            return false;
        };
        let Some(scale_y) = Self::literal_float(&self.scale_y) else {
            return false;
        };
        let Some(translate_x) = Self::literal_float(&self.translate_x) else {
            return false;
        };
        let Some(translate_y) = Self::literal_float(&self.translate_y) else {
            return false;
        };
        let Some(rotate) = Self::literal_float(&self.rotate) else {
            return false;
        };

        Self::is_identity_transform(scale_x, scale_y, translate_x, translate_y, rotate)
    }

    fn is_identity_transform(
        scale_x: f32,
        scale_y: f32,
        translate_x: f32,
        translate_y: f32,
        rotate: f32,
    ) -> bool {
        (scale_x - 1.0).abs() <= f32::EPSILON
            && (scale_y - 1.0).abs() <= f32::EPSILON
            && translate_x.abs() <= f32::EPSILON
            && translate_y.abs() <= f32::EPSILON
            && rotate.abs() <= f32::EPSILON
    }

    fn resolved_pivot(pivot_x: f32, pivot_y: f32, source_format: RectI) -> (f32, f32) {
        if pivot_x.abs() <= f32::EPSILON && pivot_y.abs() <= f32::EPSILON {
            (
                source_format.x as f32 + source_format.width as f32 * 0.5,
                source_format.y as f32 + source_format.height as f32 * 0.5,
            )
        } else {
            (pivot_x, pivot_y)
        }
    }

    fn build_transform_matrix(
        scale_x: f32,
        scale_y: f32,
        translate_x: f32,
        translate_y: f32,
        rotate: f32,
        pivot_x: f32,
        pivot_y: f32,
    ) -> Matrix {
        let mut matrix = Matrix::new_identity();
        matrix.pre_translate((pivot_x + translate_x, pivot_y + translate_y));
        matrix.pre_rotate(rotate, None);
        matrix.pre_scale((scale_x, scale_y), None);
        matrix.pre_translate((-pivot_x, -pivot_y));
        matrix
    }

    fn map_rect(matrix: Matrix, rect: RectI) -> RectI {
        if rect.width == 0 || rect.height == 0 {
            return RectI::new(rect.x, rect.y, 0, 0);
        }

        let corners = [
            Point::new(rect.x as f32, rect.y as f32),
            Point::new(rect.right() as f32, rect.y as f32),
            Point::new(rect.x as f32, rect.bottom() as f32),
            Point::new(rect.right() as f32, rect.bottom() as f32),
        ];
        let mut mapped = [Point::new(0.0, 0.0); 4];
        matrix.map_points(&mut mapped, &corners);

        let min_x = mapped
            .iter()
            .map(|point| point.x)
            .fold(f32::INFINITY, f32::min);
        let min_y = mapped
            .iter()
            .map(|point| point.y)
            .fold(f32::INFINITY, f32::min);
        let max_x = mapped
            .iter()
            .map(|point| point.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let max_y = mapped
            .iter()
            .map(|point| point.y)
            .fold(f32::NEG_INFINITY, f32::max);

        let min_x = min_x.floor() as i32;
        let min_y = min_y.floor() as i32;
        let max_x = max_x.ceil() as i32;
        let max_y = max_y.ceil() as i32;

        RectI::new(
            min_x,
            min_y,
            (max_x - min_x).max(1) as u32,
            (max_y - min_y).max(1) as u32,
        )
    }

    fn literal_float(property: &NodeProperty) -> Option<f32> {
        match property {
            NodeProperty::Float(value) => Some(*value as f32),
            NodeProperty::Int(value) => Some(*value as f32),
            NodeProperty::String(value) => value.parse::<f32>().ok(),
            _ => None,
        }
    }
}
