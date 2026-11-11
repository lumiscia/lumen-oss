//! Vulkan hardware decode on the renderer's device, read back from the final renderer texture and
//! compared with software decode. Skips on hardware without Vulkan Video decode, and any profile the
//! hardware or the `ffmpeg` CLI lacks.
//!
//! `LUMEN_VULKAN_DECODE_FIXTURES=<dir>` checks every patch clip in the directory instead of
//! generating a few. `LUMEN_VULKAN_DECODE_FILES=<a.mp4>:<b.mkv>` adds real footage to the playback
//! checks and prints how close each file came. `LUMEN_VULKAN_DECODE_REQUIRED=1` fails the IDR seek
//! test instead of skipping it, for runs on hardware that should be able to run it.
//! `LUMEN_VULKAN_DECODE_EXPECT=libx265_yuv420p10le,libsvtav1_yuv420p10le` fails the patch test
//! unless a clip whose file name contains each tag decodes on the GPU and matches software.
#![cfg(all(target_os = "linux", feature = "ffmpeg", feature = "vulkan"))]

use std::{
    collections::HashMap,
    fmt,
    io::Write,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::{Arc, Mutex},
};

use lumen_engine::{
    composition::Composition,
    error::MediaError,
    ffmpeg::{FfmpegResolverOptions, FfmpegVideoResolver},
    gpu::GpuCompositionRenderer,
    media::{
        AssetKind, MediaFrame, MediaIdentity, MediaResolver, MediaStore, ResolvedMedia,
        VideoFrameResolver, VideoMetadata,
    },
};
use lumen_gpu::wgpu;
use serde_json::{Value, json};

const MAX_CHANNEL_ERROR: u8 = 3;
const PATCH_INSET: usize = 4;
const PATCH_RGB: [[f64; 3]; 12] = [
    [1., 0., 0.],
    [0., 1., 0.],
    [0., 0., 1.],
    [1., 1., 1.],
    [0., 0., 0.],
    [1., 1., 0.],
    [0., 1., 1.],
    [1., 0., 1.],
    [0.5, 0.5, 0.5],
    [1., 0.5, 0.],
    [0.25, 0.5, 0.75],
    [0.8, 0.2, 0.4],
];
const PATCH_FRAMES: usize = 8;

struct Matrix {
    kr: f64,
    kb: f64,
    tags: [&'static str; 3],
}

const BT601: Matrix = Matrix {
    kr: 0.299,
    kb: 0.114,
    tags: ["smpte170m", "smpte170m", "smpte170m"],
};
const BT709: Matrix = Matrix {
    kr: 0.2126,
    kb: 0.0722,
    tags: ["bt709", "bt709", "bt709"],
};
const BT2020: Matrix = Matrix {
    kr: 0.2627,
    kb: 0.0593,
    tags: ["bt2020nc", "bt2020", "bt2020-10"],
};

/// A planar raw YUV format patch clips are encoded from, and keep.
struct Sampling {
    pix_fmt: &'static str,
    depth: u32,
    /// log2 of the horizontal and vertical chroma subsampling.
    chroma_shift: (usize, usize),
}

const YUV420: Sampling = Sampling {
    pix_fmt: "yuv420p",
    depth: 8,
    chroma_shift: (1, 1),
};
const YUV420P10: Sampling = Sampling {
    pix_fmt: "yuv420p10le",
    depth: 10,
    chroma_shift: (1, 1),
};
const YUV422P10: Sampling = Sampling {
    pix_fmt: "yuv422p10le",
    depth: 10,
    chroma_shift: (1, 0),
};
const YUV444: Sampling = Sampling {
    pix_fmt: "yuv444p",
    depth: 8,
    chroma_shift: (0, 0),
};
const YUV444P10: Sampling = Sampling {
    pix_fmt: "yuv444p10le",
    depth: 10,
    chroma_shift: (0, 0),
};

struct Store {
    options: FfmpegResolverOptions,
    videos: Mutex<HashMap<String, Arc<FfmpegVideoResolver>>>,
}

impl fmt::Debug for Store {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Store")
    }
}

impl Store {
    fn new(options: FfmpegResolverOptions) -> Self {
        Self {
            options,
            videos: Mutex::default(),
        }
    }

    fn resolver(&self, source: &str) -> Arc<FfmpegVideoResolver> {
        let mut videos = self.videos.lock().unwrap();
        Arc::clone(videos.entry(source.to_string()).or_insert_with(|| {
            let opened =
                FfmpegVideoResolver::open_with_options(source.to_string(), self.options.clone());
            Arc::new(opened.unwrap_or_else(|error| panic!("{source}: {error}")))
        }))
    }
}

