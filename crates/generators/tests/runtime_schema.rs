use lumen_3d::model::{AnimationInfo, CameraInfo, MaterialInfo, ModelInfo};
use lumen_engine::{
    audio::{AudioPlan, AudioSegment, AudioVoice},
    diagnostic::{Diagnostic, Severity},
    error::MediaFailure,
    media::{
        AssetKind, DynamicMediaUse, FontDependency, FrameRequirements, MediaDependency,
        MediaInventory, MediaLocation, MediaUse, NodeParamAt, VideoFrameRequirement,
    },
    node::NodeId,
};
use serde::Serialize;
use serde_json::Value;

/// Asserts that `value`, serialised, is valid against the runtime schema's definition `name`.
fn assert_valid(name: &str, value: &impl Serialize) {
    let mut schema: Value =
        serde_json::from_str(&lumen_generators::render_runtime_schema_json().unwrap()).unwrap();
    schema["$ref"] = format!("#/$defs/{name}").into();
    let validator = jsonschema::validator_for(&schema).unwrap();
    let value = serde_json::to_value(value).unwrap();
    let errors: Vec<_> = validator
        .iter_errors(&value)
        .map(|error| error.to_string())
        .collect();
    assert!(errors.is_empty(), "{name} {value}: {errors:?}");
}

fn param(template: Option<&str>) -> NodeParamAt {
    NodeParamAt {
        template: template.map(str::to_string),
        node: NodeId(3),
        param: "source".to_string(),
    }
}

#[test]
fn diagnostics_match_the_schema() {
    let code = lumen_engine::diagnostic::codes().next().unwrap();
    for (severity, node) in [
        (Severity::Error, None),
        (Severity::Warning, Some(NodeId(7))),
    ] {
        assert_valid(
            "Diagnostic",
            &Diagnostic {
                severity,
                code,
                path: "/nodes/3".to_string(),
                node,
                message: "message".to_string(),
            },
        );
    }
}

#[test]
fn media_inventory_matches_the_schema() {
    let audio = MediaLocation::AudioClip {
        template: Some("intro".to_string()),
        track: "music".to_string(),
        clip: "a".to_string(),
        index: 2,
    };
    let node = MediaLocation::Node(param(None));
    assert_valid(
        "MediaInventory",
        &MediaInventory {
            media: vec![MediaDependency {
                source: "a.mp4".to_string(),
                kinds: vec![AssetKind::Video, AssetKind::Model],
                uses: vec![
                    MediaUse {
                        at: node,
                        kinds: vec![AssetKind::Video],
                    },
                    MediaUse {
                        at: audio,
                        kinds: vec![AssetKind::Audio],
                    },
                ],
            }],
            dynamic: vec![DynamicMediaUse {
                at: param(Some("intro")),
                kinds: vec![AssetKind::Image],
            }],
            fonts: vec![FontDependency {
                family: "Inter".to_string(),
                uses: vec![param(None)],
            }],
        },
    );
}

/// `MediaFailure` serialises by hand, so its schema is written by hand too.
#[test]
fn media_failures_match_the_schema() {
    let at = || MediaLocation::Node(param(None));
    for failure in [
        MediaFailure::not_found("a.png", "no such file"),
        MediaFailure::decode("a.png", "corrupt")
            .wanting(&[AssetKind::Image, AssetKind::Video])
            .located(at),
        MediaFailure::wrong_kind("a.png", &[AssetKind::Video], Some(AssetKind::Image), "")
            .located(at),
        MediaFailure::too_large("a.png", "wider than the device allows", 8192, None),
        MediaFailure::too_large("a.png", "wider than the device allows", 8192, Some(9000))
            .wanting(&[AssetKind::Image])
            .located(at),
    ] {
        assert_valid("MediaFailure", &failure);
    }
}

#[test]
fn model_info_matches_the_schema() {
    assert_valid(
        "ModelInfo",
        &ModelInfo {
            animations: vec![
                AnimationInfo {
                    index: 0,
                    name: Some("walk".to_string()),
                    duration: 1.5,
                },
                AnimationInfo {
                    index: 1,
                    name: None,
                    duration: 0.25,
                },
            ],
            materials: vec![MaterialInfo {
                index: 0,
                name: None,
                has_base_color_texture: true,
            }],
            cameras: vec![CameraInfo {
                index: 0,
                name: Some("main".to_string()),
            }],
        },
    );
}

#[test]
fn audio_plan_matches_the_schema() {
    assert_valid(
        "AudioPlan",
        &AudioPlan {
            voices: vec![AudioVoice {
                source_id: "a".to_string(),
                source_start: 0.0,
                duration: 48_000.0,
                fade_in: 0.0,
                fade_out: 100.0,
                at: MediaLocation::Node(param(None)),
            }],
            segments: vec![AudioSegment {
                voice: 0,
                start: 0,
                frames: 512,
                position: 0.0,
                step: 1.0,
                from: 0.0,
                to: 48_000.0,
                gain: [0.0, 1.0],
            }],
        },
    );
}

#[test]
fn frame_requirements_match_the_schema() {
    assert_valid(
        "FrameRequirements",
        &FrameRequirements {
            images: vec!["a".to_string()],
            videos: vec![VideoFrameRequirement {
                stream_id: "v".to_string(),
                frames: vec![1, 2, 3],
            }],
        },
    );
}
