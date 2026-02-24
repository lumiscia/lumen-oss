use lumen::{
    Composition,
    json::{JsonDelegateStatus, SCHEMA_REVISION},
};

#[test]
fn lumen_local_parses_composition_json_delegate_payload() {
    let payload = format!(
        r#"{{
  "schema_revision": "{SCHEMA_REVISION}",
  "timeline": {{ "fps": 30.0, "duration_frames": 2 }},
  "render_settings": {{
    "width": 2,
    "height": 1,
    "background_color": [0, 0, 0, 0]
  }},
  "graph": {{
    "nodes": [
      {{
        "id": 1,
        "kind": {{
          "type": "solid_color",
          "color": [255, 0, 0, 255],
          "width": 2,
          "height": 1
        }}
      }},
      {{
        "id": 2,
        "kind": {{ "type": "media_output" }}
      }}
    ],
    "connections": [
      {{
        "from_node": 1,
        "from_port": "output",
        "to_node": 2,
        "to_port": "source"
      }}
    ]
  }}
}}"#
    );

    let result = Composition::from_json(payload.as_str());

    assert_eq!(result.status, JsonDelegateStatus::Success);
    assert!(result.errors.is_empty());
    assert!(result.composition.is_some());
}