struct SharedVideo(Arc<FfmpegVideoResolver>);

impl VideoFrameResolver for SharedVideo {
    fn metadata(&self) -> VideoMetadata {
        self.0.metadata()
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
        self.0.frame(frame)
    }
}

impl MediaStore for Store {
    fn resolve(&self, source: &str, _: AssetKind) -> Result<ResolvedMedia, MediaError> {
        Ok(ResolvedMedia::new(
            MediaIdentity::of_file(source),
            MediaResolver::Video(Box::new(SharedVideo(self.resolver(source)))),
        ))
    }
}

/// A renderer on a device with Vulkan Video decode, and a store whose sources decode on it.
fn hardware() -> Option<(GpuCompositionRenderer, Store)> {
    match pollster::block_on(GpuCompositionRenderer::new_vulkan_decode()) {
        Ok((renderer, device)) => {
            let mut options = FfmpegResolverOptions::with_hardware_decode(true);
            options.vulkan_device = Some(device);
            Some((renderer, Store::new(options)))
        }
        Err(error) => {
            eprintln!("Vulkan Video decode unavailable ({error}); skipping");
            None
        }
    }
}

fn software() -> (GpuCompositionRenderer, Store) {
    let renderer = pollster::block_on(GpuCompositionRenderer::new()).expect("create renderer");
    (renderer, Store::new(FfmpegResolverOptions::default()))
}

/// A hardware and a software renderer that a test's comparisons share, so it opens one Vulkan Video
/// device however many sources it compares: NVIDIA runs out of driver contexts after a few devices.
/// Each comparison takes fresh stores, so no decode state carries between them.
struct Renderers {
    hardware: GpuCompositionRenderer,
    device: lumen_ffmpeg::VulkanDevice,
    software: GpuCompositionRenderer,
}

impl Renderers {
    /// `None` on hardware without Vulkan Video decode.
    fn new() -> Option<Self> {
        let (hardware, device) =
            match pollster::block_on(GpuCompositionRenderer::new_vulkan_decode()) {
                Ok(pair) => pair,
                Err(error) => {
                    eprintln!("Vulkan Video decode unavailable ({error}); skipping");
                    return None;
                }
            };
        let software = pollster::block_on(GpuCompositionRenderer::new()).expect("create renderer");
        Some(Self {
            hardware,
            device,
            software,
        })
    }

    /// A store whose sources decode on the shared device.
    fn hardware_store(&self) -> Store {
        let mut options = FfmpegResolverOptions::with_hardware_decode(true);
        options.vulkan_device = Some(self.device.clone());
        Store::new(options)
    }
}

/// Tags from `LUMEN_VULKAN_DECODE_EXPECT`.
fn expected_tags() -> Vec<String> {
    std::env::var("LUMEN_VULKAN_DECODE_EXPECT")
        .unwrap_or_default()
        .split(',')
        .filter(|tag| !tag.is_empty())
        .map(str::to_string)
        .collect()
}

fn file_name(path: &Path) -> String {
    path.file_name()
        .unwrap_or_default()
        .to_string_lossy()
        .into_owned()
}

/// Whether the source's frames arrive as GPU frames. A decoder that opens may still move to the CPU
/// on its first frame. Fails instead of skipping for a source whose file name contains an
/// [`expected_tags`] tag.
fn decodes_on_hardware(store: &Store, path: &Path) -> bool {
    let source = path.to_string_lossy();
    let reason = match store.resolver(&source).frame(0) {
        Ok(MediaFrame::GpuVideo(_)) => return true,
        Ok(_) => "it moved to the CPU".to_string(),
        Err(error) => format!("decoding failed: {error}"),
    };
    let name = file_name(path);
    assert!(
        !expected_tags()
            .iter()
            .any(|tag| name.contains(tag.as_str())),
        "{source} should decode on this hardware, but {reason}"
    );
    eprintln!("{source} doesn't decode on this hardware ({reason}); skipping");
    false
}

/// Fails unless every [`expected_tags`] tag is in the file name of a clip in `checked`.
fn assert_expected_checked(checked: &[PathBuf]) {
    for tag in expected_tags() {
        assert!(
            checked.iter().any(|path| file_name(path).contains(&tag)),
            "LUMEN_VULKAN_DECODE_EXPECT names `{tag}`, but no clip with it was compared on the GPU"
        );
    }
}

