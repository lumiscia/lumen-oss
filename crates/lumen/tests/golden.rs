//! Golden-frame comparison test harness.
//!
//! Renders frame 0 of each benchmark fixture with both Vello and Skia backends,
//! then compares the output RGBA pixel data. The RMSE threshold is <= 2.0 per
//! pixel channel to account for acceptable rasterization differences.
//!
//! Requires both `renderer-vello` and `renderer-skia` features enabled:
//!   cargo test -p lumen --features "renderer-vello renderer-skia" --test golden

#![cfg(all(feature = "renderer-vello", feature = "renderer-skia"))]

use std::fs;
use std::path::PathBuf;

use lumen::{
    backend::{NoopFrameProvider, RenderBackend},
    compile::compile_project,
    model::Project,
};

/// Per-fixture RMSE thresholds. Text rendering differs significantly between
/// Vello (skrifa outline rasterization) and Skia (built-in font engine), so
/// text-heavy fixtures get a wider tolerance.
/// Per-fixture RMSE thresholds. Text rendering differs significantly between
/// Vello (skrifa outline rasterization) and Skia (built-in font engine), so
/// fixtures with text content get wider tolerance.
fn rmse_threshold(fixture_name: &str) -> f64 {
    match fixture_name {
        "text-heavy.json" => 8.0,
        "mixed-media.json" => 8.0,
        _ => 2.0,
    }
}

fn fixtures_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("bench")
        .join("fixtures")
}

fn golden_dir(backend: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("docs")
        .join("bench")
        .join("golden")
        .join(backend)
}

fn load_fixture(name: &str) -> Project {
    let path = fixtures_dir().join(name);
    let json = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("failed to read fixture {}: {e}", path.display()));
    serde_json::from_str(&json)
        .unwrap_or_else(|e| panic!("failed to parse fixture {}: {e}", path.display()))
}

fn compute_rmse(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len(), "frame buffers must have equal length");
    if a.is_empty() {
        return 0.0;
    }

    let sum_sq: f64 = a
        .iter()
        .zip(b.iter())
        .map(|(x, y)| {
            let diff = (*x as f64) - (*y as f64);
            diff * diff
        })
        .sum();

    (sum_sq / a.len() as f64).sqrt()
}

fn render_frame_vello(project: &Project) -> Vec<u8> {
    let timeline = compile_project(project).expect("compile");
    let mut renderer =
        lumen::backend::vello::GpuRenderer::new(timeline.canvas.width, timeline.canvas.height)
            .expect("vello init");
    let mut provider = NoopFrameProvider;
    renderer
        .render_frame(&timeline, 0, &mut provider)
        .expect("vello render")
}

fn render_frame_skia(project: &Project) -> Vec<u8> {
    let timeline = compile_project(project).expect("compile");
    let mut renderer =
        lumen::backend::skia::SkiaRenderer::new(timeline.canvas.width, timeline.canvas.height)
            .expect("skia init");
    let mut provider = NoopFrameProvider;
    renderer
        .render_frame(&timeline, 0, &mut provider)
        .expect("skia render")
}

fn save_golden(data: &[u8], width: u32, height: u32, backend: &str, fixture: &str) {
    let dir = golden_dir(backend);
    fs::create_dir_all(&dir).ok();
    let name = fixture.replace(".json", ".png");
    let path = dir.join(&name);
    let file = fs::File::create(&path).expect("create golden PNG");
    let encoder = image::codecs::png::PngEncoder::new(file);
    image::ImageEncoder::write_image(
        encoder,
        data,
        width,
        height,
        image::ExtendedColorType::Rgba8,
    )
    .expect("encode golden PNG");
}

#[test]
fn golden_frame_comparison() {
    let fixtures = ["vector-heavy.json", "text-heavy.json", "mixed-media.json"];

    for fixture_name in &fixtures {
        let fixture_path = fixtures_dir().join(fixture_name);
        if !fixture_path.exists() {
            eprintln!("skipping {fixture_name}: fixture not found");
            continue;
        }

        let project = load_fixture(fixture_name);
        let vello_frame = render_frame_vello(&project);

        save_golden(
            &vello_frame,
            project.canvas.width,
            project.canvas.height,
            "vello",
            fixture_name,
        );

        let skia_frame = render_frame_skia(&project);

        save_golden(
            &skia_frame,
            project.canvas.width,
            project.canvas.height,
            "skia",
            fixture_name,
        );

        let rmse = compute_rmse(&vello_frame, &skia_frame);
        let threshold = rmse_threshold(fixture_name);
        eprintln!("{fixture_name}: RMSE = {rmse:.4} (threshold: {threshold})");

        assert!(
            rmse <= threshold,
            "{fixture_name}: RMSE {rmse:.4} exceeds threshold {threshold}"
        );
    }
}
