use lumen_engine::{
    composition::Composition,
    diagnostic::{Diagnostic, Severity},
    error::{LumenError, MediaError, MediaFailure},
    json::ParseError,
    media::CpuMediaFrame,
};
use serde::Serialize;
use std::sync::Arc;
use wasm_bindgen::{JsCast, JsValue};

/// What a failure carries to JavaScript besides its message, set as properties of the `Error`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ErrorFields<'a> {
    /// The engine's diagnostic code.
    #[serde(skip_serializing_if = "Option::is_none")]
    code: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    media: Option<&'a MediaFailure>,
    #[serde(skip_serializing_if = "<[_]>::is_empty")]
    diagnostics: &'a [Diagnostic],
    /// A video frame that isn't loaded yet: the caller loads it and renders again.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    frame_not_loaded: bool,
}

impl<'a> ErrorFields<'a> {
    fn of_error(error: &'a LumenError) -> Self {
        let media = error.media_failure();
        Self {
            code: Some(media.map_or_else(|| error.code(), MediaFailure::code)),
            media,
            diagnostics: &[],
            frame_not_loaded: matches!(
                error,
                LumenError::Media(MediaError::FrameOutOfRange { .. })
            ),
        }
    }

    /// Carries the code of the first error, which is the first thing wrong with the composition.
    fn of_parse_error(error: &'a ParseError) -> Self {
        Self {
            code: error
                .diagnostics()
                .iter()
                .find(|diagnostic| diagnostic.severity == Severity::Error)
                .map(|diagnostic| diagnostic.code),
            media: None,
            diagnostics: error.diagnostics(),
            frame_not_loaded: false,
        }
    }
}

/// The JS `Error` for `error`, carrying its code and, when it has them, its media failure.
pub fn js_error(error: &LumenError) -> JsValue {
    error_with_fields(&error.to_string(), &ErrorFields::of_error(error))
}

/// The JS `Error` for a composition that doesn't parse, carrying its diagnostics.
pub fn js_parse_error(error: &ParseError) -> JsValue {
    error_with_fields(&error.to_string(), &ErrorFields::of_parse_error(error))
}

fn error_with_fields(message: &str, fields: &ErrorFields) -> JsValue {
    let error = js_sys::Error::new(message);
    let fields = serde_json::to_string(fields)
        .ok()
        .and_then(|json| js_sys::JSON::parse(&json).ok());
    if let Some(fields) = fields {
        js_sys::Object::assign(&error, &fields.unchecked_into());
    }
    error.into()
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

pub fn composition_json_to_composition(composition_json: &str) -> Result<Composition, ParseError> {
    lumen_engine::json::parse(composition_json)
}

#[cfg(test)]
mod tests {
    use lumen_engine::{
        error::{MediaFailure, RenderError},
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

        let error = LumenError::from(failure);
        let fields = serde_json::to_value(ErrorFields::of_error(&error)).unwrap();
        assert_eq!(
            fields,
            json!({
                "code": "media.too_large",
                "media": {
                    "code": "media.too_large",
                    "source": "hero.glb",
                    "wanted": ["model"],
                    "at": { "type": "node", "template": "card", "node": 4, "param": "source" },
                    "details": "too big",
                    "limit": 100,
                    "actual": 120,
                },
            })
        );
    }

    #[test]
    fn a_frame_that_is_not_loaded_is_marked_and_keeps_its_code() {
        let not_loaded = LumenError::Media(MediaError::FrameOutOfRange {
            media_source: "clip".to_string(),
            frame: 3,
            frame_count: 2,
        });
        let fields = serde_json::to_value(ErrorFields::of_error(&not_loaded)).unwrap();
        assert_eq!(fields["code"], not_loaded.code());
        assert_eq!(fields["frameNotLoaded"], true);

        let ordinary = LumenError::Render(RenderError::FrameOutOfRange {
            frame: 9,
            duration_frames: 3,
        });
        let fields = serde_json::to_value(ErrorFields::of_error(&ordinary)).unwrap();
        assert_eq!(fields, json!({ "code": ordinary.code() }));
    }
}
