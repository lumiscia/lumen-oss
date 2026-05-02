use skia_safe::{FilterMode, Rect, SamplingOptions};

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
pub enum ResizeMode {
    Stretch = 0,
    Fit = 1,
    Fill = 2,
}

impl ResizeMode {
    fn from_int(value: i64) -> Self {
        match value {
            1 => Self::Fit,
            2 => Self::Fill,
            _ => Self::Stretch,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i64)]
pub enum ResizeSampling {
    Nearest = 0,
    Linear = 1,
}

impl ResizeSampling {
    fn from_int(value: i64) -> Self {
        match value {
            0 => Self::Nearest,
            _ => Self::Linear,
        }
    }
}

#[derive(Debug, Clone, Node)]
pub struct Resize {
    pub id: NodeId,

    #[property(expected = Int)]
    pub width: NodeProperty,
    #[property(expected = Int)]
    pub height: NodeProperty,
    #[property(expected = Int)]
    pub mode: NodeProperty,
    #[property(expected = Int)]
    pub sampling: NodeProperty,

    #[input(kind = Raster)]
    pub source: PortRef,
}

impl Default for Resize {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
            mode: NodeProperty::Int(ResizeMode::Stretch as i64),
            sampling: NodeProperty::Int(ResizeSampling::Linear as i64),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl Resize {
    #[output(port = "output", kind = Raster)]
    fn eval_output(&self, ctx: &mut RenderContext) -> crate::Result<GpuImageFrame> {
        let source_result = ctx.eval(&self.source)?;
        let source = source_result.as_raster()?;
        let mode = ResizeMode::from_int(self.resolve_mode(ctx)?);
        let sampling_mode = ResizeSampling::from_int(self.resolve_sampling(ctx)?);
        let dest_width = self.resolve_width(ctx)?.max(1) as u32;
        let dest_height = self.resolve_height(ctx)?.max(1) as u32;

        let (source_width, source_height) = source.dimensions();
        let source_alpha = source.alpha_mode();
        let source_format = source.format_rect();

        // Early return if dimensions already match.
        if source_width == dest_width && source_height == dest_height {
            return source.snapshot();
        }

        let output_rect = RectI::new(source_format.x, source_format.y, dest_width, dest_height);

        // Handle empty source image by returning transparent buffer
        if source_width == 0 || source_height == 0 {
            return render_to_surface_ephemeral(
                dest_width,
                dest_height,
                ctx,
                output_rect,
                output_rect,
                source_alpha,
                ClearMode::Transparent,
                |_| {},
            );
        }

        let (image, source_width, source_height) = match source.image_parts() {
            Some(parts) => parts,
            None => {
                return render_to_surface_ephemeral(
                    dest_width,
                    dest_height,
                    ctx,
                    output_rect,
                    output_rect,
                    source_alpha,
                    ClearMode::Transparent,
                    |_| {},
                );
            }
        };

        let (source_rect, dest_rect) =
            compute_rects(source_width, source_height, dest_width, dest_height, mode);
        let sampling = match sampling_mode {
            ResizeSampling::Nearest => SamplingOptions::default(),
            ResizeSampling::Linear => SamplingOptions::from(FilterMode::Linear),
        };
        let clear_mode = match mode {
            ResizeMode::Fit => ClearMode::Transparent,
            ResizeMode::Stretch | ResizeMode::Fill => ClearMode::None,
        };

        render_to_surface_ephemeral(
            dest_width,
            dest_height,
            ctx,
            output_rect,
            output_rect,
            source_alpha,
            clear_mode,
            |canvas| {
                canvas.draw_image_rect_with_sampling_options(
                    &image,
                    Some((&source_rect, skia_safe::canvas::SrcRectConstraint::Fast)),
                    dest_rect,
                    sampling,
                    &skia_safe::Paint::default(),
                );
            },
        )
    }
}

fn compute_rects(
    source_width: u32,
    source_height: u32,
    dest_width: u32,
    dest_height: u32,
    mode: ResizeMode,
) -> (Rect, Rect) {
    let source_rect = Rect::from_wh(source_width as f32, source_height as f32);
    let dest_rect = Rect::from_wh(dest_width as f32, dest_height as f32);

    match mode {
        ResizeMode::Stretch => (source_rect, dest_rect),
        ResizeMode::Fit => {
            let scale = (dest_width as f32 / source_width as f32)
                .min(dest_height as f32 / source_height as f32);
            let scaled_width = source_width as f32 * scale;
            let scaled_height = source_height as f32 * scale;
            let x_offset = (dest_width as f32 - scaled_width) * 0.5;
            let y_offset = (dest_height as f32 - scaled_height) * 0.5;
            (
                source_rect,
                Rect::from_xywh(x_offset, y_offset, scaled_width, scaled_height),
            )
        }
        ResizeMode::Fill => {
            let scale = (dest_width as f32 / source_width as f32)
                .max(dest_height as f32 / source_height as f32);
            let crop_width = dest_width as f32 / scale;
            let crop_height = dest_height as f32 / scale;
            let x_offset = (source_width as f32 - crop_width) * 0.5;
            let y_offset = (source_height as f32 - crop_height) * 0.5;
            (
                Rect::from_xywh(x_offset, y_offset, crop_width, crop_height),
                dest_rect,
            )
        }
    }
}
