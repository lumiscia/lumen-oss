use lumen::json::{
    JsonDelegateRequest, JsonDelegateStatus, convert_json_delegate, json_delegate_enabled,
};

#[test]
fn lumen_local_uses_lumen_json_delegate_capability() {
    assert!(json_delegate_enabled());

    let fixture =
        std::fs::read_to_string("../lumen/tests/fixtures/json_delegate/sample_project.json")
            .expect("read json delegate fixture");

    let result = convert_json_delegate(&JsonDelegateRequest {
        input_payload: fixture,
        input_schema_revision: "lumen_graph_v1".to_string(),
        caller_context: "lumen-local-tests".to_string(),
    });

    assert!(matches!(result.status, JsonDelegateStatus::Success));
    assert!(result.project_bundle.is_some());
}
