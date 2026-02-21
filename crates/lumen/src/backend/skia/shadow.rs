use skia_safe::{Paint, image_filters};

use crate::compile::{CompiledShadowStyle, RuntimeFrameContext};
use crate::model::BlendMode;

use super::primitives::{to_color4f, to_skia_blend_mode};

pub fn build_shadow_paint(
    shadow: &CompiledShadowStyle,
    frame_state: &RuntimeFrameContext,
    opacity: f32,
    blend_mode: BlendMode,
) -> Paint {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_blend_mode(to_skia_blend_mode(blend_mode));

    let sigma = shadow.blur.resolve(frame_state).max(0.0);
    let filter = image_filters::drop_shadow(
        (
            shadow.offset_x.resolve(frame_state),
            shadow.offset_y.resolve(frame_state),
        ),
        (sigma, sigma),
        to_color4f(shadow.color, opacity),
        None,
        None,
        None::<image_filters::CropRect>,
    );
    paint.set_image_filter(filter);
    paint
}