/// The first test clip whose frames arrive as GPU frames.
fn hardware_clip(store: &Store, directory: &Path) -> Option<PathBuf> {
    playback_clips(directory)
        .into_iter()
        .find(|clip| decodes_on_hardware(store, clip))
}

/// The first test clip that also decodes on the CPU, which an FFmpeg without an AV1 software decoder
/// can't do for AV1.
fn clip_for_both(store: &Store, directory: &Path) -> Option<PathBuf> {
    let software = Store::new(FfmpegResolverOptions::default());
    playback_clips(directory).into_iter().find(|clip| {
        decodes_on_hardware(store, clip)
            && software.resolver(&clip.to_string_lossy()).frame(0).is_ok()
    })
}

fn composition(store: &Store, nodes: Value, sources: &[&str]) -> Composition {
    let metadata = store.resolver(sources[0]).metadata();
    let last = nodes.as_array().unwrap().last().unwrap()["id"].clone();
    let mut all = nodes.as_array().unwrap().clone();
    let mut connections: Vec<Value> = Vec::new();
    for node in &mut all {
        if let Some(inputs) = node.as_object_mut().unwrap().remove("in") {
            for (port, from) in inputs.as_object().unwrap() {
                let from = from
                    .as_array()
                    .cloned()
                    .unwrap_or_else(|| vec![from.clone()]);
                for from in from {
                    let to = node["id"].clone();
                    connections.push(json!({ "from_node": from, "to_node": to, "to_port": port }));
                }
            }
        }
    }
    all.push(json!({ "id": 9999, "type": "media_output" }));
    connections.push(json!({ "from_node": last, "to_node": 9999, "to_port": "source" }));
    lumen_engine::json::parse_value(&json!({
        "timeline": { "fps": metadata.fps, "duration_frames": metadata.frame_count },
        "render_settings": { "width": metadata.width, "height": metadata.height },
        "nodes": all,
        "connections": connections,
    }))
    .expect("composition parses")
}

fn media(id: u32, source: &str) -> Value {
    json!({ "id": id, "type": "media_in", "params": { "kind": "video", "source": source } })
}

fn render(
    renderer: &mut GpuCompositionRenderer,
    composition: &Composition,
    frame: u32,
    store: &Store,
) -> Vec<u8> {
    let raster = renderer
        .render_frame(composition, frame, store)
        .unwrap_or_else(|error| panic!("frame {frame} does not render: {error}"));
    let size = raster.domain.storage_size;
    let gpu = renderer.gpu_renderer();
    read_rgba8(gpu, &raster.texture, size)
}

fn read_rgba8(
    gpu: &lumen_gpu::Renderer,
    texture: &wgpu::Texture,
    size: lumen_gpu::Size,
) -> Vec<u8> {
    gpu.read_texture_rgba8(texture, size)
        .unwrap_or_else(|error| panic!("readback fails: {error}"))
}

fn patch_edges(size: usize, count: usize) -> Vec<usize> {
    (0..count)
        .map(|index| (size * index / count) / 2 * 2)
        .chain([size])
        .collect()
}

/// The largest channel difference inside the 4x3 patches. It covers each patch up to the frame's
/// edge, where padding beyond the visible size would show, and stops short of the edges between
/// patches, which 4:2:0 chroma and lossy coding blur.
fn patch_error(a: &[u8], b: &[u8], width: usize, height: usize) -> (u8, usize, usize) {
    let (xs, ys) = (patch_edges(width, 4), patch_edges(height, 3));
    let mut worst = (0, 0, 0);
    for row in 0..3 {
        for column in 0..4 {
            let top = if row == 0 { 0 } else { ys[row] + PATCH_INSET };
            let bottom = if row == 2 {
                height
            } else {
                ys[row + 1] - PATCH_INSET
            };
            let left = if column == 0 {
                0
            } else {
                xs[column] + PATCH_INSET
            };
            let right = if column == 3 {
                width
            } else {
                xs[column + 1] - PATCH_INSET
            };
            for y in top..bottom {
                for x in left..right {
                    let at = (y * width + x) * 4;
                    let error = (0..4)
                        .map(|channel| a[at + channel].abs_diff(b[at + channel]))
                        .max()
                        .unwrap();
                    if error > worst.0 {
                        worst = (error, x, y);
                    }
                }
            }
        }
    }
    worst
}

