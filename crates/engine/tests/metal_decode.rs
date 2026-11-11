//! VideoToolbox decode read back from the final renderer texture and compared with software decode
//! of the same clip. Skips when the `ffmpeg` CLI, one of its encoders, or VideoToolbox decode of a
//! clip is missing.
#![cfg(all(target_os = "macos", feature = "ffmpeg", feature = "metal"))]

use std::{
    collections::HashMap,
    path::Path,
    process::Command,
    sync::{Arc, Mutex},
};

use lumen_engine::{
    composition::Composition,
    error::MediaError,
    gpu::{GpuCompositionRenderer, RenderTarget},
    media::{
        AssetKind, FfmpegResolverOptions, FfmpegVideoResolver, MediaFrame, MediaIdentity,
        MediaResolver, MediaStore, ResolvedMedia, VideoFrameResolver,
    },
};
use serde_json::json;

/// Largest channel difference allowed, except for one sample in a thousand, which covers the edges
/// swscale's chroma rounds differently.
const MAX_CHANNEL_ERROR: u8 = 3;

/// Opens each source once, so a test can ask the resolver the renderer used where its frames came
/// from.
struct Store {
    options: FfmpegResolverOptions,
    resolvers: Mutex<HashMap<String, Arc<FfmpegVideoResolver>>>,
}

impl Store {
    fn new(options: FfmpegResolverOptions) -> Self {
        Self {
            options,
            resolvers: Mutex::default(),
        }
    }

    fn resolver(&self, stream_id: &str) -> Result<Arc<FfmpegVideoResolver>, MediaError> {
        let mut resolvers = self.resolvers.lock().expect("resolvers");
        if let Some(resolver) = resolvers.get(stream_id) {
            return Ok(Arc::clone(resolver));
        }
        let resolver = Arc::new(FfmpegVideoResolver::open_with_options(
            stream_id,
            self.options.clone(),
        )?);
        resolvers.insert(stream_id.to_string(), Arc::clone(&resolver));
        Ok(resolver)
    }
}

impl std::fmt::Debug for Store {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("Store")
    }
}

impl MediaStore for Store {
    fn resolve(&self, source: &str, _: AssetKind) -> Result<ResolvedMedia, MediaError> {
        Ok(ResolvedMedia::new(
            MediaIdentity::of_file(source),
            MediaResolver::Video(Box::new(self.resolver(source)?)),
        ))
    }
}

fn encode(path: &Path, input: &[&str], output: &[&str]) -> bool {
    let result = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(input)
        .args(output)
        .arg(path)
        .output();
    match result {
        Ok(result) if result.status.success() => true,
        Ok(result) => {
            eprintln!(
                "ffmpeg can't make {}; skipping: {}",
                path.display(),
                String::from_utf8_lossy(&result.stderr).trim()
            );
            false
        }
        Err(error) => {
            eprintln!("ffmpeg CLI unavailable ({error}); skipping");
            false
        }
    }
}

fn composition(source: &str, (width, height): (u32, u32)) -> Composition {
    lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": 30, "duration_frames": 30 },
        "render_settings": { "width": width, "height": height },
        "nodes": [
            { "id": 1, "type": "media_in", "params": { "kind": "video", "source": source } },
            { "id": 9999, "type": "media_output" },
        ],
        "connections": [{ "from_node": 1, "to_node": 9999, "to_port": "source" }],
    }))
    .expect("composition parses")
}

fn render(
    renderer: &mut GpuCompositionRenderer,
    scene: &Composition,
    frame: u32,
    store: &Store,
) -> Vec<u8> {
    let raster = renderer
        .render(scene, frame, store, RenderTarget::Owned)
        .unwrap_or_else(|error| panic!("frame {frame} does not render: {error}"))
        .raster;
    renderer
        .gpu_renderer()
        .read_texture_rgba8(&raster.texture, raster.domain.storage_size)
        .expect("readback")
}

