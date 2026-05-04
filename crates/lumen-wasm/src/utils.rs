use lumen::{
    composition::Composition,
    media::{CpuMediaFrame, premultiply_rgba_in_place_if_needed},
};
use std::sync::Arc;

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
) -> Result<CpuMediaFrame, String> {
    premultiply_rgba_in_place_if_needed(&mut rgba);
    let expected = width as usize * height as usize * 4;
    if rgba.len() < expected {
        return Err("RGBA buffer is smaller than frame dimensions".to_string());
    }
    Ok(CpuMediaFrame {
        rgba: Arc::new(rgba),
        width,
        height,
        row_bytes: width as usize * 4,
    })
}

pub fn composition_json_to_composition(composition_json: &str) -> Result<Composition, String> {
    lumen::json::parse(composition_json).map_err(|e| e.to_string())
}
