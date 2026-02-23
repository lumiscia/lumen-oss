#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonDelegateRequest {
    pub input_payload: String,
    pub input_schema_revision: String,
    pub caller_context: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum JsonDelegateStatus {
    Success,
    CapabilityDisabled,
    ValidationError,
    ConversionError,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonDelegateIssue {
    pub code: String,
    pub message: String,
    pub path: Option<String>,
    pub hint: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsonDelegateResult {
    pub status: JsonDelegateStatus,
    pub project_bundle_json: Option<String>,
    pub errors: Vec<JsonDelegateIssue>,
    pub warnings: Vec<JsonDelegateIssue>,
}

pub fn json_delegate_enabled() -> bool {
    cfg!(feature = "json")
}

pub fn convert_json_delegate(request: &JsonDelegateRequest) -> JsonDelegateResult {
    #[cfg(feature = "json")]
    {
        if request.input_payload.trim().is_empty() {
            return JsonDelegateResult {
                status: JsonDelegateStatus::ValidationError,
                project_bundle_json: None,
                errors: vec![JsonDelegateIssue {
                    code: "json_delegate.empty_payload".to_string(),
                    message: "input payload must not be empty".to_string(),
                    path: Some("$".to_string()),
                    hint: Some("Provide a non-empty JSON delegate payload".to_string()),
                }],
                warnings: Vec::new(),
            };
        }

        let parsed = serde_json::from_str::<serde_json::Value>(&request.input_payload);
        match parsed {
            Ok(payload) => JsonDelegateResult {
                status: JsonDelegateStatus::Success,
                project_bundle_json: Some(payload.to_string()),
                errors: Vec::new(),
                warnings: Vec::new(),
            },
            Err(error) => JsonDelegateResult {
                status: JsonDelegateStatus::ValidationError,
                project_bundle_json: None,
                errors: vec![JsonDelegateIssue {
                    code: "json_delegate.invalid_json".to_string(),
                    message: "input payload is not valid JSON".to_string(),
                    path: Some("$".to_string()),
                    hint: Some(error.to_string()),
                }],
                warnings: Vec::new(),
            },
        }
    }

    #[cfg(not(feature = "json"))]
    {
        let _ = request;
        JsonDelegateResult {
            status: JsonDelegateStatus::CapabilityDisabled,
            project_bundle_json: None,
            errors: vec![JsonDelegateIssue {
                code: "capability_disabled".to_string(),
                message: "json delegate capability is disabled in this build".to_string(),
                path: None,
                hint: Some("Rebuild with the `json` feature enabled".to_string()),
            }],
            warnings: Vec::new(),
        }
    }
}