/// Compares `path` decoded on the GPU with software inside its patches, returning whether it did.
fn check_patch_clip(renderers: &mut Renderers, path: &Path) -> bool {
    let hardware_store = renderers.hardware_store();
    if !decodes_on_hardware(&hardware_store, path) {
        return false;
    }
    let software_store = Store::new(FfmpegResolverOptions::default());
    let source = path.to_string_lossy();
    let nodes = json!([media(1, &source)]);
    let hardware_scene = composition(&hardware_store, nodes.clone(), &[&source]);
    let software_scene = composition(&software_store, nodes, &[&source]);
    let metadata = software_store.resolver(&source).metadata();
    let (width, height) = (metadata.width as usize, metadata.height as usize);
    for frame in 0..PATCH_FRAMES as u32 {
        let on_hardware = render(
            &mut renderers.hardware,
            &hardware_scene,
            frame,
            &hardware_store,
        );
        let on_software = render(
            &mut renderers.software,
            &software_scene,
            frame,
            &software_store,
        );
        let (error, x, y) = patch_error(&on_hardware, &on_software, width, height);
        assert!(
            error <= MAX_CHANNEL_ERROR,
            "{source} frame {frame}: {error}/255 off at ({x}, {y})"
        );
    }
    assert!(decodes_on_hardware(&hardware_store, path), "{source}");
    println!("{source}: patches within {MAX_CHANNEL_ERROR}/255");
    true
}

/// Flat patches in `sampling`'s raw format, whose YCbCr is computed from `PATCH_RGB` with `matrix`
/// and `full_range`.
fn patch_frames(
    width: usize,
    height: usize,
    matrix: &Matrix,
    full_range: bool,
    sampling: &Sampling,
) -> Vec<u8> {
    let (xs, ys) = (patch_edges(width, 4), patch_edges(height, 3));
    let (shift_x, shift_y) = sampling.chroma_shift;
    let (chroma_width, chroma_height) =
        (width.div_ceil(1 << shift_x), height.div_ceil(1 << shift_y));
    let max = f64::from((1 << sampling.depth) - 1);
    let scale = f64::from(1 << (sampling.depth - 8));
    let (luma, chroma, offset, mid) = if full_range {
        (max, max, 0., f64::from(1 << (sampling.depth - 1)))
    } else {
        (219. * scale, 224. * scale, 16. * scale, 128. * scale)
    };
    let mut frames = Vec::new();
    for frame in 0..PATCH_FRAMES {
        let mut planes = [
            vec![0_u16; width * height],
            vec![0_u16; chroma_width * chroma_height],
            vec![0_u16; chroma_width * chroma_height],
        ];
        for row in 0..3 {
            for column in 0..4 {
                let [r, g, b] = PATCH_RGB[(row * 4 + column + frame) % PATCH_RGB.len()];
                let y = matrix.kr * r + (1. - matrix.kr - matrix.kb) * g + matrix.kb * b;
                let cb = (b - y) / (2. * (1. - matrix.kb));
                let cr = (r - y) / (2. * (1. - matrix.kr));
                let samples = [offset + luma * y, mid + chroma * cb, mid + chroma * cr]
                    .map(|sample: f64| sample.round().clamp(0., max) as u16);
                for line in ys[row]..ys[row + 1] {
                    planes[0][line * width..][xs[column]..xs[column + 1]].fill(samples[0]);
                }
                for line in ys[row] >> shift_y..ys[row + 1] >> shift_y {
                    for plane in 1..3 {
                        planes[plane][line * chroma_width..]
                            [xs[column] >> shift_x..xs[column + 1] >> shift_x]
                            .fill(samples[plane]);
                    }
                }
            }
        }
        for sample in planes.iter().flatten() {
            if sampling.depth > 8 {
                frames.extend_from_slice(&sample.to_le_bytes());
            } else {
                frames.push(*sample as u8);
            }
        }
    }
    frames
}

/// Runs the `ffmpeg` CLI, and returns `false` when it or the encoder is missing.
fn run_ffmpeg(args: &[&str], stdin: Option<&[u8]>) -> bool {
    let mut child = match Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error"])
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(error) => {
            eprintln!("ffmpeg CLI unavailable ({error}); skipping");
            return false;
        }
    };
    if let Some(stdin) = stdin {
        // A failure to start encoding shows up as the exit status.
        let _ = child.stdin.take().unwrap().write_all(stdin);
    }
    let output = child.wait_with_output().expect("wait for ffmpeg");
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() && stderr.contains("Unknown encoder") {
        eprintln!("{}; skipping", stderr.trim());
        return false;
    }
    assert!(output.status.success(), "ffmpeg failed: {stderr}");
    true
}

