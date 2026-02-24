use std::sync::Arc;

use skia_safe::{CubicResampler, Rect, SamplingOptions};

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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeMode {
    Stretch,
    Fit,
    Fill,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResizeSampling {
    Nearest,
    Linear,
}

#[derive(Debug, Clone)]
pub struct Resize {
    pub width: u32,
    pub height: u32,
    pub mode: ResizeMode,
    pub sampling: ResizeSampling,
}

impl NodeEval for Resize {
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
        let (bytes, src_w, src_h) = inputs.get_raster("source")?.clone().into_parts();

        let dst_w = self.width.max(1);
        let dst_h = self.height.max(1);

        if src_w == 0 || src_h == 0 {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(vec![0u8; (dst_w as usize) * (dst_h as usize) * 4]),
                dst_w,
                dst_h,
            )));
        }

        let Some(image) = make_skia_image(&bytes, src_w, src_h) else {
            return Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
                Arc::new(vec![0u8; (dst_w as usize) * (dst_h as usize) * 4]),
                dst_w,
                dst_h,
            )));
        };

        let (src_rect, dst_rect) = compute_rects(src_w, src_h, dst_w, dst_h, self.mode);
        let sampling = match self.sampling {
            ResizeSampling::Nearest => SamplingOptions::default(),
            ResizeSampling::Linear => SamplingOptions::from(CubicResampler::catmull_rom()),
        };

        let resized = render_with_skia(dst_w, dst_h, |canvas| {
            canvas.draw_image_rect_with_sampling_options(
                &image,
                Some((&src_rect, skia_safe::canvas::SrcRectConstraint::Fast)),
                dst_rect,
                sampling,
                &skia_safe::Paint::default(),
            );
        });

        Ok(PortValue::RasterFrame(RasterFrame::Bitmap(
            Arc::new(resized),
            dst_w,
            dst_h,
        )))
    }
}

fn compute_rects(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32, mode: ResizeMode) -> (Rect, Rect) {
    let src_full = Rect::from_wh(src_w as f32, src_h as f32);
    let dst_full = Rect::from_wh(dst_w as f32, dst_h as f32);

    match mode {
        ResizeMode::Stretch => (src_full, dst_full),
        ResizeMode::Fit => {
            let scale = (dst_w as f32 / src_w as f32).min(dst_h as f32 / src_h as f32);
            let w = src_w as f32 * scale;
            let h = src_h as f32 * scale;
            let x = (dst_w as f32 - w) * 0.5;
            let y = (dst_h as f32 - h) * 0.5;
            (src_full, Rect::from_xywh(x, y, w, h))
        }
        ResizeMode::Fill => {
            let scale = (dst_w as f32 / src_w as f32).max(dst_h as f32 / src_h as f32);
            let w = dst_w as f32 / scale;
            let h = dst_h as f32 / scale;
            let x = (src_w as f32 - w) * 0.5;
            let y = (src_h as f32 - h) * 0.5;
            (Rect::from_xywh(x, y, w, h), dst_full)
        }
    }
}
