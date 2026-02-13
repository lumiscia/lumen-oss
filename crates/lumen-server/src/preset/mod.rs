pub mod chat_story_v1;

use anyhow::{Context, anyhow};
use lumen::Project;
use serde_json::Value;

use crate::preset::chat_story_v1::{ChatStoryPresetV1, compile_chat_story_project};

pub fn project_from_payload(payload: &Value) -> anyhow::Result<Project> {
    let Some(object) = payload.as_object() else {
        return Err(anyhow!("project payload must be an object"));
    };

    let kind = object.get("kind").and_then(Value::as_str);
    if let Some(kind) = kind {
        if kind != "chat_story_v1" {
            return Err(anyhow!("unsupported preset kind `{kind}`"));
        }
        let preset: ChatStoryPresetV1 =
            serde_json::from_value(payload.clone()).context("failed to decode chat_story_v1")?;
        return compile_chat_story_project(&preset);
    }

    serde_json::from_value(payload.clone()).context("failed to decode raw project payload")
}