const ENCODERS: [&[&str]; 3] = [
    &["-c:v", "libx264", "-preset", "veryfast", "-crf", "14"],
    &[
        "-c:v", "libx265", "-preset", "veryfast", "-crf", "14", "-tag:v", "hvc1",
    ],
    &["-c:v", "libsvtav1", "-preset", "10", "-crf", "20"],
];

fn generated_patch_clip(
    directory: &Path,
    encoder: &[&str],
    sampling: &Sampling,
    (width, height): (usize, usize),
    matrix: &Matrix,
    full_range: bool,
) -> Option<PathBuf> {
    let path = directory.join(format!(
        "{}_{}_{width}x{height}_{}_{}.mp4",
        encoder[1], sampling.pix_fmt, matrix.tags[0], full_range
    ));
    let size = format!("{width}x{height}");
    let mut args = vec![
        "-f",
        "rawvideo",
        "-pix_fmt",
        sampling.pix_fmt,
        "-s",
        &size,
        "-r",
        "24",
        "-i",
        "-",
    ];
    args.extend_from_slice(encoder);
    args.extend([
        "-pix_fmt",
        sampling.pix_fmt,
        "-colorspace",
        matrix.tags[0],
        "-color_primaries",
        matrix.tags[1],
        "-color_trc",
        matrix.tags[2],
        "-color_range",
        if full_range { "pc" } else { "tv" },
    ]);
    let output = path.to_string_lossy();
    args.push(&output);
    run_ffmpeg(
        &args,
        Some(&patch_frames(width, height, matrix, full_range, sampling)),
    )
    .then_some(path.clone())
}

#[test]
fn decoded_colours_match_software_inside_patches() {
    if let Some(directory) = std::env::var_os("LUMEN_VULKAN_DECODE_FIXTURES") {
        let mut clips: Vec<_> = std::fs::read_dir(directory)
            .expect("read fixtures")
            .map(|entry| entry.unwrap().path())
            .collect();
        clips.sort();
        let mut renderers = Renderers::new();
        clips.retain(|clip| {
            renderers
                .as_mut()
                .is_some_and(|renderers| check_patch_clip(renderers, clip))
        });
        assert_expected_checked(&clips);
        return;
    }
    let mut renderers = Renderers::new();
    let mut checked = Vec::new();
    let directory = tempfile::tempdir().expect("temp dir");
    let [h264, hevc, av1] = ENCODERS;
    // HEVC Main 10 is what iPhones record.
    for (encoder, sampling) in [
        (h264, &YUV420),
        (hevc, &YUV420),
        (av1, &YUV420),
        (hevc, &YUV420P10),
        (hevc, &YUV422P10),
        (hevc, &YUV444),
        (hevc, &YUV444P10),
        (av1, &YUV420P10),
    ] {
        for (matrix, full_range) in [
            (&BT601, false),
            (&BT709, false),
            (&BT709, true),
            (&BT2020, false),
        ] {
            let Some(clip) = generated_patch_clip(
                directory.path(),
                encoder,
                sampling,
                (642, 362),
                matrix,
                full_range,
            ) else {
                break;
            };
            if renderers
                .as_mut()
                .is_some_and(|renderers| check_patch_clip(renderers, &clip))
            {
                checked.push(clip);
            }
        }
    }
    assert_expected_checked(&checked);
}

fn motion_clip(directory: &Path, encoder: &[&str]) -> Option<PathBuf> {
    let path = directory.join(format!("{}_motion.mp4", encoder[1]));
    let mut args = vec![
        "-f",
        "lavfi",
        "-i",
        "testsrc2=s=642x362:r=30:d=4",
        "-g",
        "12",
    ];
    args.extend_from_slice(encoder);
    args.extend(["-pix_fmt", "yuv420p"]);
    let output = path.to_string_lossy();
    args.push(&output);
    run_ffmpeg(&args, None).then_some(path.clone())
}

fn playback_clips(directory: &Path) -> Vec<PathBuf> {
    let mut clips: Vec<_> = ENCODERS
        .iter()
        .filter_map(|encoder| motion_clip(directory, encoder))
        .collect();
    if let Some(files) = std::env::var_os("LUMEN_VULKAN_DECODE_FILES") {
        clips.extend(std::env::split_paths(&files));
    }
    clips
}

