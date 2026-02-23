use super::{
    JsonDelegateRequest,
    JsonDelegateStatus,
    OBS_CODE_JSON_DELEGATE_VALIDATION_ERROR,
    convert_json_delegate,
    json_delegate_enabled,
    json_delegate_observability_code,
};
#[cfg(not(feature = "json"))]
use super::OBS_CODE_JSON_DELEGATE_CAPABILITY_DISABLED;
#[cfg(feature = "json")]
use super::{OBS_CODE_JSON_DELEGATE_CONVERSION_ERROR, OBS_CODE_JSON_DELEGATE_SUCCESS};

fn request(payload: &str) -> JsonDelegateRequest {
	JsonDelegateRequest {
		input_payload: payload.to_string(),
		input_schema_revision: "chat_story_v1".to_string(),
		caller_context: "tests".to_string(),
	}
}

#[cfg(not(feature = "json"))]
#[test]
fn returns_deterministic_capability_disabled_when_feature_is_off() {
	assert!(!json_delegate_enabled());

	let first = convert_json_delegate(&request("{}"));
	let second = convert_json_delegate(&request("{}"));

	assert!(matches!(
		first.status,
		JsonDelegateStatus::CapabilityDisabled
	));
	assert!(matches!(
		second.status,
		JsonDelegateStatus::CapabilityDisabled
	));
	assert_eq!(
		first.errors.first().map(|issue| issue.code.as_str()),
		Some("capability_disabled")
	);
	assert_eq!(
		first.errors.first().map(|issue| issue.observability_code.as_str()),
		Some(OBS_CODE_JSON_DELEGATE_CAPABILITY_DISABLED)
	);
	assert_eq!(
		first.errors.first().and_then(|issue| issue.hint.as_deref()),
		second
			.errors
			.first()
			.and_then(|issue| issue.hint.as_deref())
	);
}

#[cfg(feature = "json")]
#[test]
fn converts_valid_payload_when_feature_is_on() {
	assert!(json_delegate_enabled());

	let raw = r#"{
		"canvas": { "width": 320, "height": 180, "background": [0, 0, 0, 255] },
		"timeline": { "fps": { "num": 30, "den": 1 }, "total_frames": 10 },
		"sources": [],
		"layers": []
	}"#;

	let result = convert_json_delegate(&request(raw));
	assert!(matches!(result.status, JsonDelegateStatus::Success));
	assert_eq!(
		json_delegate_observability_code(result.status),
		OBS_CODE_JSON_DELEGATE_SUCCESS
	);
	assert!(result.project_bundle.is_some());
	assert!(result.errors.is_empty());
}

#[cfg(feature = "json")]
#[test]
fn returns_validation_error_for_invalid_json_payload_without_echoing_secrets() {
	let secret = "SUPER_SECRET_TOKEN_42";
	let malformed = format!("{{\"apiKey\":\"{secret}\"");
	let result = convert_json_delegate(&request(malformed.as_str()));

	assert!(matches!(result.status, JsonDelegateStatus::ValidationError));
	assert_eq!(
		json_delegate_observability_code(result.status),
		OBS_CODE_JSON_DELEGATE_VALIDATION_ERROR
	);
	assert_eq!(
		result.errors.first().map(|issue| issue.code.as_str()),
		Some("validation_error")
	);
	assert_eq!(
		result
			.errors
			.first()
			.map(|issue| issue.observability_code.as_str()),
		Some(OBS_CODE_JSON_DELEGATE_VALIDATION_ERROR)
	);
	assert!(
		result
			.errors
			.first()
			.map(|issue| !issue.message.contains(secret))
			.unwrap_or(false)
	);
	assert!(
		result
			.errors
			.first()
			.and_then(|issue| issue.hint.as_ref())
			.map(|hint| !hint.contains(secret))
			.unwrap_or(false)
	);
	assert!(result.project_bundle.is_none());
}

#[cfg(feature = "json")]
#[test]
fn returns_conversion_error_without_echoing_sensitive_source_ids() {
	let secret = "TOP_SECRET_SOURCE_ID";
	let raw = format!(
		r#"{{
			"canvas": {{ "width": 320, "height": 180, "background": [0, 0, 0, 255] }},
			"timeline": {{ "fps": {{ "num": 30, "den": 1 }}, "total_frames": 10 }},
			"sources": [{{ "id": "{secret}", "media": "image", "kind": "file" }}],
			"layers": []
		}}"#
	);

	let result = convert_json_delegate(&request(raw.as_str()));

	assert!(matches!(result.status, JsonDelegateStatus::ConversionError));
	assert_eq!(
		json_delegate_observability_code(result.status),
		OBS_CODE_JSON_DELEGATE_CONVERSION_ERROR
	);
	assert_eq!(
		result.errors.first().map(|issue| issue.code.as_str()),
		Some("conversion_error")
	);
	assert_eq!(
		result
			.errors
			.first()
			.map(|issue| issue.observability_code.as_str()),
		Some(OBS_CODE_JSON_DELEGATE_CONVERSION_ERROR)
	);
	assert!(
		result
			.errors
			.first()
			.map(|issue| !issue.message.contains(secret))
			.unwrap_or(false)
	);
	assert!(
		result
			.errors
			.first()
			.and_then(|issue| issue.hint.as_ref())
			.map(|hint| !hint.contains(secret))
			.unwrap_or(false)
	);
	assert!(result.project_bundle.is_none());
}

#[test]
fn returns_validation_error_for_unsupported_schema_revision() {
	let result = convert_json_delegate(&JsonDelegateRequest {
		input_payload: "{}".to_string(),
		input_schema_revision: "legacy_v0".to_string(),
		caller_context: "tests".to_string(),
	});

	assert!(matches!(result.status, JsonDelegateStatus::ValidationError));
	assert_eq!(
		json_delegate_observability_code(result.status),
		OBS_CODE_JSON_DELEGATE_VALIDATION_ERROR
	);
	assert_eq!(
		result.errors.first().map(|issue| issue.code.as_str()),
		Some("validation_error")
	);
	assert_eq!(
		result
			.errors
			.first()
			.map(|issue| issue.observability_code.as_str()),
		Some(OBS_CODE_JSON_DELEGATE_VALIDATION_ERROR)
	);
}
