//! Checks what the server answers against `definitions/openapi.json`: every response the tests
//! read through [`checked`] has a status its operation describes, a body its schema accepts and,
//! when it is an error, a code the operation lists for that status.

use std::sync::OnceLock;

use lumen_server_api::{ANY_RENDER_FAILURE, Body, Operation};
use reqwest::{StatusCode, header::HeaderMap};
use serde::de::DeserializeOwned;
use serde_json::{Value, json};

const BASE: &str = "file:///definitions/";

fn registry() -> &'static jsonschema::Registry<'static> {
    static REGISTRY: OnceLock<jsonschema::Registry<'static>> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let parse = |json: &str| serde_json::from_str::<Value>(json).expect("a definition parses");
        jsonschema::Registry::new()
            .add(
                format!("{BASE}openapi.json"),
                parse(include_str!("../../../../definitions/openapi.json")),
            )
            .expect("openapi.json")
            .add(
                format!("{BASE}runtime.schema.json"),
                parse(include_str!("../../../../definitions/runtime.schema.json")),
            )
            .expect("runtime.schema.json")
            .add(
                format!("{BASE}composition.schema.json"),
                parse(include_str!(
                    "../../../../definitions/composition.schema.json"
                )),
            )
            .expect("composition.schema.json")
            .prepare()
            .expect("the definitions refer only to each other")
    })
}

/// What is wrong with `value` as the API's type `name`.
fn schema_errors(name: &str, value: &Value) -> Vec<String> {
    let schema = json!({ "$ref": format!("{BASE}openapi.json#/components/schemas/{name}") });
    let validator = jsonschema::options()
        .with_registry(registry())
        .build(&schema)
        .expect("the API's schemas are valid");
    validator
        .iter_errors(value)
        .map(|error| format!("{}: {error}", error.instance_path()))
        .collect()
}

/// A response the server gave, already checked against the API.
#[derive(Debug)]
pub struct Response {
    status: StatusCode,
    headers: HeaderMap,
    body: Vec<u8>,
}

impl Response {
    pub fn status(&self) -> StatusCode {
        self.status
    }

    pub fn headers(&self) -> &HeaderMap {
        &self.headers
    }

    pub fn json<T: DeserializeOwned>(self) -> serde_json::Result<T> {
        serde_json::from_slice(&self.body)
    }

    pub fn bytes(self) -> Result<Vec<u8>, std::convert::Infallible> {
        Ok(self.body)
    }
}

/// Reads `response`, the answer to `method` on `path` with the JSON body `request`, and panics
/// with what the API doesn't describe about it.
pub fn checked(
    method: &str,
    path: &str,
    request: Option<&Value>,
    response: reqwest::blocking::Response,
) -> Response {
    let response = Response {
        status: response.status(),
        headers: response.headers().clone(),
        body: response.bytes().expect("response body").to_vec(),
    };
    let path = path.split('?').next().unwrap_or(path);
    let Some(operation) = Operation::of(method, path) else {
        assert!(
            matches!(
                response.status,
                StatusCode::NOT_FOUND | StatusCode::METHOD_NOT_ALLOWED
            ),
            "{method} {path} is not in the API, and answered {}",
            response.status
        );
        return response;
    };
    let status = response.status.as_u16();
    let body = || -> Value {
        serde_json::from_slice(&response.body).unwrap_or_else(|error| {
            panic!("{} answered {status} without JSON: {error}", operation.id)
        })
    };
    if let Some((_, answer)) = operation.success.iter().find(|(known, _)| *known == status) {
        if let Body::Json(name) = answer {
            let errors = schema_errors(name, &body());
            assert!(
                errors.is_empty(),
                "{} {status} is not a `{name}`: {errors:#?}",
                operation.id
            );
        }
        // A request the server took is one the API describes.
        if let (Body::Json(name), Some(request)) = (operation.request, request) {
            let errors = schema_errors(name, request);
            assert!(
                errors.is_empty(),
                "{} took a request that is not a `{name}`: {errors:#?}",
                operation.id
            );
        }
        return response;
    }
    let errors = operation.errors();
    let Some((_, codes)) = errors.iter().find(|(known, _)| *known == status) else {
        panic!(
            "{} answered {status}, which the API doesn't describe for it: {}",
            operation.id,
            String::from_utf8_lossy(&response.body)
        );
    };
    let body = body();
    let invalid = schema_errors("ErrorBody", &body);
    assert!(
        invalid.is_empty(),
        "{} {status} is not an `ErrorBody`: {invalid:#?}",
        operation.id
    );
    let code = body["code"].as_str().unwrap_or_default();
    assert!(
        codes.contains(&code) || codes.contains(&ANY_RENDER_FAILURE),
        "{} answered {status} with `{code}`, and the API lists {codes:?} for it",
        operation.id
    );
    response
}
