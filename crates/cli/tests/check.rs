//! `--check` reads a composition without a GPU and says what is wrong with it.

use std::process::{Command, Output};

use serde_json::{Value, json};

fn check(composition: &Value, format: &[&str]) -> Output {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("composition.json");
    std::fs::write(&path, composition.to_string()).expect("write composition");
    Command::new(env!("CARGO_BIN_EXE_lumen"))
        .args(["--check", "--composition", path.to_str().unwrap()])
        .args(format)
        .args(["--media-root", dir.path().to_str().unwrap()])
        .env_remove("LUMEN_FONT_DIR")
        .output()
        .expect("run lumen")
}

fn text_composition(family: &str) -> Value {
    json!({
        "timeline": { "duration_frames": 1 },
        "render_settings": { "width": 16, "height": 16 },
        "nodes": [
            { "id": 1, "type": "text", "params": { "content": "a", "font_family": family } },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "to_node": 2, "to_port": "source" }]
    })
}

#[test]
fn errors_exit_1_and_warnings_alone_exit_0() {
    let mut invalid = text_composition("Roboto");
    invalid["nodes"][0]["params"]["opacityy"] = json!(1);
    let output = check(&invalid, &["--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["ok"], json!(false));
    assert_eq!(
        report["diagnostics"][0]["code"],
        json!("node.unknown_param")
    );
    assert_eq!(
        report["diagnostics"][0]["path"],
        json!("/nodes/0/params/opacityy")
    );

    let output = check(&text_composition("No Such Family 272"), &[]);
    assert_eq!(output.status.code(), Some(0));
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("warning font.fallback at /nodes/0/params/font_family: "),
        "{stdout}"
    );
    assert!(stdout.contains("0 errors, 1 warnings"), "{stdout}");

    let output = check(&text_composition("Roboto"), &["--format", "json"]);
    assert_eq!(output.status.code(), Some(0));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report, json!({ "ok": true, "diagnostics": [] }));
}

#[test]
fn asset_references_are_errors_without_an_asset_store() {
    let output = check(&text_composition("asset:01JCA2QX"), &["--format", "json"]);
    assert_eq!(output.status.code(), Some(1));
    let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    let diagnostics = report["diagnostics"].as_array().expect("diagnostics");
    assert_eq!(diagnostics.len(), 1, "{diagnostics:?}");
    assert_eq!(diagnostics[0]["code"], json!("media.not_found"));
    assert_eq!(diagnostics[0]["path"], json!("/nodes/0/params/font_family"));
}
