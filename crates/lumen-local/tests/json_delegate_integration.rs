use lumen::json;

#[test]
fn lumen_local_parses_composition_json() {
    let payload = r#"{
  "timeline": { "fps": 30.0, "duration_frames": 2 },
  "render_settings": {
    "width": 2,
    "height": 1,
    "background_color": [0, 0, 0, 0]
  },
  "nodes": [
    {
      "id": 1,
      "type": "solid_color",
      "properties": {
        "color": [255, 0, 0, 255],
        "width": 2,
        "height": 1
      }
    },
    {
      "id": 2,
      "type": "media_output"
    }
  ],
  "connections": [
    {
      "from_node": 1,
      "from_port": "output",
      "to_node": 2,
      "to_port": "source"
    }
  ]
}"#;

    let composition = json::parse(payload).expect("should parse composition JSON");
    assert_eq!(composition.graph.nodes.len(), 2);
    assert_eq!(composition.graph.connections.len(), 1);
    assert_eq!(composition.timeline.duration_frames, 2);
    assert_eq!(composition.render_settings.width, 2);
    assert_eq!(composition.render_settings.height, 1);
}
