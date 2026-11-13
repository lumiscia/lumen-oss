#![cfg(target_os = "linux")]

//! Drives the real `lumen preview` on lavapipe and reads the sheet back. Skips the renders when
//! lavapipe isn't installed.

use std::{
    fs,
    path::{Path, PathBuf},
    process::{Command, Output},
};

use image::RgbImage;
use serde_json::json;

/// Between cells and around the edge, as `lumen preview` lays them out.
const GUTTER: u32 = 8;
const BACKGROUND: [u8; 3] = [40, 40, 40];

fn lavapipe_icd() -> Option<PathBuf> {
    let icd = fs::read_dir("/usr/share/vulkan/icd.d")
        .ok()?
        .filter_map(|entry| Some(entry.ok()?.path()))
        .find(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with("lvp_icd"))
        });
    if icd.is_none() {
        eprintln!("lavapipe not installed; skipping lumen preview renders");
    }
    icd
}

fn lumen(icd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_lumen"))
        .args(args)
        .env_remove("LUMEN_HARDWARE_DECODE")
        .env("VK_ICD_FILENAMES", icd)
        .env("LUMEN_GPU_FORCE_FALLBACK_ADAPTER", "1")
        .env("WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER", "1")
        .env("WGPU_BACKEND", "vulkan")
        .env("WGPU_POWER_PREF", "none")
        .env("NO_COLOR", "1")
        .output()
        .expect("run lumen")
}

fn succeeded(output: Output) -> String {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("utf-8 stdout")
}

/// A 64 × 48 timeline of 30 frames whose red grows with the frame number.
fn write_composition(directory: &Path) -> PathBuf {
    let path = directory.join("composition.json");
    let composition = json!({
        "timeline": { "fps": 30, "duration_frames": 30 },
        "render_settings": { "width": 64, "height": 48 },
        "nodes": [
            {
                "id": 1,
                "type": "background",
                "params": { "paint": "=tween(#000000, #ff0000, 0, 29, 'linear')", "width": 64, "height": 48 }
            },
            { "id": 2, "type": "media_output" }
        ],
        "connections": [{ "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }]
    });
    fs::write(&path, composition.to_string()).expect("write composition");
    path
}

/// The centre pixel of each cell of a `columns` × `rows` grid of `cell` (width, height) cells.
fn cell_centres(sheet: &RgbImage, columns: u32, rows: u32, cell: (u32, u32)) -> Vec<[u8; 3]> {
    let (width, height) = cell;
    (0..rows * columns)
        .map(|index| {
            let x = GUTTER + (index % columns) * (width + GUTTER) + width / 2;
            let y = GUTTER + (index / columns) * (height + GUTTER) + height / 2;
            sheet.get_pixel(x, y).0
        })
        .collect()
}

#[test]
fn the_default_sheet_is_twelve_frames_four_to_a_row() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().unwrap();
    let composition = write_composition(directory.path());
    let sheet = directory.path().join("sheet.png");
    let stdout = succeeded(lumen(
        &icd,
        &[
            "preview",
            composition.to_str().unwrap(),
            "-o",
            sheet.to_str().unwrap(),
        ],
    ));
    assert!(
        stdout.contains("frames=12 grid=4x3 size=1920x1091"),
        "{stdout}"
    );

    // 1920 wide leaves (1920 - 5 gutters) / 4 = 470 per cell, and 470 * 48 / 64 rounds to 353.
    let sheet = image::open(&sheet).unwrap().to_rgb8();
    assert_eq!(sheet.dimensions(), (1920, 3 * 353 + 4 * GUTTER));
    assert_eq!(sheet.get_pixel(GUTTER / 2, GUTTER / 2).0, BACKGROUND);
    let reds: Vec<u8> = cell_centres(&sheet, 4, 3, (470, 353))
        .iter()
        .map(|pixel| pixel[0])
        .collect();
    assert!(reds.windows(2).all(|pair| pair[0] < pair[1]), "{reds:?}");
    assert!(reds[0] < 5 && reds[11] > 250, "{reds:?}");
}

#[test]
fn a_last_partial_row_starts_at_the_left() {
    let Some(icd) = lavapipe_icd() else { return };
    let directory = tempfile::tempdir().unwrap();
    let composition = write_composition(directory.path());
    let composition = composition.to_str().unwrap();
    let sheet = directory.path().join("sheet.jpg");
    let stdout = succeeded(lumen(
        &icd,
        &[
            &["preview", composition, "-o", sheet.to_str().unwrap()][..],
            &["--count", "5", "--columns", "4", "--width", "400"],
            &["--frames", "10..20"],
        ]
        .concat(),
    ));
    assert!(
        stdout.contains("frames=5 grid=4x2 size=400x160"),
        "{stdout}"
    );

    // (400 - 5 gutters) / 4 = 90 per cell, and 90 * 48 / 64 rounds to 68.
    let sheet = image::open(&sheet).unwrap().to_rgb8();
    assert_eq!(sheet.dimensions(), (400, 2 * 68 + 3 * GUTTER));
    let centres = cell_centres(&sheet, 4, 2, (90, 68));
    let near = |pixel: [u8; 3], expected: [u8; 3]| {
        pixel
            .iter()
            .zip(expected)
            .all(|(&got, want)| got.abs_diff(want) <= 3)
    };
    for empty in &centres[5..] {
        assert!(near(*empty, BACKGROUND), "{centres:?}");
    }

    // The first cell is frame 10, and the fifth the range's last, frame 19.
    let still = |frame: &str| {
        let path = directory.path().join(format!("{frame}.png"));
        let args = ["render", composition, "-o", path.to_str().unwrap()];
        succeeded(lumen(&icd, &[&args[..], &["--frame", frame]].concat()));
        image::open(&path).unwrap().to_rgb8().get_pixel(32, 24).0
    };
    assert!(near(centres[0], still("10")), "{centres:?}");
    assert!(near(centres[4], still("19")), "{centres:?}");
}

#[test]
fn bad_arguments_fail_before_rendering_and_write_nothing() {
    let directory = tempfile::tempdir().unwrap();
    let composition = write_composition(directory.path());
    let composition = composition.to_str().unwrap();
    let out = directory.path().join("out");
    let png = out.join("sheet.png");
    let png = png.to_str().unwrap();
    for (args, message) in [
        (&["-o", png, "--count", "0"][..], "--count"),
        (&["-o", png, "--columns", "0"], "--columns"),
        (
            &["-o", png, "--frames", "20..40"],
            "reaches past the timeline",
        ),
        (&["-o", png, "--frames", "9..9"], "is empty"),
        (
            &["-o", png, "--width", "40"],
            "leaves no room for 4 columns",
        ),
        (&["-o", "sheet.gif"], "use .png, .jpg or .jpeg"),
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_lumen"))
            .args([&["preview", composition][..], args].concat())
            .output()
            .expect("run lumen");
        let stderr = String::from_utf8_lossy(&output.stderr);
        assert!(!output.status.success(), "{args:?}");
        assert!(stderr.contains(message), "{args:?}: {stderr}");
    }
    assert!(!out.exists());
}
