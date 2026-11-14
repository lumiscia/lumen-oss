//! The OpenAPI document of `lumen-server`'s HTTP API, built from the `lumen-server-api` types and
//! its table of routes. Values the engine produces are `$ref`s into `runtime.schema.json`, and a
//! request's composition is a `$ref` to `composition.schema.json`.

use anyhow::{Context, Result, anyhow, bail};
use lumen_server_api::{
    ANY_RENDER_FAILURE, Body, CheckReport, CheckRequest, CompleteMultipartRequest,
    CreateMediaRequest, CreateRenderResponse, ErrorBody, ErrorCode, Health, MediaResponse,
    MultipartUploadResponse, OPERATIONS, Operation, OutputFormat, PartResponse, RenderEvent,
    RenderRequest, RenderResponse, RenderWebhook, UploadTargetResponse, UrlMediaRequest,
};
use schemars::{SchemaGenerator, generate::SchemaSettings};
use serde_json::{Map, Value, json};

const SCHEMAS: &str = "#/components/schemas/";

/// The OpenAPI 3.1 document, with each of its schemas checked to be a valid JSON Schema.
pub fn render_openapi_json() -> Result<String> {
    let schemas = schemas()?;
    for (name, schema) in &schemas {
        jsonschema::meta::validate(schema)
            .map_err(|error| anyhow!("generated API schema `{name}` is invalid: {error}"))?;
    }
    let document = document(schemas)?;
    serde_json::to_string_pretty(&document).context("failed to serialise the OpenAPI document")
}

fn document(schemas: Map<String, Value>) -> Result<Value> {
    let mut paths = Map::new();
    for operation in OPERATIONS {
        for name in named_schemas(operation) {
            if !schemas.contains_key(name) {
                bail!(
                    "`{}` names `{name}`, which the API has no schema for",
                    operation.id
                );
            }
        }
        let path = paths
            .entry(operation.path)
            .or_insert_with(|| json!({}))
            .as_object_mut()
            .context("a path is not an object")?;
        path.insert(
            operation.method.as_str().to_ascii_lowercase(),
            operation_object(operation),
        );
    }
    Ok(json!({
        "openapi": "3.1.0",
        "info": {
            "title": "Lumen API",
            "version": lumen_engine::SCHEMA_VERSION,
            "description": "The HTTP API of lumen-server, which other hosts of Lumen implement too. A host can leave routes out and add values of its own where a schema is marked `x-lumen-open`."
        },
        "tags": [
            { "name": "renders", "description": "Rendering compositions." },
            { "name": "media", "description": "Uploaded files a composition uses as `asset:<id>`." },
            { "name": "health" }
        ],
        "paths": paths,
        "webhooks": { "render": { "post": webhook_object() } },
        "components": {
            "schemas": schemas,
            "securitySchemes": { "token": { "type": "http", "scheme": "bearer" } }
        }
    }))
}

/// The types an operation takes and answers with.
fn named_schemas(operation: &Operation) -> Vec<&'static str> {
    std::iter::once(operation.request)
        .chain(operation.success.iter().map(|(_, body)| *body))
        .filter_map(|body| match body {
            Body::Json(name) | Body::Socket(name) => Some(name),
            Body::Empty | Body::Bytes => None,
        })
        .collect()
}

fn json_content(name: &str) -> Value {
    json!({ "application/json": { "schema": { "$ref": format!("{SCHEMAS}{name}") } } })
}

