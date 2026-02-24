use std::sync::Arc;

use skia_safe::{Paint, image_filters};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::{BitmapFrame, RasterFrame},
    render::RenderContext,
};

const INPUT_PORTS: [InputPortDef; 1] = [InputPortDef {
    name: "source",
    kind: PortKind::RasterFrame,
    optional: false,
}];

const OUTPUT_PORTS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

#[derive(Debug, Clone)]
pub struct Blur {
    pub radius: f32,
}

impl Blur {
    pub fn is_noop(&self) -> bool {
        self.radius <= 0.0
    }
}

impl NodeEval for Blur {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &INPUT_PORTS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &OUTPUT_PORTS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        if self.is_noop() {
            return Ok(PortValue::RasterFrame(inputs.get_raster("source")?.clone()));
        }

        let source = inputs.get_raster("source")?;
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();
        let source_data = source.data_rect();
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

        let sigma = self.radius.max(0.5);
        let blurred = render_with_skia(width, height, Some(ctx), |canvas| {
            if let Some(filter) = image_filters::blur((sigma, sigma), None, None, None) {
                let mut paint = Paint::default();
                paint.set_image_filter(filter);
                canvas.draw_image(&image, (0.0, 0.0), Some(&paint));
            } else {
                canvas.draw_image(&image, (0.0, 0.0), None);
            }
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(Arc::new(blurred), width, height, source_format, source_data)
                .with_alpha_mode(source_alpha),
        )))
    }
}
