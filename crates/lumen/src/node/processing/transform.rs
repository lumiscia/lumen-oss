use skia_safe::{CubicResampler, Matrix, SamplingOptions};

use crate::{
    node::{
        NodeId, NodeProperty, PortRef,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    raster::RasterFrame,
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
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<RasterFrame> {
        let source_result = ctx.eval(self.source.clone())?;
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

        let render_width = source_width.max(ctx.renderer.composition.render_settings.width);
        let render_height = source_height.max(ctx.renderer.composition.render_settings.height);

        let (pivot_x, pivot_y) =
            Self::resolved_pivot(pivot_x, pivot_y, source_width, source_height);
        let mut matrix = Matrix::new_identity();
        matrix.pre_translate((pivot_x + translate_x, pivot_y + translate_y));
        matrix.pre_rotate(rotate, None);
        matrix.pre_scale((scale_x, scale_y), None);
        matrix.pre_translate((-pivot_x, -pivot_y));

        let sampling = match sampling_mode {
            TransformSampling::Nearest => SamplingOptions::default(),
            TransformSampling::Linear => SamplingOptions::from(CubicResampler::catmull_rom()),
        };

        render_to_surface_ephemeral(
            render_width,
            render_height,
            ctx,
            source_format,
            source_data,
            source_alpha,
            ClearMode::Transparent,
            |canvas| {
                canvas.concat(&matrix);
                canvas.draw_image_with_sampling_options(&image, (0.0, 0.0), sampling, None);
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

    fn resolved_pivot(pivot_x: f32, pivot_y: f32, width: u32, height: u32) -> (f32, f32) {
        if pivot_x.abs() <= f32::EPSILON && pivot_y.abs() <= f32::EPSILON {
            (width as f32 * 0.5, height as f32 * 0.5)
        } else {
            (pivot_x, pivot_y)
        }
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
