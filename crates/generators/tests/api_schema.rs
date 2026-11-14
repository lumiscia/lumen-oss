use lumen_engine::{
    diagnostic::{Diagnostic, Severity},
    error::MediaFailure,
    media::AssetKind,
};
use lumen_server_api::{
    ErrorBody, ErrorCode, FontFace, FrameRange, Media, MediaKind, MediaMetadata, MediaSource,
    MediaStatus, OutputFormat, Render, RenderEvent, RenderFailure, RenderOutput, RenderRequest,
    RenderStatus, VideoBitDepth, VideoCodec,
};
use serde::Serialize;
use serde_json::{Value, json};

const BASE: &str = "file:///definitions/";

/// What is wrong with `value`, serialised, against the API schema's definition `name`, with the
/// runtime and composition schemas it refers to beside it.
fn schema_errors(name: &str, value: &impl Serialize) -> Vec<String> {
    let parse = |json: String| serde_json::from_str::<Value>(&json).unwrap();
    let registry = jsonschema::Registry::new()
        .add(
            format!("{BASE}api.schema.json"),
            parse(lumen_generators::render_api_schema_json().unwrap()),
        )
        .unwrap()
        .add(
            format!("{BASE}runtime.schema.json"),
            parse(lumen_generators::render_runtime_schema_json().unwrap()),
        )
        .unwrap()
        .add(
            format!("{BASE}composition.schema.json"),
            parse(lumen_generators::render_composition_schema_json().unwrap()),
        )
        .unwrap()
        .prepare()
        .unwrap();
    let schema = json!({ "$ref": format!("{BASE}api.schema.json#/$defs/{name}") });
    let validator = jsonschema::options()
        .with_registry(&registry)
        .build(&schema)
        .unwrap();
    let value = serde_json::to_value(value).unwrap();
    validator
        .iter_errors(&value)
        .map(|error| error.to_string())
        .collect()
}

fn assert_valid(name: &str, value: &impl Serialize) {
    let errors = schema_errors(name, value);
    assert!(errors.is_empty(), "{name}: {errors:?}");
}

fn media_failure() -> Value {
    let failure = MediaFailure::too_large("hero.png", "wider than the device allows", 8192, None)
        .wanting(&[AssetKind::Image]);
    serde_json::to_value(failure).unwrap()
}

fn failure(code: ErrorCode) -> RenderFailure {
    RenderFailure {
        code,
        message: "failed".to_string(),
        retryable: false,
        media: Some(media_failure()),
    }
}

#[test]
fn renders_and_their_events_match_the_schema() {
    let render = Render {
        id: "r_1".to_string(),
        status: RenderStatus::Failed,
        progress: 0.4,
        url: None,
        error: Some(failure(ErrorCode::Other("media.too_large".to_string()))),
        created_at: "2026-11-14T00:00:00Z".to_string(),
        input_hash: "ab".to_string(),
        organization_id: "self-hosted".to_string(),
        cost_micros: 0,
        output_bytes: None,
        duration_ms: None,
    };
    assert_valid("Render", &render);
    assert_valid(
        "Render",
        &Render {
            status: RenderStatus::Succeeded,
            progress: 1.0,
            url: Some("/renders/r_1/artifact".to_string()),
            error: None,
            output_bytes: Some(10),
            duration_ms: Some(20),
            ..render
        },
    );
    let render_id = "r_1".to_string();
    for event in [
        RenderEvent::Queued {
            render_id: render_id.clone(),
        },
        RenderEvent::Progress {
            render_id: render_id.clone(),
            progress: 0.5,
        },
        RenderEvent::Completed {
            render_id: render_id.clone(),
            url: "/renders/r_1/artifact".to_string(),
        },
        RenderEvent::Failed {
            render_id,
            error: failure(ErrorCode::RenderFailed),
        },
    ] {
        assert_valid("RenderEvent", &event);
    }
}

#[test]
fn errors_match_the_schema() {
    let diagnostic = Diagnostic {
        severity: Severity::Error,
        code: lumen_engine::diagnostic::codes().next().unwrap(),
        path: "/nodes/0".to_string(),
        node: None,
        message: "message".to_string(),
    };
    assert_valid(
        "ErrorBody",
        &ErrorBody {
            code: ErrorCode::InvalidProjectPayload,
            error: "invalid".to_string(),
            diagnostics: vec![serde_json::to_value(diagnostic).unwrap()],
            media: Some(media_failure()),
        },
    );
    // The runtime schema's `MediaFailure` is what checks `media`.
    let without_code =
        json!({ "code": "render_failed", "error": "failed", "media": { "source": "a" } });
    assert!(!schema_errors("ErrorBody", &without_code).is_empty());
}

#[test]
fn requests_and_media_match_the_schema() {
    assert_valid(
        "RenderRequest",
        &RenderRequest {
            composition: json!({
                "timeline": { "fps": 30, "duration_frames": 1 },
                "render_settings": { "width": 8, "height": 8 },
                "nodes": [],
                "connections": []
            }),
            media: [("hero".to_string(), "https://example.com/a.png".to_string())].into(),
            webhook_url: None,
            quality: Some("high".to_string()),
            codec: Some(VideoCodec::Hevc),
            bit_depth: Some(VideoBitDepth::Ten),
            alpha: None,
            output: Some(RenderOutput {
                format: OutputFormat::JpegSequence,
                frames: FrameRange {
                    start: Some(2),
                    end: None,
                },
                jpeg_quality: Some(80),
            }),
        },
    );
    assert_valid(
        "Media",
        &Media {
            id: "m_1".to_string(),
            content_type: "font/ttf".to_string(),
            created_at: "2026-11-14T00:00:00Z".to_string(),
            expires_at: "2026-11-15T00:00:00Z".to_string(),
            file_name: "a.ttf".to_string(),
            object_key: None,
            rejection_reason: None,
            size_bytes: 1,
            source_type: MediaSource::Upload,
            source_url: None,
            status: MediaStatus::Validated,
            uploaded_bytes: None,
            kind: Some(MediaKind::Font),
            metadata: Some(MediaMetadata {
                families: Some(vec!["Roboto".to_string()]),
                faces: Some(vec![FontFace {
                    family: "Roboto".to_string(),
                    weight: 400,
                    style: "normal".to_string(),
                }]),
                ..MediaMetadata::default()
            }),
        },
    );
}
