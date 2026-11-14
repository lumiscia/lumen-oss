//! The schema of `lumen-server`'s HTTP API, built from the `lumen-server-api` types. Values the
//! engine produces are `$ref`s into `runtime.schema.json`, and a request's composition is a `$ref`
//! to `composition.schema.json`.

use anyhow::{Context, Result, anyhow, bail};
use lumen_server_api::{
    CheckReport, CheckRequest, CompleteMultipartRequest, CreateMediaRequest, CreateRenderResponse,
    ErrorBody, Health, MediaResponse, MultipartUploadResponse, OutputFormat, PartResponse,
    RenderEvent, RenderRequest, RenderResponse, UploadTargetResponse, UrlMediaRequest,
};
use schemars::{SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

/// The API schema, checked to be a valid JSON Schema.
pub fn render_api_schema_json() -> Result<String> {
    let schema = render_schema()?;
    jsonschema::meta::validate(&schema)
        .map_err(|error| anyhow!("generated API schema is invalid: {error}"))?;
    serde_json::to_string_pretty(&schema).context("failed to serialise the API schema")
}

fn render_schema() -> Result<Value> {
    // Requests are described as they are read, so a field with a default is optional; responses
    // as they are written, so a field that is always written is required.
    let mut requests = SchemaSettings::draft2020_12().into_generator();
    requests.subschema_for::<RenderRequest>();
    requests.subschema_for::<CheckRequest>();
    requests.subschema_for::<CreateMediaRequest>();
    requests.subschema_for::<UrlMediaRequest>();
    requests.subschema_for::<CompleteMultipartRequest>();
    let mut responses = SchemaSettings::draft2020_12()
        .for_serialize()
        .into_generator();
    responses.subschema_for::<Health>();
    responses.subschema_for::<CheckReport>();
    responses.subschema_for::<CreateRenderResponse>();
    responses.subschema_for::<RenderResponse>();
    responses.subschema_for::<RenderEvent>();
    responses.subschema_for::<UploadTargetResponse>();
    responses.subschema_for::<MediaResponse>();
    responses.subschema_for::<MultipartUploadResponse>();
    responses.subschema_for::<PartResponse>();
    responses.subschema_for::<ErrorBody>();

    let mut defs = definitions(requests)?;
    for (name, def) in definitions(responses)? {
        match defs.get(&name) {
            Some(existing) if *existing != def => {
                bail!("`{name}` is described differently as a request and as a response")
            }
            _ => {
                defs.insert(name, def);
            }
        }
    }
    defs.insert("RenderEventType".to_string(), event_type_def(&defs)?);
    let formats = defs
        .get_mut("OutputFormat")
        .context("the API schema has no `OutputFormat`")?;
    formats["x-lumen-files"] = OutputFormat::ALL
        .iter()
        .map(|format| {
            json!({
                "format": format.name(),
                "contentType": format.content_type(),
                "extension": format.extension()
            })
        })
        .collect();
    Ok(json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "$defs": defs
    }))
}

fn definitions(mut generator: SchemaGenerator) -> Result<Map<String, Value>> {
    let mut defs = Value::Object(generator.take_definitions(true));
    drop_optional_nulls(&mut defs);
    match defs {
        Value::Object(defs) => Ok(defs),
        _ => bail!("schema definitions are not an object"),
    }
}

/// A field that is left out when it has no value is never `null`, so its schema is its type
/// alone. A field that is always there keeps `null` when it can be.
fn drop_optional_nulls(value: &mut Value) {
    match value {
        Value::Object(object) => {
            let required: Vec<Value> = object
                .get("required")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default();
            if let Some(Value::Object(properties)) = object.get_mut("properties") {
                for (name, property) in properties.iter_mut() {
                    if !required.iter().any(|field| field == name.as_str()) {
                        drop_null(property);
                    }
                }
            }
            object.values_mut().for_each(drop_optional_nulls);
        }
        Value::Array(items) => items.iter_mut().for_each(drop_optional_nulls),
        _ => {}
    }
}

fn drop_null(schema: &mut Value) {
    let Value::Object(object) = schema else {
        return;
    };
    if let Some(Value::Array(types)) = object.get_mut("type") {
        types.retain(|ty| ty != "null");
        if let [only] = types.as_slice() {
            let only = only.clone();
            object.insert("type".to_string(), only);
        }
    }
    if let Some(Value::Array(branches)) = object.get_mut("anyOf") {
        branches.retain(|branch| branch.get("type") != Some(&json!("null")));
        if let [only] = branches.as_slice() {
            let only = only.clone();
            object.remove("anyOf");
            if let Value::Object(only) = only {
                object.extend(only);
            }
        }
    }
}

/// The name of every socket event, as its own open string union.
fn event_type_def(defs: &Map<String, Value>) -> Result<Value> {
    let variants = defs
        .get("RenderEvent")
        .and_then(|event| event.get("oneOf"))
        .and_then(Value::as_array)
        .context("`RenderEvent` is not a union of events")?;
    let names = variants
        .iter()
        .map(|variant| {
            variant
                .pointer("/properties/type/const")
                .cloned()
                .context("a `RenderEvent` has no constant `type`")
        })
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({
        "description": "The `type` of a socket event. These are lumen-server's; another host can send others.",
        "type": "string",
        "enum": names,
        "x-lumen-open": true
    }))
}