/// Compares frames decoded on hardware and software in one scene, allowing one sample in a thousand
/// more than `MAX_CHANNEL_ERROR` off. Returns whether the hardware could decode the sources, as
/// nothing is compared when it can't.
fn compare_playback(
    renderers: &mut Renderers,
    label: &str,
    nodes: Value,
    sources: &[&str],
    frames: impl IntoIterator<Item = u32>,
) -> bool {
    let hardware_store = renderers.hardware_store();
    if !sources
        .iter()
        .all(|source| decodes_on_hardware(&hardware_store, Path::new(source)))
    {
        return false;
    }
    let software_store = Store::new(FfmpegResolverOptions::default());
    let hardware_scene = composition(&hardware_store, nodes.clone(), sources);
    let software_scene = composition(&software_store, nodes, sources);
    let (mut largest, mut total, mut over, mut samples) = (0_u8, 0_u64, 0_u64, 0_u64);
    for frame in frames {
        let on_hardware = render(
            &mut renderers.hardware,
            &hardware_scene,
            frame,
            &hardware_store,
        );
        // A new decoder per frame, as software decode on one CPU can share a seek bug with hardware.
        let on_software = render(
            &mut renderers.software,
            &software_scene,
            frame,
            &Store::new(FfmpegResolverOptions::default()),
        );
        for (a, b) in on_hardware.iter().zip(&on_software) {
            let error = a.abs_diff(*b);
            largest = largest.max(error);
            total += u64::from(error);
            over += u64::from(error > MAX_CHANNEL_ERROR);
            samples += 1;
        }
    }
    println!(
        "{label}: max {largest}/255, mean {:.3}, over {MAX_CHANNEL_ERROR}/255 {:.4}%",
        total as f64 / samples as f64,
        over as f64 * 100.0 / samples as f64,
    );
    assert!(
        over * 1000 <= samples,
        "{label}: {over} of {samples} samples were more than {MAX_CHANNEL_ERROR}/255 off"
    );
    for source in sources {
        assert!(
            decodes_on_hardware(&hardware_store, Path::new(source)),
            "{source}"
        );
    }
    true
}

#[test]
fn seeking_time_remap_and_two_sources_match_software() {
    let Some(mut renderers) = Renderers::new() else {
        return;
    };
    let directory = tempfile::tempdir().expect("temp dir");
    let clips = playback_clips(directory.path());
    let sources: Vec<_> = clips.iter().map(|clip| clip.to_string_lossy()).collect();
    for source in &sources {
        compare_playback(
            &mut renderers,
            &format!("{source} forward then seeks"),
            json!([media(1, source)]),
            &[source],
            (0..30).chain([100, 101, 40, 41, 42, 7, 8, 80, 3]),
        );
        compare_playback(
            &mut renderers,
            &format!("{source} time remap"),
            json!([
                media(1, source),
                { "id": 2, "type": "time_remap", "in": { "source": 1 }, "params": {
                    "frame": "=if(frame % 8 < 4, 60 - frame, frame * 2)"
                } },
            ]),
            &[source],
            0..30,
        );
    }
    for pair in sources.windows(2) {
        let (first, second) = (&pair[0], &pair[1]);
        compare_playback(
            &mut renderers,
            &format!("{first} and {second} at once"),
            json!([
                media(1, first),
                media(2, second),
                { "id": 3, "type": "opacity", "in": { "source": 2 }, "params": { "opacity": 0.5 } },
                { "id": 4, "type": "raster_multimerge", "in": { "layers": [1, 3] } },
            ]),
            &[first, second],
            0..30,
        );
    }
}

#[test]
fn rotated_sources_decode_upright_and_match_software() {
    let directory = tempfile::tempdir().expect("temp dir");
    let coded = directory.path().join("coded.mp4");
    let coded_path = coded.to_string_lossy();
    if !run_ffmpeg(
        &[
            &["-f", "lavfi", "-i", "testsrc2=s=640x360:r=30:d=1"][..],
            ENCODERS[0],
            &["-pix_fmt", "yuv420p", &coded_path],
        ]
        .concat(),
        None,
    ) {
        return;
    }
    let Some(mut renderers) = Renderers::new() else {
        return;
    };
    if !compare_playback(
        &mut renderers,
        "the unrotated clip",
        json!([media(1, &coded_path)]),
        &[&coded_path],
        0..1,
    ) {
        return;
    }
    for degrees in ["90", "270"] {
        let rotated = directory.path().join(format!("rotated_{degrees}.mp4"));
        let source = rotated.to_string_lossy();
        run_ffmpeg(
            &[
                "-display_rotation",
                degrees,
                "-i",
                &coded_path,
                "-c",
                "copy",
                &source,
            ],
            None,
        );
        assert!(
            compare_playback(
                &mut renderers,
                &format!("{source} with display rotation {degrees}"),
                json!([media(1, &source)]),
                &[&source],
                0..10,
            ),
            "the rotated clip didn't decode on hardware, though its unrotated stream did"
        );
    }
}

