use std::ops::Range;

use lumen_ffmpeg::{QualityConfig, QualityRequest, VideoEncoderChoice};
use serde::Serialize;

use super::{Error, ErrorCode, RenderOutput, SequenceLimits, resolve_encoder};
use crate::{composition::Composition, diagnostic::Diagnostic};

/// What a host asks to render, in the library's terms. A host maps its own input to it, such as
/// the `lumen` CLI's flags or `lumen-server`'s request body.
#[derive(Debug, Clone, Default)]
pub struct RenderRequest {
    pub output: RenderOutput,
    /// The preset, codec, bit depth, alpha, rate control and pinned encoder. They apply to the
    /// video formats only, and any of them on an image output is [`ErrorCode::InvalidOutput`].
    pub quality: QualityRequest,
    /// The encoder pinned when `quality` pins none, such as the one `LUMEN_VIDEO_ENCODER` names.
    /// Unlike `quality.encoder`, an image output ignores it.
    pub default_encoder: Option<String>,
}

/// What a [`RenderRequest`] renders, worked out before any GPU work.
#[derive(Debug, Clone)]
pub struct RenderPlan {
    pub output: RenderOutput,
    /// The frames of the timeline the render covers.
    pub frames: Range<u32>,
    /// The encoders a video tries, in order. `None` for an image output.
    pub encoder: Option<VideoEncoderChoice>,
}

/// Checks `request` against `composition` and resolves its encoders with `quality`. An output
/// [`RenderOutput::resolve`] refuses, or an image output with quality settings, is
/// [`ErrorCode::InvalidOutput`]; with `limits`, a sequence past them is
/// [`ErrorCode::OutputTooLarge`]; and an encoder that can't be resolved fails as
/// [`resolve_encoder`] does.
pub fn plan(
    composition: &Composition,
    request: &RenderRequest,
    quality: &QualityConfig,
    limits: Option<&SequenceLimits>,
) -> Result<RenderPlan, Error> {
    let duration_frames = composition.timeline().duration_frames;
    let frames = match limits {
        Some(limits) => request.output.validate(duration_frames, limits)?,
        None => request.output.resolve(duration_frames)?,
    };
    let format = request.output.format;
    if !format.is_video() && request.quality != QualityRequest::default() {
        return Err(Error::new(
            ErrorCode::InvalidOutput,
            "quality, codec, bit depth, alpha, rate control and encoder only apply to the mp4, mov and webm formats",
            false,
        ));
    }
    let encoder = if format.is_video() {
        let video = request
            .quality
            .clone()
            .with_encoder_fallback(request.default_encoder.as_deref());
        Some(resolve_encoder(quality, composition, format, video)?)
    } else {
        None
    };
    Ok(RenderPlan {
        output: request.output.clone(),
        frames,
        encoder,
    })
}

/// What a check of a composition found. It is `ok` when none of the diagnostics is an error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct CheckReport {
    pub diagnostics: Vec<Diagnostic>,
    pub ok: bool,
}

impl CheckReport {
    pub fn new(diagnostics: Vec<Diagnostic>) -> Self {
        let ok = !diagnostics.iter().any(Diagnostic::is_error);
        Self { diagnostics, ok }
    }
}

#[cfg(test)]
mod tests {
    use lumen_ffmpeg::{EncodeCodec, EncoderQuality, RateControl};
    use serde_json::json;

    use super::*;
    use crate::render::OutputFormat;

    fn composition() -> Composition {
        crate::json::parse_value(&json!({
            "timeline": { "fps": 24, "duration_frames": 10 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": [
                { "id": 1, "type": "background", "params": { "paint": [0, 0, 0, 255], "width": 16, "height": 16 } },
                { "id": 2, "type": "media_output" }
            ],
            "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
        }))
        .unwrap()
    }

    #[test]
    fn an_image_output_takes_no_video_settings() {
        let composition = composition();
        let config = QualityConfig::default();
        let video_settings = [
            QualityRequest {
                preset: Some("high".into()),
                ..QualityRequest::default()
            },
            QualityRequest {
                codec: Some(EncodeCodec::Hevc),
                ..QualityRequest::default()
            },
            QualityRequest {
                alpha: Some(true),
                ..QualityRequest::default()
            },
            QualityRequest {
                settings: Some(EncoderQuality {
                    rate_control: Some(RateControl::Crf(23)),
                    ..EncoderQuality::default()
                }),
                ..QualityRequest::default()
            },
            QualityRequest {
                encoder: Some("libx264".into()),
                ..QualityRequest::default()
            },
        ];
        for format in [OutputFormat::Png, OutputFormat::JpegSequence] {
            for quality in &video_settings {
                let request = RenderRequest {
                    output: RenderOutput::of(format),
                    quality: quality.clone(),
                    ..RenderRequest::default()
                };
                let error = plan(&composition, &request, &config, None).unwrap_err();
                assert_eq!(
                    error.code(),
                    ErrorCode::InvalidOutput,
                    "{format:?} {quality:?}"
                );
                assert!(
                    error.to_string().contains("only apply to the mp4"),
                    "{error}"
                );
            }
            let request = RenderRequest {
                output: RenderOutput::of(format),
                default_encoder: Some("libx264".into()),
                ..RenderRequest::default()
            };
            let image = plan(&composition, &request, &config, None).unwrap();
            assert!(image.encoder.is_none());
        }
    }
}
