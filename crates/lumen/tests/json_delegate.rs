#[cfg(feature = "json")]
mod tests {
    use std::sync::{Arc, RwLock};

    use lumen::{
        AssetCache, Composition, NodeKind, NullMediaStore, RasterFrame, RenderContext,
        RuntimeCapabilityProfile, SurfacePool,
    };

    #[test]
    fn from_json_parses_schema_and_renders_frame() {
        let payload = r#"
		{
		  "schema_revision": "lumen_graph_v2",
		  "timeline": { "fps": 30.0, "duration_frames": 10 },
		  "render_settings": {
			"width": 2,
			"height": 1,
			"background_color": [0, 0, 0, 0]
		  },
		  "graph": [
			{
			  "id": "solid",
			  "kind": {
				"type": "solid_color",
				"color": [255, 0, 0, 255],
				"width": 2,
				"height": 1
			  },
			  "inputs": {}
			},
			{
			  "id": "out",
			  "kind": { "type": "media_output" },
			  "inputs": {
				"source": { "node": "solid", "port": "output" }
			  }
			}
		  ]
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
        let RasterFrame::Bitmap(bitmap) = frame else {
            panic!("expected bitmap frame");
        };
        assert_eq!((bitmap.storage_width, bitmap.storage_height), (2, 1));
        assert_eq!(bitmap.pixels.as_slice(), &[255, 0, 0, 255, 255, 0, 0, 255]);
    }

    #[test]
    fn component_inline_dynamics_and_dotted_refs_sample_correctly() {
        let payload = r#"
		{
		  "schema_revision": "lumen_graph_v2",
		  "components": {
			"my_custom_shape": {
			  "props": {
				"pos_y": { "type": "int", "name": "Y Position", "default": 0 }
			  },
			  "inputs": {},
			  "outputs": {
				"output": {
				  "kind": "raster_frame",
				  "name": "Output",
				  "source": { "node": "renderer", "port": "output" }
				}
			  },
			  "graph": [
				{
				  "id": "shape",
				  "kind": {
					"type": "shape",
					"geometry": { "type": "rectangle", "width": 32, "height": 16, "border_radius": 0 },
					"position": { "x": 0, "y": { "expr": "component.pos_y" } }
				  },
				  "inputs": {}
				},
				{
				  "id": "renderer",
				  "kind": {
					"type": "shape_renderer",
					"fill_color": [255,255,255,255],
					"stroke_color": [0,0,0,255],
					"stroke_width": 0,
					"fill_enabled": true,
					"stroke_enabled": false
				  },
				  "inputs": {
					"vector": { "node": "shape", "port": "vector" }
				  }
				}
			  ]
			}
		  },
		  "timeline": { "fps": 30.0, "duration_frames": 20 },
		  "render_settings": { "width": 64, "height": 64, "background_color": [0,0,0,0] },
		  "graph": [
			{
			  "id": "hero_box",
			  "kind": {
				"type": "component",
				"component": "my_custom_shape",
				"props": {
				  "pos_y": {
					"anim": {
					  "keys": [
						{ "frame": 0, "value": 0, "interpolation": "linear" },
						{ "frame": 10, "value": { "expr": "frame * 2" }, "interpolation": "linear" }
					  ],
					  "before_extrapolation": "hold",
					  "after_extrapolation": "hold"
					}
				  }
				}
			  },
			  "inputs": {}
			},
			{
			  "id": "move",
			  "kind": {
				"type": "transform",
				"translate_y": { "expr": "hero_box.shape.position.y" }
			  },
			  "inputs": {
				"source": { "node": "hero_box", "port": "output" }
			  }
			},
			{
			  "id": "out",
			  "kind": { "type": "media_output" },
			  "inputs": {
				"source": { "node": "move", "port": "output" }
			  }
			}
		  ]
		}
		"#;

        let result = Composition::from_json(payload);
        assert!(
            result.errors.is_empty(),
            "expected no errors, got: {:?}",
            result.errors
        );
        let composition = result.composition.expect("composition should parse");

        let mut context = RenderContext::new(
            &composition,
            Arc::new(SurfacePool::new()),
            Arc::new(RwLock::new(AssetCache::new())),
            Arc::new(NullMediaStore),
            RuntimeCapabilityProfile::cpu_only(),
        );

        let shape_id = composition
            .graph
            .nodes
            .iter()
            .find_map(|(id, node)| matches!(node.kind, NodeKind::Shape(_)).then_some(*id))
            .expect("shape node should be flattened");
        let transform_id = composition
            .graph
            .nodes
            .iter()
            .find_map(|(id, node)| matches!(node.kind, NodeKind::Transform(_)).then_some(*id))
            .expect("transform node should exist");

        context.request.frame = 5;
        let shape_y = composition
            .sample_property(shape_id, "position.y", 5, &context)
            .expect("shape position.y should sample");
        let transform_y = composition
            .sample_property(transform_id, "translate_y", 5, &context)
            .expect("transform translate_y should sample");
        assert_eq!(shape_y, lumen::PropertyValue::Float(10.0));
        assert_eq!(transform_y, lumen::PropertyValue::Float(10.0));

        context.request.frame = 10;
        let shape_y_end = composition
            .sample_property(shape_id, "position.y", 10, &context)
            .expect("shape position.y at end should sample");
        assert_eq!(shape_y_end, lumen::PropertyValue::Float(20.0));
    }
}
