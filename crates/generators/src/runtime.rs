//! The schema of the JSON values the engine returns while it runs, such as diagnostics and the
//! media inventory, built from the Rust types that serialise them.

use anyhow::{Context, Result, anyhow};
use lumen_3d::model::ModelInfo;
use lumen_engine::{
    audio::AudioPlan,
    diagnostic::{self, Diagnostic, Severity},
    error::MediaFailure,
    media::{FrameRequirements, MediaInventory},
};
use schemars::generate::SchemaSettings;
use serde_json::{Value, json};

/// The runtime schema, checked to be a valid JSON Schema.
pub fn render_runtime_schema_json() -> Result<String> {
    let schema = render_schema()?;
    jsonschema::meta::validate(&schema)
        .map_err(|error| anyhow!("generated runtime schema is invalid: {error}"))?;
    serde_json::to_string_pretty(&schema).context("failed to serialise the runtime schema")
}

fn render_schema() -> Result<Value> {
    let mut generator = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    generator.subschema_for::<Diagnostic>();
    generator.subschema_for::<MediaInventory>();
    generator.subschema_for::<MediaFailure>();
    generator.subschema_for::<ModelInfo>();
    generator.subschema_for::<AudioPlan>();
    generator.subschema_for::<FrameRequirements>();
    let mut defs = Value::Object(generator.take_definitions(true));
    drop_null_types(&mut defs);
    defs["DiagnosticCode"] = diagnostic_code_def();
    let code = defs
        .pointer_mut("/Diagnostic/properties/code")
        .context("the Diagnostic schema has no `code`")?;
    *code = json!({
        "description": code["description"],
        "$ref": "#/$defs/DiagnosticCode"
    });
    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": defs
    }))
}

/// An optional field is skipped when absent and never written as `null`, so its schema is its
/// type alone.
fn drop_null_types(value: &mut Value) {
    match value {
        Value::Object(object) => {
            if let Some(Value::Array(types)) = object.get_mut("type") {
                types.retain(|ty| ty != "null");
                if let [only] = types.as_slice() {
                    let only = only.clone();
                    object.insert("type".to_string(), only);
                }
            }
            object.values_mut().for_each(drop_null_types);
        }
        Value::Array(items) => items.iter_mut().for_each(drop_null_types),
        _ => {}
    }
}

fn diagnostic_code_def() -> Value {
    let mut codes: Vec<_> = diagnostic::code_list().collect();
    codes.sort_by_key(|(code, _)| *code);
    codes.dedup_by_key(|(code, _)| *code);
    let codes: Vec<Value> = codes
        .into_iter()
        .map(|(code, severity)| {
            json!({
                "code": code,
                "severity": match severity {
                    Severity::Error => "error",
                    Severity::Warning => "warning",
                }
            })
        })
        .collect();
    json!({
        "description": "A code a diagnostic can have. Codes are stable and public, and new ones can appear in any release, so treat one you don't know by its severity.",
        "type": "string",
        "x-lumen-codes": codes
    })
}