/// Renders frames of `path` decoded by VideoToolbox and by software, and checks they match. Returns
/// whether VideoToolbox decoded the clip.
fn compare(label: &str, path: &Path) -> bool {
    let source = path.to_string_lossy();
    let hardware = Store::new(FfmpegResolverOptions::with_hardware_decode(true));
    let software = Store::new(FfmpegResolverOptions::default());
    let resolver = hardware
        .resolver(&source)
        .unwrap_or_else(|error| panic!("{label}: {error}"));
    if !matches!(resolver.frame(0), Ok(MediaFrame::GpuVideo(_))) {
        eprintln!("{label}: VideoToolbox doesn't decode this clip; skipping");
        return false;
    }
    let metadata = resolver.metadata();
    let scene = composition(&source, (metadata.width, metadata.height));
    let mut on_hardware = pollster::block_on(GpuCompositionRenderer::new()).expect("renderer");
    let mut on_software = pollster::block_on(GpuCompositionRenderer::new()).expect("renderer");
    let (mut largest, mut total, mut over, mut samples) = (0_u8, 0_u64, 0_u64, 0_u64);
    for frame in [0, 7, 20] {
        let a = render(&mut on_hardware, &scene, frame, &hardware);
        let b = render(&mut on_software, &scene, frame, &software);
        assert_eq!(a.len(), b.len(), "{label}: output sizes differ");
        for (a, b) in a.iter().zip(&b) {
            let error = a.abs_diff(*b);
            largest = largest.max(error);
            total += u64::from(error);
            over += u64::from(error > MAX_CHANNEL_ERROR);
            samples += 1;
        }
    }
    // A frame the renderer couldn't import moves the source to the CPU for good.
    assert!(
        matches!(resolver.frame(29), Ok(MediaFrame::GpuVideo(_))),
        "{label}: the renderer moved the source to the CPU"
    );
    println!(
        "{label}: max {largest}/255, mean {:.3}, over {MAX_CHANNEL_ERROR}/255 {:.4}%",
        total as f64 / samples as f64,
        over as f64 * 100.0 / samples as f64,
    );
    assert!(
        over * 1000 <= samples,
        "{label}: {over} of {samples} samples were more than {MAX_CHANNEL_ERROR}/255 off"
    );
    true
}

#[test]
fn videotoolbox_frames_match_software_decode() {
    let directory = tempfile::tempdir().expect("temp dir");
    let clip = |name: &str| directory.path().join(name);
    let testsrc = ["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=1"];
    let h264 = ["-c:v", "libx264", "-crf", "12", "-pix_fmt", "yuv420p"];
    let bt709 = [
        "-colorspace",
        "bt709",
        "-color_primaries",
        "bt709",
        "-color_trc",
        "bt709",
    ];

    let untagged = clip("untagged_sd.mp4");
    if !encode(&untagged, &testsrc, &h264) {
        return;
    }
    let mut compared = vec![compare("untagged SD H.264", &untagged)];

    let tagged = clip("bt709_full.mp4");
    if encode(
        &tagged,
        &testsrc,
        &[&h264[..], &bt709[..], &["-color_range", "pc"]].concat(),
    ) {
        compared.push(compare("BT.709 full-range H.264", &tagged));
    }

    for degrees in ["90", "270"] {
        let rotated = clip(&format!("rotated_{degrees}.mp4"));
        if encode(
            &rotated,
            &[
                "-display_rotation",
                degrees,
                "-i",
                &untagged.to_string_lossy(),
            ],
            &["-c", "copy"],
        ) {
            compared.push(compare(
                &format!("H.264 with display rotation {degrees}"),
                &rotated,
            ));
        }
    }

    // One flat colour, as swscale interpolates chroma across edges at depths above 8 bits.
    let ten_bit = clip("hevc_10_bit.mp4");
    if encode(
        &ten_bit,
        &["-f", "lavfi", "-i", "color=c=0xc04080:s=640x360:r=30:d=1"],
        &[
            &["-c:v", "libx265", "-crf", "12", "-pix_fmt", "yuv420p10le"][..],
            &["-tag:v", "hvc1", "-x265-params", "log-level=error"],
            &bt709,
        ]
        .concat(),
    ) {
        compared.push(compare("BT.709 10-bit HEVC", &ten_bit));
    }

    assert!(
        compared.iter().any(|&compared| compared),
        "VideoToolbox decoded none of the clips"
    );
}
