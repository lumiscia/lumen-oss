use lumen_engine::{
    composition::Composition,
    diagnostic::Diagnostic,
    error::{LumenError, MediaError, MediaFailure},
    json::ParseError,
    media::CpuMediaFrame,
};
use std::sync::Arc;
use wasm_bindgen::JsValue;

/// The JS `Error` for a failed render. A video frame that isn't loaded yet has the code
/// `media_frame_out_of_range`, on which callers load the frame and render again.
pub fn render_failure(error: &LumenError) -> JsValue {
    let js_error = js_sys::Error::new(&error.to_string());
    let set = |key: &str, value: &JsValue| {
        let _ = js_sys::Reflect::set(&js_error, &JsValue::from_str(key), value);
    };
    if matches!(error, LumenError::Media(MediaError::FrameOutOfRange { .. })) {
        set("code", &JsValue::from_str("media_frame_out_of_range"));
    } else if let Some(failure) = error.media_failure() {
        set("code", &JsValue::from_str(failure.code()));
        if let Ok(media) = js_sys::JSON::parse(&media_failure_json(failure)) {
            set("media", &media);
        }
    }
    js_error.into()
}

/// The structured fields of `failure` as JSON: its `code`, `source`, `wanted` and `found` kinds,
/// `at`, `details`, and for `media.too_large` the `limit` and `actual` value.
pub fn media_failure_json(failure: &MediaFailure) -> String {
    serde_json::to_string(failure).unwrap_or_else(|_| "{}".to_string())
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

/// `diagnostics` as a JS array of plain objects.
pub fn diagnostics_value(diagnostics: &[Diagnostic]) -> JsValue {
    serde_json::to_string(diagnostics)
        .ok()
        .and_then(|json| js_sys::JSON::parse(&json).ok())
        .unwrap_or_else(|| js_sys::Array::new().into())
}

/// The JS `Error` for a composition that doesn't parse, with its `diagnostics`.
pub fn parse_failure(error: &ParseError) -> JsValue {
    let js_error = js_sys::Error::new(&error.to_string());
    let _ = js_sys::Reflect::set(
        &js_error,
        &JsValue::from_str("diagnostics"),
        &diagnostics_value(error.diagnostics()),
    );
    js_error.into()
}

pub fn composition_json_to_composition(composition_json: &str) -> Result<Composition, ParseError> {
    lumen_engine::json::parse(composition_json)
}

#[cfg(test)]
mod tests {
    use lumen_engine::{
        error::MediaFailure,
        media::{AssetKind, MediaLocation, NodeParamAt},
        node::NodeId,
    };
    use serde_json::json;

    use super::*;

    #[test]
    fn a_media_failure_carries_its_code_and_fields_to_javascript() {
        let failure = MediaFailure::too_large("hero.glb", "too big", 100, Some(120))
            .wanting(&[AssetKind::Model])
            .located(|| {
                MediaLocation::Node(NodeParamAt {
                    template: Some("card".to_string()),
                    node: NodeId::new(4),
                    param: "source".to_string(),
                })
            });

        let fields: serde_json::Value =
            serde_json::from_str(&media_failure_json(&failure)).unwrap();
        assert_eq!(
            fields,
            json!({
                "code": "media.too_large",
                "source": "hero.glb",
                "wanted": ["model"],
                "at": { "type": "node", "template": "card", "node": 4, "param": "source" },
                "details": "too big",
                "limit": 100,
                "actual": 120,
            })
        );
    }
}
