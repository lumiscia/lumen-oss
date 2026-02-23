#[cfg(not(feature = "json"))]
use std::collections::HashMap;

#[cfg(not(feature = "json"))]
use crate::Project;

#[cfg(feature = "json")]
mod enabled;

#[cfg(feature = "json")]
pub use enabled::{JsonProject, ProjectBundle};

#[cfg(not(feature = "json"))]
#[derive(Debug)]
pub struct ProjectBundle {
    pub project: Project,
    pub background: [u8; 4],
    pub image_sources: HashMap<String, String>,
}

pub const JSON_DELEGATE_CAPABILITY: &str = "json_delegate";
pub const JSON_DELEGATE_SCHEMA_REVISION: &str = "chat_story_v1";
pub const OBS_CODE_JSON_DELEGATE_SUCCESS: &str = "delegate.success";
pub const OBS_CODE_JSON_DELEGATE_CAPABILITY_DISABLED: &str = "delegate.capability_disabled";
pub const OBS_CODE_JSON_DELEGATE_VALIDATION_ERROR: &str = "delegate.validation_error";
pub const OBS_CODE_JSON_DELEGATE_CONVERSION_ERROR: &str = "delegate.conversion_error";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonDelegateRequest {
    pub input_payload: String,
    pub input_schema_revision: String,
    pub caller_context: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JsonDelegateStatus {
    Success,
    CapabilityDisabled,
    ValidationError,
    ConversionError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonDelegateIssue {
    pub code: String,
    pub observability_code: String,
    pub message: String,
    pub path: Option<String>,
    pub hint: Option<String>,
}

#[derive(Debug)]
pub struct JsonDelegateResult {
    pub status: JsonDelegateStatus,
    pub project_bundle: Option<ProjectBundle>,
    pub errors: Vec<JsonDelegateIssue>,
    pub warnings: Vec<JsonDelegateIssue>,
}

pub fn json_delegate_enabled() -> bool {
    cfg!(feature = "json")
}

pub fn json_delegate_observability_code(status: JsonDelegateStatus) -> &'static str {
    match status {
        JsonDelegateStatus::Success => OBS_CODE_JSON_DELEGATE_SUCCESS,
        JsonDelegateStatus::CapabilityDisabled => OBS_CODE_JSON_DELEGATE_CAPABILITY_DISABLED,
        JsonDelegateStatus::ValidationError => OBS_CODE_JSON_DELEGATE_VALIDATION_ERROR,
        JsonDelegateStatus::ConversionError => OBS_CODE_JSON_DELEGATE_CONVERSION_ERROR,
    }
}

fn build_issue(
    status: JsonDelegateStatus,
    code: &str,
    message: &str,
    path: Option<&str>,
    hint: Option<&str>,
) -> JsonDelegateIssue {
    JsonDelegateIssue {
        code: code.to_string(),
        observability_code: json_delegate_observability_code(status).to_string(),
        message: message.to_string(),
        path: path.map(ToString::to_string),
        hint: hint.map(ToString::to_string),
    }
}

pub fn convert_json_delegate(request: &JsonDelegateRequest) -> JsonDelegateResult {
    if request.input_schema_revision != JSON_DELEGATE_SCHEMA_REVISION {
        return JsonDelegateResult {
            status: JsonDelegateStatus::ValidationError,
            project_bundle: None,
            errors: vec![build_issue(
                JsonDelegateStatus::ValidationError,
                "validation_error",
                "unsupported schema revision",
                Some("$.input_schema_revision"),
                Some("Use schema revision `chat_story_v1`"),
            )],
            warnings: Vec::new(),
        };
    }

    #[cfg(feature = "json")]
    {
        let raw_json = match serde_json::from_str::<serde_json::Value>(&request.input_payload) {
            Ok(raw_json) => raw_json,
            Err(_) => {
                return JsonDelegateResult {
                    status: JsonDelegateStatus::ValidationError,
                    project_bundle: None,
                    errors: vec![build_issue(
                        JsonDelegateStatus::ValidationError,
                        "validation_error",
                        "input payload is not valid JSON",
                        Some("$"),
                        Some("Provide JSON payload matching `chat_story_v1`"),
                    )],
                    warnings: Vec::new(),
                };
            }
        };

        let parsed = match serde_json::from_value::<JsonProject>(raw_json) {
            Ok(parsed) => parsed,
            Err(_) => {
                return JsonDelegateResult {
                    status: JsonDelegateStatus::ValidationError,
                    project_bundle: None,
                    errors: vec![build_issue(
                        JsonDelegateStatus::ValidationError,
                        "validation_error",
                        "input payload does not match `chat_story_v1` schema",
                        Some("$"),
                        Some("Provide JSON payload matching `chat_story_v1`"),
                    )],
                    warnings: Vec::new(),
                };
            }
        };

        let bundle: Result<ProjectBundle, anyhow::Error> = parsed.try_into();
        match bundle {
            Ok(project_bundle) => JsonDelegateResult {
                status: JsonDelegateStatus::Success,
                project_bundle: Some(project_bundle),
                errors: Vec::new(),
                warnings: Vec::new(),
            },
            Err(_) => JsonDelegateResult {
                status: JsonDelegateStatus::ConversionError,
                project_bundle: None,
                errors: vec![build_issue(
                    JsonDelegateStatus::ConversionError,
                    "conversion_error",
                    "json delegate conversion failed",
                    Some("$"),
                    Some("Adjust unsupported fields to match `chat_story_v1` contract"),
                )],
                warnings: Vec::new(),
            },
        }
    }

    #[cfg(not(feature = "json"))]
    {
        let _ = request;
        JsonDelegateResult {
            status: JsonDelegateStatus::CapabilityDisabled,
            project_bundle: None,
            errors: vec![build_issue(
                JsonDelegateStatus::CapabilityDisabled,
                "capability_disabled",
                "json delegate capability is disabled in this build",
                None,
                Some("Rebuild with the `json` feature enabled"),
            )],
            warnings: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests;