fn operation_object(operation: &Operation) -> Value {
    let mut object = json!({
        "operationId": operation.id,
        "summary": operation.summary,
        "tags": [operation.tag],
        "security": if operation.token { json!([{ "token": [] }]) } else { json!([]) },
    });
    if !operation.params.is_empty() {
        object["parameters"] = operation
            .params
            .iter()
            .map(|param| {
                json!({
                    "name": param.name,
                    "in": "path",
                    "required": true,
                    "description": param.description,
                    "schema": if param.integer {
                        json!({ "type": "integer", "minimum": 1 })
                    } else {
                        json!({ "type": "string" })
                    }
                })
            })
            .collect();
    }
    match operation.request {
        Body::Json(name) => {
            object["requestBody"] = json!({ "required": true, "content": json_content(name) });
        }
        Body::Bytes => {
            object["requestBody"] = json!({
                "required": true,
                "description": "The file's bytes.",
                "content": { "application/octet-stream": {} }
            });
        }
        Body::Empty | Body::Socket(_) => {}
    }
    let mut responses = Map::new();
    for (status, body) in operation.success {
        let response = match body {
            Body::Json(name) => json!({ "description": "Success.", "content": json_content(name) }),
            Body::Bytes => json!({
                "description": "The file, with its own content type.",
                "content": { "*/*": {} }
            }),
            Body::Empty => json!({ "description": "Success, with no body." }),
            Body::Socket(name) => {
                object["x-lumen-socket"] =
                    json!({ "message": { "$ref": format!("{SCHEMAS}{name}") } });
                json!({ "description": "The connection becomes a WebSocket, whose messages are JSON." })
            }
        };
        responses.insert(status.to_string(), response);
    }
    for (status, codes) in operation.errors() {
        responses.insert(status.to_string(), error_response(&codes));
    }
    object["responses"] = Value::Object(responses);
    object
}

/// An error answer, which says each code it can carry and lists them in `x-lumen-codes`.
fn error_response(codes: &[&str]) -> Value {
    let lines: Vec<String> = codes
        .iter()
        .map(|code| {
            if *code == ANY_RENDER_FAILURE {
                return "The code of the render's own failure, as its `error.code`.".to_string();
            }
            let doc = ErrorCode::LISTED
                .iter()
                .find(|(listed, _)| listed == code)
                .map_or("", |(_, doc)| doc.trim());
            format!("`{code}`: {doc}")
        })
        .collect();
    json!({
        "description": lines.join("\n\n"),
        "content": json_content("ErrorBody"),
        "x-lumen-codes": codes
    })
}

/// What a host that delivers webhooks sends, signed as Standard Webhooks.
fn webhook_object() -> Value {
    let header = |name: &str, description: &str| {
        json!({
            "name": name,
            "in": "header",
            "required": true,
            "description": description,
            "schema": { "type": "string" }
        })
    };
    json!({
        "operationId": "renderWebhook",
        "summary": "Sent to a render's `webhookUrl` when the render ends, signed as a Standard Webhooks request. lumen-server delivers none.",
        "tags": ["renders"],
        "parameters": [
            header("webhook-id", "The message's id, the same on every delivery of it."),
            header("webhook-timestamp", "When this delivery was sent, in seconds since the Unix epoch."),
            header("webhook-signature", "Space-separated signatures of `{id}.{timestamp}.{body}`, each `v1,` and base64 HMAC-SHA256.")
        ],
        "requestBody": { "required": true, "content": json_content("RenderWebhook") },
        "responses": { "2XX": { "description": "The webhook was received." } }
    })
}

/// Every type of the API, by name.
fn schemas() -> Result<Map<String, Value>> {
    let settings = |settings: SchemaSettings| {
        settings.with(|settings| settings.definitions_path = "/components/schemas".into())
    };
    // Requests are described as they are read, so a field with a default is optional; responses
    // as they are written, so a field that is always written is required.
    let mut requests = settings(SchemaSettings::draft2020_12()).into_generator();
    requests.subschema_for::<RenderRequest>();
    requests.subschema_for::<CheckRequest>();
    requests.subschema_for::<CreateMediaRequest>();
    requests.subschema_for::<UrlMediaRequest>();
    requests.subschema_for::<CompleteMultipartRequest>();
    let mut responses = settings(SchemaSettings::draft2020_12().for_serialize()).into_generator();
    responses.subschema_for::<Health>();
    responses.subschema_for::<CheckReport>();
    responses.subschema_for::<CreateRenderResponse>();
    responses.subschema_for::<RenderResponse>();
    responses.subschema_for::<RenderEvent>();
    responses.subschema_for::<RenderWebhook>();
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
        .context("the API has no `OutputFormat`")?;
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
    Ok(defs)
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
