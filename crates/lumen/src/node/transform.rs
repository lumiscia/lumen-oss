use std::sync::Arc;

use skia_safe::{CubicResampler, Matrix, SamplingOptions};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame},
    render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransformSampling {
    Nearest,
    Linear,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Transform {
    pub scale_x: f32,
    pub scale_y: f32,
    pub translate_x: f32,
    pub translate_y: f32,
    pub rotate: f32,
    pub pivot_x: f32,
    pub pivot_y: f32,
    pub sampling: TransformSampling,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            scale_x: 1.0,
            scale_y: 1.0,
            translate_x: 0.0,
            translate_y: 0.0,
            rotate: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
            sampling: TransformSampling::Linear,
        }
    }
}

const INPUT_PORT_DEFS: &[InputPortDef] = &[InputPortDef {
    name: "source",
    kind: PortKind::RasterFrame,
    optional: false,
}];

const OUTPUT_PORT_DEFS: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

impl NodeEval for Transform {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        INPUT_PORT_DEFS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        OUTPUT_PORT_DEFS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let source = inputs.get_raster("source")?;
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();
        let source_data = source.data_rect();
        if self.is_identity() {
            return Ok(PortValue::RasterFrame(source.clone()));
        }

        let (bytes, width, height) = source.clone().into_parts();

        if width == 0 || height == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                BitmapFrame::with_domain(bytes, width, height, source_format, source_data)
                    .with_alpha_mode(source_alpha),
            )));
        }

        let Some(image) =
            make_skia_image(&bytes, width, height, (width as usize) * 4, source_alpha)
        else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                BitmapFrame::with_domain(bytes, width, height, source_format, source_data)
                    .with_alpha_mode(source_alpha),
            )));
        };

        let (pivot_x, pivot_y) = self.resolved_pivot(width, height);
        let mut matrix = Matrix::new_identity();
        matrix.pre_translate((pivot_x + self.translate_x, pivot_y + self.translate_y));
        matrix.pre_rotate(self.rotate, None);
        matrix.pre_scale((self.scale_x, self.scale_y), None);
        matrix.pre_translate((-pivot_x, -pivot_y));

        let sampling = match self.sampling {
            TransformSampling::Nearest => SamplingOptions::default(),
            TransformSampling::Linear => SamplingOptions::from(CubicResampler::catmull_rom()),
        };

        let transformed = render_with_skia(width, height, Some(ctx), |canvas| {
            canvas.concat(&matrix);
            canvas.draw_image_with_sampling_options(&image, (0.0, 0.0), sampling, None);
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                Arc::new(transformed),
                width,
                height,
                source_format,
                source_data,
            )
            .with_alpha_mode(source_alpha),
        )))
    }
}

impl Transform {
    pub fn is_identity(&self) -> bool {
        (self.scale_x - 1.0).abs() <= f32::EPSILON
            && (self.scale_y - 1.0).abs() <= f32::EPSILON
            && self.translate_x.abs() <= f32::EPSILON
            && self.translate_y.abs() <= f32::EPSILON
            && self.rotate.abs() <= f32::EPSILON
    }

    fn resolved_pivot(&self, width: u32, height: u32) -> (f32, f32) {
        if self.pivot_x.abs() <= f32::EPSILON && self.pivot_y.abs() <= f32::EPSILON {
            (width as f32 * 0.5, height as f32 * 0.5)
        } else {
            (self.pivot_x, self.pivot_y)
        }
    }
}
