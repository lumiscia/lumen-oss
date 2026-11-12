//! Every JSON composition in `docs/3d.md` passes `lumen --check`, with no warnings either.

use std::process::Command;

use serde_json::{Value, json};

fn json_blocks(markdown: &str) -> Vec<Value> {
    let mut blocks = Vec::new();
    let mut current: Option<String> = None;
    for line in markdown.lines() {
        match (&mut current, line) {
            (None, "```json") => current = Some(String::new()),
            (Some(block), "```") => {
                blocks.push(serde_json::from_str(block).expect("a JSON block parses"));
                current = None;
            }
            (Some(block), line) => {
                block.push_str(line);
                block.push('\n');
            }
            (None, _) => {}
        }
    }
    blocks
}

#[test]
fn the_3d_guides_compositions_pass_check() {
    let docs = concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/3d.md");
    let blocks = json_blocks(&std::fs::read_to_string(docs).expect("docs/3d.md"));
    assert!(!blocks.is_empty(), "the guide has JSON blocks");

    let dir = tempfile::tempdir().expect("temp dir");
    for (index, mut block) in blocks.into_iter().enumerate() {
        // The guide shows `nodes` and `connections`; the rest is the same for every example.
        block["timeline"] = json!({ "duration_frames": 30 });
        block["render_settings"] = json!({ "width": 64, "height": 36 });
        let path = dir.path().join(format!("block-{index}.json"));
        std::fs::write(&path, block.to_string()).expect("write composition");
        let output = Command::new(env!("CARGO_BIN_EXE_lumen"))
            .args(["--check", "--format", "json", "--composition"])
            .arg(&path)
            .args(["--media-root", dir.path().to_str().unwrap()])
            .env_remove("LUMEN_FONT_DIR")
            .output()
            .expect("run lumen");
        let report: Value = serde_json::from_slice(&output.stdout).expect("JSON report");
        assert_eq!(
            report,
            json!({ "ok": true, "diagnostics": [] }),
            "JSON block {index} of docs/3d.md:\n{block:#}"
        );
    }
}