/// Reports a test that checked nothing, which `cargo test` still prints as `ok`.
fn skip_unless_required(reason: &str) {
    assert!(
        std::env::var_os("LUMEN_VULKAN_DECODE_REQUIRED").is_none(),
        "required, but could not run: {reason}"
    );
    eprintln!("SKIPPED, nothing was checked: {reason}");
}

/// HEVC with B-frames and an IDR frame every 12 frames, from `hevc_nvenc`. Without NVENC the test
/// that uses it skips.
fn hevc_b_frame_clip(directory: &Path) -> Result<PathBuf, String> {
    let path = directory.join("hevc_b_frames.mp4");
    let output = Command::new("ffmpeg")
        .args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi"])
        .args(["-i", "testsrc2=s=642x362:r=30:d=4", "-c:v", "hevc_nvenc"])
        .args([
            "-g", "12", "-bf", "3", "-tag:v", "hvc1", "-pix_fmt", "yuv420p",
        ])
        .arg(&path)
        .output()
        .map_err(|error| format!("ffmpeg CLI unavailable ({error})"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(format!("hevc_nvenc failed: {}", stderr.trim()));
    }
    let probe = Command::new("ffprobe")
        .args(["-v", "error", "-select_streams", "v"])
        .args(["-show_entries", "stream=has_b_frames", "-of", "csv=p=0"])
        .arg(&path)
        .output()
        .map_err(|error| format!("ffprobe CLI unavailable ({error})"))?;
    let reorder = String::from_utf8_lossy(&probe.stdout);
    if reorder.trim().parse::<u32>().unwrap_or(0) == 0 {
        return Err(format!("hevc_nvenc made no B-frames ({})", reorder.trim()));
    }
    Ok(path)
}

/// An IDR frame releases every frame before it from the decoder at once. Stopping among those and
/// seeking back must not find the rest still queued.
#[test]
fn seeking_back_from_frames_an_idr_frame_released_matches_software() {
    let directory = tempfile::tempdir().expect("temp dir");
    let clip = match hevc_b_frame_clip(directory.path()) {
        Ok(clip) => clip,
        Err(reason) => return skip_unless_required(&reason),
    };
    let source = clip.to_string_lossy();
    let Some(mut renderers) = Renderers::new() else {
        return skip_unless_required("no Vulkan Video decode device");
    };
    let compared = compare_playback(
        &mut renderers,
        &format!("{source} seeks back before IDR frames"),
        json!([media(1, &source)]),
        &[&source],
        [12, 24, 36, 48, 60]
            .into_iter()
            .flat_map(|idr| [idr - 2, idr - 3]),
    );
    if !compared {
        skip_unless_required("Vulkan Video can't decode HEVC here");
    }
}

#[test]
fn a_frame_the_renderer_cannot_import_moves_its_source_to_the_cpu() {
    let directory = tempfile::tempdir().expect("temp dir");
    let Some((_decoding_renderer, store)) = hardware() else {
        return;
    };
    let Some(clip) = clip_for_both(&store, directory.path()) else {
        return;
    };
    let source = clip.to_string_lossy();
    // Frames decoded on the first device can't be imported by a renderer on another.
    let (mut other_renderer, _) = hardware().expect("second device");
    let (mut software_renderer, software_store) = software();
    let nodes = json!([media(1, &source)]);
    let scene = composition(&store, nodes.clone(), &[&source]);
    let software_scene = composition(&software_store, nodes, &[&source]);
    for frame in [0, 1, 2, 50, 3] {
        let fallback = render(&mut other_renderer, &scene, frame, &store);
        let reference = render(
            &mut software_renderer,
            &software_scene,
            frame,
            &software_store,
        );
        assert_eq!(fallback, reference, "frame {frame}");
    }
    let resolver = store.resolver(&source);
    assert!(matches!(
        resolver.frame(5),
        Ok(MediaFrame::CpuRgba(_) | MediaFrame::Yuv(_))
    ));
}

/// H.264 High 10, a profile Vulkan Video has no H.264 profile for, so FFmpeg's setup of the first
/// frame's decode fails on every device.
#[test]
fn a_profile_the_device_cannot_decode_moves_its_source_to_the_cpu() {
    let directory = tempfile::tempdir().expect("temp dir");
    let clip = directory.path().join("h264_high10.mp4");
    let source = clip.to_string_lossy();
    if !run_ffmpeg(
        &[
            &["-f", "lavfi", "-i", "testsrc2=s=642x362:r=30:d=2"][..],
            ENCODERS[0],
            &["-pix_fmt", "yuv420p10le", &source],
        ]
        .concat(),
        None,
    ) {
        return;
    }
    let Some((mut hardware_renderer, store)) = hardware() else {
        return;
    };
    let resolver = store.resolver(&source);
    match resolver.frame(0) {
        Ok(MediaFrame::GpuVideo(_)) => {
            eprintln!("this device decodes H.264 High 10; skipping");
            return;
        }
        Ok(_) => {}
        Err(error) => panic!("{source} should fall back to the CPU: {error}"),
    }
    let (mut software_renderer, software_store) = software();
    let nodes = json!([media(1, &source)]);
    let scene = composition(&store, nodes.clone(), &[&source]);
    let software_scene = composition(&software_store, nodes, &[&source]);
    for frame in [0, 1, 2, 40, 3] {
        let fallback = render(&mut hardware_renderer, &scene, frame, &store);
        let reference = render(
            &mut software_renderer,
            &software_scene,
            frame,
            &software_store,
        );
        assert_eq!(fallback, reference, "frame {frame}");
    }
    assert!(matches!(
        resolver.frame(5),
        Ok(MediaFrame::CpuRgba(_) | MediaFrame::Yuv(_))
    ));
    println!("{source}: fell back to the CPU and matches software");
}

/// Binding resolves both frames before either is imported, so the first failed import finds another
/// frame resolved and none drawn.
#[test]
fn a_failed_first_import_moves_a_source_with_two_bound_frames_to_the_cpu() {
    let directory = tempfile::tempdir().expect("temp dir");
    let Some((_decoding_renderer, store)) = hardware() else {
        return;
    };
    let Some(clip) = clip_for_both(&store, directory.path()) else {
        return;
    };
    let source = clip.to_string_lossy();
    // Frames decoded on the first device can't be imported by a renderer on another.
    let (mut other_renderer, _) = hardware().expect("second device");
    let (mut software_renderer, software_store) = software();
    let nodes = json!([
        { "id": 1, "type": "media_in", "params": { "kind": "video", "source": source, "range_start": 0 } },
        { "id": 2, "type": "media_in", "params": { "kind": "video", "source": source, "range_start": 1 } },
        { "id": 3, "type": "opacity", "in": { "source": 2 }, "params": { "opacity": 0.5 } },
        { "id": 4, "type": "raster_multimerge", "in": { "layers": [1, 3] } },
    ]);
    let scene = composition(&store, nodes.clone(), &[&source]);
    let software_scene = composition(&software_store, nodes, &[&source]);
    for frame in [0, 1, 40] {
        let fallback = render(&mut other_renderer, &scene, frame, &store);
        let reference = render(
            &mut software_renderer,
            &software_scene,
            frame,
            &software_store,
        );
        assert_eq!(fallback, reference, "frame {frame}");
    }
}

#[test]
fn a_source_that_drew_from_the_gpu_fails_rather_than_moving_to_the_cpu() {
    let directory = tempfile::tempdir().expect("temp dir");
    let Some((mut renderer, store)) = hardware() else {
        return;
    };
    let Some(clip) = hardware_clip(&store, directory.path()) else {
        return;
    };
    let source = clip.to_string_lossy();
    let resolver = store.resolver(&source);
    let Ok(MediaFrame::GpuVideo(undrawn)) = resolver.frame(5) else {
        panic!("frame 5 should decode on the GPU like frame 0");
    };
    let scene = composition(&store, json!([media(1, &source)]), &[&source]);
    render(&mut renderer, &scene, 1, &store);

    let error = undrawn.decode_on_cpu("test").unwrap_err().to_string();

    assert!(error.contains("would change"), "{error}");
    assert!(matches!(resolver.frame(2), Ok(MediaFrame::GpuVideo(_))));
}
