use std::sync::Arc;

use skia_safe::{image_filters, Paint};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia},
    },
    raster::RasterFrame,
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
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let (bytes, width, height) = inputs.get_raster("source")?.clone().into_parts();

        if self.is_noop() || width == 0 || height == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(bytes, width, height)));
        }

        let Some(image) = make_skia_image(&bytes, width, height) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(bytes, width, height)));
        };

        let sigma = self.radius.max(0.5);
        let blurred = render_with_skia(width, height, |canvas| {
            let filter = image_filters::blur((sigma, sigma), None, None, None)
                .expect("blur filter creation");
            let mut paint = Paint::default();
            paint.set_image_filter(filter);
            canvas.draw_image(&image, (0.0, 0.0), Some(&paint));
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(blurred),
            width,
            height,
        )))
    }
}
