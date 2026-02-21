use skia_safe::Paint;

use crate::compile::{CompiledShadowStyle, RuntimeFrameContext};
use crate::model::BlendMode;

pub fn build_shadow_paint(
    _shadow: &CompiledShadowStyle,
    _frame_state: &RuntimeFrameContext,
    _opacity: f32,
    _blend_mode: BlendMode,
) -> Paint {
    Paint::default()
}
