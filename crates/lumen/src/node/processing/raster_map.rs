pub(crate) fn byte_to_unit(value: u8) -> f32 {
    f32::from(value) / 255.0
}

pub(crate) fn unit_to_byte(value: f32) -> u8 {
    (value.clamp(0.0, 1.0) * 255.0).round() as u8
}
