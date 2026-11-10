/// An sRGB-encoded 8-bit channel in linear light.
pub fn srgb_to_linear(channel: u8) -> f32 {
    let c = f32::from(channel) / 255.0;
    if c <= 0.04045 {
        c / 12.92
    } else {
        ((c + 0.055) / 1.055).powf(2.4)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_ends_and_middle_grey_convert_as_srgb_says() {
        assert_eq!(srgb_to_linear(0), 0.0);
        assert_eq!(srgb_to_linear(255), 1.0);
        assert!((srgb_to_linear(188) - 0.5).abs() < 0.005);
    }
}
