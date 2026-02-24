#[cfg(feature = "json")]
mod tests {
    use std::sync::{Arc, RwLock};

    use lumen::{
        AssetCache, Composition, NullMediaStore, RasterFrame, RenderContext,
        RuntimeCapabilityProfile, SurfacePool,
    };

    #[test]
    fn from_json_parses_schema_and_renders_frame() {
        let payload = r#"
		{
		  "schema_revision": "lumen_graph_v1",
		  "timeline": { "fps": 30.0, "duration_frames": 10 },
		  "render_settings": {
			"width": 2,
			"height": 1,
			"background_color": [0, 0, 0, 0]
		  },
		  "graph": {
			"nodes": [
			  {
				"id": 1,
				"kind": {
				  "type": "solid_color",
				  "color": [255, 0, 0, 255],
				  "width": 2,
				  "height": 1
				}
			  },
			  {
				"id": 2,
				"kind": { "type": "media_output" }
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
		  }
		}
		"#;

        let result = Composition::from_json(payload);
        assert!(result.errors.is_empty());
        let composition = result.composition.expect("composition should parse");

        let mut context = RenderContext::new(
            &composition,
            Arc::new(SurfacePool::new()),
            Arc::new(RwLock::new(AssetCache::new())),
            Arc::new(NullMediaStore),
            RuntimeCapabilityProfile::cpu_only(),
        );
        let frame = composition
            .render_frame(0, &mut context)
            .expect("frame should render");
        let RasterFrame::Bitmap(bytes, width, height) = frame else {
            panic!("expected bitmap frame");
        };
        assert_eq!((width, height), (2, 1));
        assert_eq!(bytes.as_slice(), &[255, 0, 0, 255, 255, 0, 0, 255]);
    }
}
