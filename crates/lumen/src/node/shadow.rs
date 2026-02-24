use std::sync::Arc;

use skia_safe::{Paint, image_filters};

use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue,
        pixel_utils::{make_skia_image, render_with_skia, to_skia_color},
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
pub struct Shadow {
    pub offset_x: i32,
    pub offset_y: i32,
    pub color: [u8; 4],
}

impl NodeEval for Shadow {
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

        if width == 0 || height == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(bytes, width, height)));
        }

        let Some(image) = make_skia_image(&bytes, width, height) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(bytes, width, height)));
        };

        let shadow_color = to_skia_color(self.color);
        let dx = self.offset_x as f32;
        let dy = self.offset_y as f32;

        let output = render_with_skia(width, height, |canvas| {
            let filter = image_filters::drop_shadow_only(
                (dx, dy),
                (0.0, 0.0),
                shadow_color,
                None,
                None,
                None,
            );
            if let Some(filter) = filter {
                let mut shadow_paint = Paint::default();
                shadow_paint.set_image_filter(filter);
                canvas.draw_image(&image, (0.0, 0.0), Some(&shadow_paint));
            }
            canvas.draw_image(&image, (0.0, 0.0), None);
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(output),
            width,
            height,
        )))
    }
}
