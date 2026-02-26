//! JSON delegate API surface for constructing compositions from serialized payloads.

mod convert;
mod schema;

use crate::{
    composition::Composition,
    error::{ExpressionError, LumenError, Warning},
};
use convert::convert_json_composition;
use schema::JsonComposition;

pub const SCHEMA_REVISION: &str = "lumen_graph_v2";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonDelegateStatus {
    Success,
    ValidationError,
    ConversionError,
}

#[derive(Debug)]
pub struct JsonDelegateResult {
    pub status: JsonDelegateStatus,
    pub composition: Option<Composition>,
    pub errors: Vec<LumenError>,
    pub warnings: Vec<Warning>,
}

pub trait JsonDelegate {
    fn from_json(input: &str) -> JsonDelegateResult;
}

impl JsonDelegate for Composition {
    fn from_json(input: &str) -> JsonDelegateResult {
        Composition::from_json(input)
    }
}

impl Composition {
    pub fn from_json(input: &str) -> JsonDelegateResult {
        let payload = match serde_json::from_str::<JsonComposition>(input) {
            Ok(payload) => payload,
            Err(error) => {
                return JsonDelegateResult {
                    status: JsonDelegateStatus::ValidationError,
                    composition: None,
                    errors: vec![
                        ExpressionError::Parse {
                            node_id: None,
                            property_path: Some("json".to_string()),
                            details: error.to_string(),
                        }
                        .into(),
                    ],
                    warnings: Vec::new(),
                };
            }
        };

        if payload.schema_revision != SCHEMA_REVISION {
            return JsonDelegateResult {
                status: JsonDelegateStatus::ValidationError,
                composition: None,
                errors: vec![
                    ExpressionError::Parse {
                        node_id: None,
                        property_path: Some("schema_revision".to_string()),
                        details: format!(
                            "expected schema revision `{SCHEMA_REVISION}`, got `{}`",
                            payload.schema_revision
                        ),
                    }
                    .into(),
                ],
                warnings: Vec::new(),
            };
        }

        let composition = match convert_json_composition(payload) {
            Ok(composition) => composition,
            Err(errors) => {
                return JsonDelegateResult {
                    status: JsonDelegateStatus::ConversionError,
                    composition: None,
                    errors,
                    warnings: Vec::new(),
                };
            }
        };

        match composition.validate_structure() {
            Ok(warnings) => JsonDelegateResult {
                status: JsonDelegateStatus::Success,
                composition: Some(composition),
                errors: Vec::new(),
                warnings,
            },
            Err(errors) => JsonDelegateResult {
                status: JsonDelegateStatus::ValidationError,
                composition: None,
                errors,
                warnings: Vec::new(),
            },
        }
    }
}

impl TryFrom<&str> for Composition {
    type Error = JsonDelegateResult;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let result = Composition::from_json(value);
        if let Some(composition) = result.composition {
            Ok(composition)
        } else {
            Err(result)
        }
    }
}
