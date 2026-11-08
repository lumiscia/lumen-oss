use lumen_engine::{
    composition::Composition,
    error::{LumenError, MediaError},
    media::{CpuMediaFrame, premultiply_rgba_in_place_if_needed},
};
use std::sync::Arc;
use wasm_bindgen::JsValue;

/// The JS `Error` for a failed render. A video frame that isn't loaded yet has the code
/// `media_frame_out_of_range`, on which callers load the frame and render again.
pub fn render_failure(error: &LumenError) -> JsValue {
    let js_error = js_sys::Error::new(&error.to_string());
    if matches!(error, LumenError::Media(MediaError::FrameOutOfRange { .. })) {
        let _ = js_sys::Reflect::set(
            &js_error,
            &JsValue::from_str("code"),
            &JsValue::from_str("media_frame_out_of_range"),
        );
    }
    js_error.into()
}

pub fn validate_rgba_len(width: u32, height: u32, len: usize) -> bool {
    u64::from(width)
        .checked_mul(u64::from(height))
        .and_then(|px| px.checked_mul(4))
        .and_then(|bytes| usize::try_from(bytes).ok())
        .is_some_and(|expected| expected == len)
}

/// A frame holding `rgba` as it is, whose colour may or may not be multiplied by alpha.
pub fn raw_frame_from_rgba(
    width: u32,
    height: u32,
    rgba: Vec<u8>,
) -> Result<CpuMediaFrame, String> {
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

/// A frame of straight-alpha `rgba` with its colour premultiplied.
pub fn image_frame_from_rgba(
    width: u32,
    height: u32,
    mut rgba: Vec<u8>,
) -> Result<CpuMediaFrame, String> {
    premultiply_rgba_in_place_if_needed(&mut rgba);
    raw_frame_from_rgba(width, height, rgba)
}

pub fn composition_json_to_composition(composition_json: &str) -> Result<Composition, String> {
    lumen_engine::json::parse(composition_json).map_err(|e| format!("{e:#}"))
}
