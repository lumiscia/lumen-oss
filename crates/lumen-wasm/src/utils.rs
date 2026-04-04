use lumen::{
    composition::Composition,
    media::premultiply_rgba_in_place_if_needed,
    raster::{AlphaMode, ImageFrame, RectI},
};

pub fn validate_rgba_len(width: u32, height: u32, len: usize) -> bool {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|px| px.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .is_some_and(|expected| expected == len)
}

pub fn image_frame_from_rgba(
    width: u32,
    height: u32,
    mut rgba: Vec<u8>,
) -> Result<ImageFrame, String> {
    premultiply_rgba_in_place_if_needed(&mut rgba);
    let rect = RectI::from_size(width, height);
    ImageFrame::from_rgba_bytes(
        rgba.as_slice(),
        width,
        height,
        (width as usize) * 4,
        AlphaMode::Premultiplied,
        rect,
        rect,
    )
    .map_err(|e| e.to_string())
}

pub fn composition_json_to_composition(composition_json: &str) -> Result<Composition, String> {
    lumen::json::parse(composition_json).map_err(|e| e.to_string())
}
