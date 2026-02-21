use skia_safe::canvas::SaveLayerRec;
use skia_safe::{BlendMode, Canvas, ClipOp, Paint, PathBuilder, RRect, Rect, Vector};

use crate::backend::RenderError;

#[derive(Debug, Clone, Copy)]
pub enum SimpleMaskGeometry {
    Rect {
        bounds: Rect,
        corner_radius: [f32; 4],
    },
    Ellipse {
        bounds: Rect,
    },
}

#[derive(Debug, Clone, Copy)]
pub enum MaskPhase {
    Content,
    Mask,
}

pub fn render_masked<F>(
    canvas: &Canvas,
    bounds: Rect,
    simple_mask: Option<SimpleMaskGeometry>,
    mut render: F,
) -> Result<(), RenderError>
where
    F: FnMut(&Canvas, MaskPhase) -> Result<(), RenderError>,
{
    if let Some(geometry) = simple_mask {
        canvas.save();
        match geometry {
            SimpleMaskGeometry::Rect {
                bounds,
                corner_radius,
            } => {
                if corner_radius.iter().any(|value| *value > 0.0) {
                    let radii = [
                        Vector::new(corner_radius[0], corner_radius[0]),
                        Vector::new(corner_radius[1], corner_radius[1]),
                        Vector::new(corner_radius[2], corner_radius[2]),
                        Vector::new(corner_radius[3], corner_radius[3]),
                    ];
                    let rrect = RRect::new_rect_radii(bounds, &radii);
                    canvas.clip_rrect(rrect, ClipOp::Intersect, true);
                } else {
                    canvas.clip_rect(bounds, ClipOp::Intersect, true);
                }
            }
            SimpleMaskGeometry::Ellipse { bounds } => {
                let mut builder = PathBuilder::new();
                builder.add_oval(bounds, None, None);
                let path = builder.detach();
                canvas.clip_path(&path, ClipOp::Intersect, true);
            }
        }
        let result = render(canvas, MaskPhase::Content);
        canvas.restore();
        return result;
    }

    let content_layer = SaveLayerRec::default().bounds(&bounds);
    canvas.save_layer(&content_layer);
    if let Err(error) = render(canvas, MaskPhase::Content) {
        canvas.restore();
        return Err(error);
    }

    let mut dst_in = Paint::default();
    dst_in.set_blend_mode(BlendMode::DstIn);
    let mask_layer = SaveLayerRec::default().bounds(&bounds).paint(&dst_in);
    canvas.save_layer(&mask_layer);
    if let Err(error) = render(canvas, MaskPhase::Mask) {
        canvas.restore();
        canvas.restore();
        return Err(error);
    }

    canvas.restore();
    canvas.restore();
    Ok(())
}
