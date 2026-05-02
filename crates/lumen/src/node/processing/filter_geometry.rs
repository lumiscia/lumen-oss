use crate::gpu_image::RectI;

pub(crate) fn filter_pad(sigma: f32) -> i32 {
    if sigma <= 0.0 {
        0
    } else {
        (sigma * 4.0 + 2.0).ceil() as i32
    }
}

pub(crate) fn expand_rect(rect: RectI, pad: i32) -> RectI {
    if pad <= 0 {
        return rect;
    }

    let pad64 = i64::from(pad);
    let min_x = i64::from(rect.x) - pad64;
    let min_y = i64::from(rect.y) - pad64;
    let max_x = rect.right() + pad64;
    let max_y = rect.bottom() + pad64;

    RectI::new(
        min_x.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        min_y.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32,
        (max_x - min_x).max(1) as u32,
        (max_y - min_y).max(1) as u32,
    )
}

pub(crate) fn offset_rect(rect: RectI, offset_x: i32, offset_y: i32) -> RectI {
    RectI::new(
        rect.x.saturating_add(offset_x),
        rect.y.saturating_add(offset_y),
        rect.width,
        rect.height,
    )
}
