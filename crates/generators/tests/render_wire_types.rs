//! The render types the HTTP API carries are defined twice: in `lumen_engine::render`, and in
//! `lumen-server-api`, which doesn't depend on the engine. Each pair must have the same schema.

use lumen_engine::render;
use schemars::{JsonSchema, schema_for};

fn assert_same_schema<Engine: JsonSchema, Wire: JsonSchema>() {
    assert_eq!(
        serde_json::to_value(schema_for!(Engine)).unwrap(),
        serde_json::to_value(schema_for!(Wire)).unwrap(),
    );
}

#[test]
fn the_engine_and_the_api_spell_render_outputs_the_same() {
    assert_same_schema::<render::OutputFormat, lumen_server_api::OutputFormat>();
    assert_same_schema::<render::FrameRange, lumen_server_api::FrameRange>();
}
