//! JSON delegate API surface for constructing compositions from serialized payloads.

use crate::{
	composition::Composition,
	error::{LumenError, Warning},
};

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
		#[cfg(feature = "json")]
		{
			match serde_json::from_str::<serde_json::Value>(input) {
				Ok(_) => JsonDelegateResult {
					status: JsonDelegateStatus::ConversionError,
					composition: None,
					errors: Vec::new(),
					warnings: Vec::new(),
				},
				Err(_) => JsonDelegateResult {
					status: JsonDelegateStatus::ValidationError,
					composition: None,
					errors: Vec::new(),
					warnings: Vec::new(),
				},
			}
		}

		#[cfg(not(feature = "json"))]
		{
			let _ = input;
			JsonDelegateResult {
				status: JsonDelegateStatus::ValidationError,
				composition: None,
				errors: Vec::new(),
				warnings: Vec::new(),
			}
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
