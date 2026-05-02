pub fn premultiply_rgba_in_place_if_needed(pixels: &mut [u8]) {
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        if alpha == u16::from(u8::MAX) {
            continue;
        }
        if alpha == 0 {
            pixel[0] = 0;
            pixel[1] = 0;
            pixel[2] = 0;
            continue;
        }
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha) + 127)
                .checked_div(u16::from(u8::MAX))
                .unwrap_or(0) as u8;
        }
    }
}
