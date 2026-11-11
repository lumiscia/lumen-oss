//! Measures rendering a composition: time per frame, allocations and held memory, one run per
//! process.

use std::{
    alloc::{GlobalAlloc, Layout, System},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Instant,
};

use anyhow::{Context, anyhow, bail};
use lumen_engine::{
    composition::Composition,
    error::MediaError,
    gpu::GpuCompositionRenderer,
    media::{
        CpuMediaFrame, FontResolver, ImageResolver, MediaFrame, MediaStore, VideoFrameResolver,
        VideoMetadata,
    },
};
use serde_json::{Value, json};

use super::compositions;
use crate::bench::media::BenchmarkMediaStore;

static ALLOCATIONS: AtomicU64 = AtomicU64::new(0);
static ALLOCATED_BYTES: AtomicU64 = AtomicU64::new(0);

pub struct CountingAlloc;

// SAFETY: every call forwards to `System` with its arguments unchanged.
unsafe impl GlobalAlloc for CountingAlloc {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(layout.size() as u64, Ordering::Relaxed);
        unsafe { System.alloc(layout) }
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) }
    }

    unsafe fn realloc(&self, pointer: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        ALLOCATIONS.fetch_add(1, Ordering::Relaxed);
        ALLOCATED_BYTES.fetch_add(new_size as u64, Ordering::Relaxed);
        unsafe { System.realloc(pointer, layout, new_size) }
    }
}

struct Args {
    workload: String,
    scrub: bool,
    frames: Option<u32>,
    passes: u32,
    launch_dir: Option<PathBuf>,
    full_render: bool,
    out: Option<PathBuf>,
}

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        workload: String::new(),
        scrub: false,
        frames: None,
        passes: 3,
        launch_dir: None,
        full_render: false,
        out: None,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        let mut value = || raw.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--workload" => args.workload = value()?,
            "--seq" => {
                args.scrub = match value()?.as_str() {
                    "scrub" => true,
                    "loop" => false,
                    other => bail!("unknown sequence {other}"),
                }
            }
            "--frames" => args.frames = Some(value()?.parse()?),
            "--passes" => args.passes = value()?.parse()?,
            "--launch-dir" => args.launch_dir = Some(value()?.into()),
            "--full-render" => args.full_render = true,
            "--out" => args.out = Some(value()?.into()),
            other => bail!("unknown argument {other}"),
        }
    }
    Ok(args)
}

pub fn run() -> anyhow::Result<()> {
    let args = parse_args()?;
    if let Ok(dir) = std::env::var("LUMEN_BENCH_DUMP_JSON") {
        for (name, value) in [
            ("dynamic_text", dynamic_text()),
            ("dynamic_path", dynamic_path()),
            ("motion_blur", motion_blur()),
            ("shader_switch", shader_switch()),
        ] {
            std::fs::write(format!("{dir}/{name}.json"), value.to_string())?;
        }
        return Ok(());
    }
    let (composition, media) = load(&args)?;
    let result = pollster::block_on(measure(&args, &composition, &media))?;
    let line = result.to_string();
    println!("{line}");
    if let Some(out) = &args.out {
        use std::io::Write;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(out)?;
        writeln!(file, "{line}")?;
    }
    Ok(())
}

async fn new_renderer(
    full_render: bool,
    composition: &Composition,
) -> anyhow::Result<GpuCompositionRenderer> {
    let mut renderer = GpuCompositionRenderer::new().await?;
    renderer.set_static_output_reuse(!full_render);
    if let Some(mib) = std::env::var("LUMEN_BENCH_POOL_MIB")
        .ok()
        .and_then(|mib| mib.parse::<u64>().ok())
    {
        renderer.set_frame_graph_pool_budget(mib << 20);
    }
    renderer.compile(composition)?;
    Ok(renderer)
}

#[derive(Default)]
struct Series(Vec<f64>);

impl Series {
    fn summary(&self) -> Value {
        let mut sorted = self.0.clone();
        sorted.sort_unstable_by(f64::total_cmp);
        if sorted.is_empty() {
            return Value::Null;
        }
        let at = |quantile: f64| sorted[((sorted.len() - 1) as f64 * quantile).round() as usize];
        json!({ "median": at(0.5), "p95": at(0.95), "min": sorted[0], "max": at(1.0) })
    }
}

#[derive(Default)]
struct Timings {
    bind: Series,
    upload: Series,
    submit: Series,
    gpu: Series,
    cpu: Series,
    total: Series,
    allocations: Series,
    allocated_bytes: Series,
}

impl Timings {
    fn summary(&self) -> Value {
        json!({
            "frames": self.total.0.len(),
            "bind_ms": self.bind.summary(),
            "upload_ms": self.upload.summary(),
            "submit_ms": self.submit.summary(),
            "gpu_wait_ms": self.gpu.summary(),
            "cpu_ms": self.cpu.summary(),
            "total_ms": self.total.summary(),
            "allocations": self.allocations.summary(),
            "allocated_bytes": self.allocated_bytes.summary(),
        })
    }
}

async fn measure(args: &Args, composition: &Composition, media: &Fixture) -> anyhow::Result<Value> {
    let setup_started = Instant::now();
    let mut renderer = new_renderer(args.full_render, composition).await?;
    let setup_ms = setup_started.elapsed().as_secs_f64() * 1e3;
    let duration = composition.timeline().duration_frames;
    let frames = args.frames.unwrap_or(duration).min(duration);
    let sequence = frame_sequence(frames, duration, args.scrub);

    let mut cold = Timings::default();
    let mut warm = Timings::default();
    let mut peak_held = 0;
    let mut peak_rss = 0;
    let mut peak_vram = 0;
    let mut steady_held = Series::default();
    let mut steady_rss = Series::default();
    let mut steady_vram = Series::default();
    let mut warm_start = None;
    let mut rendered = 0u64;
    for pass in 0..args.passes {
        if pass == 1 {
            warm_start = Some(bind_stats(&renderer));
        }
        let timings = if pass == 0 { &mut cold } else { &mut warm };
        for &frame in &sequence {
            let allocations = ALLOCATIONS.load(Ordering::Relaxed);
            let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed);
            let started = Instant::now();
            let bound = renderer.bind_frame(composition, frame, media)?;
            let bound_at = Instant::now();
            renderer.upload_bound_frame(&bound)?;
            let uploaded_at = Instant::now();
            let (_raster, submission) = renderer.submit_render()?;
            let submitted_at = Instant::now();
            drop(bound);
            let allocations = ALLOCATIONS.load(Ordering::Relaxed) - allocations;
            let bytes = ALLOCATED_BYTES.load(Ordering::Relaxed) - bytes;
            renderer
                .gpu_renderer()
                .device
                .poll(lumen_gpu::wgpu::PollType::Wait {
                    submission_index: Some(submission),
                    timeout: None,
                })
                .map_err(|error| anyhow!("poll: {error}"))?;
            let done = Instant::now();
            let ms = |from: Instant, to: Instant| (to - from).as_secs_f64() * 1e3;
            timings.bind.0.push(ms(started, bound_at));
            timings.upload.0.push(ms(bound_at, uploaded_at));
            timings.submit.0.push(ms(uploaded_at, submitted_at));
            timings.gpu.0.push(ms(submitted_at, done));
            timings.cpu.0.push(ms(started, submitted_at));
            timings.total.0.push(ms(started, done));
            timings.allocations.0.push(allocations as f64);
            timings.allocated_bytes.0.push(bytes as f64);
            rendered += 1;
            if rendered.is_multiple_of(120) {
                let held = renderer.held_bytes();
                let rss = rss_bytes();
                let vram = vram_bytes();
                peak_held = peak_held.max(held);
                peak_rss = peak_rss.max(rss);
                peak_vram = peak_vram.max(vram);
                if pass > 0 {
                    steady_held.0.push(held as f64);
                    steady_rss.0.push(rss as f64);
                    steady_vram.0.push(vram as f64);
                }
            }
        }
    }
    let end = bind_stats(&renderer);
    let start = warm_start.unwrap_or_default();
    let last = |series: &Series| series.0.last().copied().unwrap_or(0.0);
    let stats = renderer.frame_graph_stats();
    Ok(json!({
        "workload": args.workload,
        "seq": if args.scrub { "scrub" } else { "loop" },
        "full_render": args.full_render,
        "adapter": adapter_name(&renderer),
        "frames": frames,
        "passes": args.passes,
        "setup_ms": setup_ms,
        "cold": cold.summary(),
        "warm": warm.summary(),
        "bind_group_misses": end.misses,
        "warm_bind_group_misses": end.misses - start.misses,
        "warm_bind_group_hits": end.hits - start.hits,
        "passes_recorded": end.passes,
        "frame_graph": {
            "peak_frame_bytes": stats.recorder.peak_frame_bytes,
            "peak_unshared_bytes": stats.recorder.peak_unshared_bytes,
            "textures_created": stats.recorder.textures_created,
            "textures_destroyed": stats.recorder.textures_destroyed,
            "held_bytes": stats.held_bytes,
            "retained_bytes": stats.retained_bytes,
            "adopted_bytes": stats.adopted_bytes,
            "bind_groups": stats.bind_groups,
        },
        "programs": renderer.created_program_count(),
        "peak_held_bytes": peak_held,
        "end_held_bytes": last(&steady_held),
        "steady_held": steady_held.summary(),
        "peak_rss_bytes": peak_rss,
        "end_rss_bytes": last(&steady_rss),
        "first_steady_rss_bytes": steady_rss.0.first().copied().unwrap_or(0.0),
        "peak_vram_bytes": peak_vram,
        "end_vram_bytes": last(&steady_vram),
        "first_steady_vram_bytes": steady_vram.0.first().copied().unwrap_or(0.0),
    }))
}

#[derive(Default, Clone, Copy)]
struct BindStats {
    hits: u64,
    misses: u64,
    passes: u64,
}

fn bind_stats(renderer: &GpuCompositionRenderer) -> BindStats {
    let stats = renderer.frame_graph_stats().recorder;
    BindStats {
        hits: stats.bind_group_hits,
        misses: stats.bind_group_misses,
        passes: stats.passes,
    }
}

fn adapter_name(renderer: &GpuCompositionRenderer) -> String {
    renderer
        .gpu_renderer()
        .adapter_info()
        .map_or_else(|| "unknown".to_string(), |info| info.name.clone())
}

/// The frames of one pass, in order or in a fixed shuffle of the timeline.
fn frame_sequence(frames: u32, duration: u32, scrub: bool) -> Vec<u32> {
    if !scrub {
        return (0..frames).collect();
    }
    let mut state = 0x2545_f491_4f6c_dd1d_u64;
    (0..frames)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            (state % u64::from(duration)) as u32
        })
        .collect()
}

fn rss_bytes() -> u64 {
    std::fs::read_to_string("/proc/self/statm")
        .ok()
        .and_then(|statm| statm.split_whitespace().nth(1)?.parse::<u64>().ok())
        .map_or(0, |pages| pages * 4096)
}

/// Device memory in use by the whole machine: noisy where other processes share the GPU.
fn vram_bytes() -> u64 {
    let amd: u64 = std::fs::read_dir("/sys/class/drm")
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with("card") && !name.contains('-')
        })
        .flat_map(|entry| {
            ["mem_info_vram_used", "mem_info_gtt_used"]
                .map(|file| entry.path().join("device").join(file))
        })
        .filter_map(|path| {
            std::fs::read_to_string(path)
                .ok()?
                .trim()
                .parse::<u64>()
                .ok()
        })
        .sum();
    if amd > 0 {
        return amd;
    }
    std::process::Command::new("nvidia-smi")
        .args(["--query-gpu=memory.used", "--format=csv,noheader,nounits"])
        .output()
        .ok()
        .and_then(|output| {
            String::from_utf8_lossy(&output.stdout)
                .trim()
                .parse::<u64>()
                .ok()
        })
        .map_or(0, |mib| mib << 20)
}

fn load(args: &Args) -> anyhow::Result<(Composition, Fixture)> {
    let launch_dir = || {
        args.launch_dir
            .as_deref()
            .ok_or_else(|| anyhow!("--launch-dir is required for this workload"))
    };
    let fonts = || FontDir::open(&launch_dir()?.join("fonts"));
    let parse = |value: Value| -> anyhow::Result<Composition> {
        lumen_engine::json::parse_value(&value).context("fixture does not parse")
    };
    Ok(match args.workload.as_str() {
        "launch" => {
            let source = std::fs::read_to_string(launch_dir()?.join("launch-video.json"))?;
            let composition = lumen_engine::json::parse(&source).context("launch json")?;
            (composition, Fixture::fonts(fonts()?))
        }
        "dynamic_text" => (parse(dynamic_text())?, Fixture::fonts(fonts()?)),
        "dynamic_path" => (parse(dynamic_path())?, Fixture::fonts(fonts()?)),
        "motion_blur" => (parse(motion_blur())?, Fixture::fonts(fonts()?)),
        "remapped_media" => (
            parse(remapped_media())?,
            Fixture::video(MemoryVideo::new(1920, 1080, 16, 240)),
        ),
        "shader_switch" => (parse(shader_switch())?, Fixture::fonts(fonts()?)),
        name if name.starts_with("file:") => {
            let source = std::fs::read_to_string(&name["file:".len()..])?;
            let composition = lumen_engine::json::parse(&source).context("composition json")?;
            (composition, Fixture::fonts(fonts()?))
        }
        name => {
            let fixture =
                compositions::by_name(name).ok_or_else(|| anyhow!("unknown workload {name}"))?;
            (fixture.build(), Fixture::bench(fixture.media_store()))
        }
    })
}

/// The composition JSON shape: the last node feeds the output.
fn graph(frames: u32, nodes: Vec<Value>, connections: Vec<(u32, u32, &'static str)>) -> Value {
    let last = nodes.last().expect("a node")["id"].as_u64().expect("an id") as u32;
    let mut nodes = nodes;
    nodes.push(json!({ "id": 100_000, "type": "media_output" }));
    let mut connections = connections;
    connections.push((last, 100_000, "source"));
    let connections: Vec<Value> = connections
        .into_iter()
        .map(|(from, to, port)| json!({ "from_node": from, "to_node": to, "to_port": port }))
        .collect();
    json!({
        "timeline": { "fps": 60, "duration_frames": frames },
        "render_settings": { "width": 1920, "height": 1080 },
        "nodes": nodes,
        "connections": connections,
    })
}

/// `layers` merged over a background, feeding the output.
fn stacked(layers: Vec<Value>, wires: Vec<(u32, u32, &'static str)>) -> Value {
    let mut nodes = vec![json!({
        "id": 1, "type": "background", "params": { "paint": [18, 20, 28, 255] }
    })];
    let top = layers
        .iter()
        .map(|layer| layer["id"].as_u64().expect("an id") as u32)
        .collect::<Vec<_>>();
    nodes.extend(layers);
    let mut connections = wires;
    connections.push((1, 90_000, "layers"));
    connections.extend(
        top.into_iter()
            .filter(|id| !connections_feed_other(*id))
            .map(|id| (id, 90_000, "layers")),
    );
    nodes.push(json!({ "id": 90_000, "type": "raster_multimerge" }));
    graph(600, nodes, connections)
}

/// Layer ids ending in 1 feed the transform or blur at the next id rather than the stack.
fn connections_feed_other(id: u32) -> bool {
    id % 10 == 1
}

const WORDS: [&str; 6] = [
    "render graph",
    "frame 1024",
    "composition",
    "throughput 60",
    "shaping text",
    "WAVE AV To",
];

/// A selection among `options` that changes every `period` frames.
fn selected(options: &[String], period: u32) -> String {
    let mut expression = format!("'{}'", options[options.len() - 1]);
    for (index, option) in options[..options.len() - 1].iter().enumerate().rev() {
        expression = format!(
            "if(floor(frame / {period}) % {} == {index}, '{option}', {expression})",
            options.len()
        );
    }
    format!("={expression}")
}

/// Text whose content changes every frame and whose position always moves, so each frame
/// reshapes every node.
fn dynamic_text() -> Value {
    let layers = (0..24u32)
        .map(|index| {
            let options: Vec<String> = (0..WORDS.len())
                .map(|word| format!("{} {index}", WORDS[(word + index as usize) % WORDS.len()]))
                .collect();
            json!({
                "id": 10 * (index + 1), "type": "text", "params": {
                    "content": selected(&options, 1),
                    "font_family": if index % 2 == 0 { "Geist Mono" } else { "Geist" },
                    "font_size": 44,
                    "color": [235, 240, 250, 255],
                    "position": format!(
                        "=vec2({} + 30 * sin(frame / 40 + {index}), {})",
                        60 + 640 * (index % 3),
                        60 + 120 * (index / 3)
                    ),
                }
            })
        })
        .collect();
    stacked(layers, Vec::new())
}

/// Paths whose outlines change every frame, one rasterized from many points.
fn dynamic_path() -> Value {
    let outline = |index: u32, phase: u32| {
        let mut data = format!("M {} 0", 60 + phase * 4);
        for point in 1..40u32 {
            let angle = f64::from(point + index + phase) * 0.61;
            let radius = 60.0 + 40.0 * angle.sin();
            data += &format!(
                " L {:.1} {:.1}",
                radius * f64::from(point).cos(),
                radius * f64::from(point).sin()
            );
        }
        data + " Z"
    };
    let layers = (0..40u32)
        .map(|index| {
            let options: Vec<String> = (0..4).map(|phase| outline(index, phase)).collect();
            json!({
                "id": 10 * (index + 1), "type": "path", "params": {
                    "data": selected(&options, 1),
                    "fill_paint": [80 + index * 4, 200 - index * 3, 220, 255],
                    "position": format!(
                        "=vec2({} + 20 * sin(frame / 30), {})",
                        120 + 220 * (index % 8),
                        120 + 200 * (index / 8)
                    ),
                }
            })
        })
        .collect();
    stacked(layers, Vec::new())
}

/// Shapes, a transform and text sweeping across the canvas with motion blur.
fn motion_blur() -> Value {
    let mut layers = Vec::new();
    let mut wires = Vec::new();
    for index in 0..12u32 {
        let row = 80 + 80 * index;
        let position = format!("=vec2(960 + 700 * sin(frame / 25 + {index}), {row})");
        let base = 100 * (index + 1);
        match index % 3 {
            0 => layers.push(json!({
                "id": base, "type": "shape", "params": {
                    "position": position, "width": 160, "height": 60,
                    "fill_paint": [240, 120, 60, 255], "motion_blur": 180,
                }
            })),
            1 => layers.push(json!({
                "id": base, "type": "text", "params": {
                    "content": "Motion", "font_family": "Geist", "font_size": 64,
                    "color": [250, 250, 250, 255], "position": position, "motion_blur": 180,
                }
            })),
            _ => {
                layers.push(json!({
                    "id": base + 1, "type": "shape", "params": {
                        "position": [120, row], "width": 160, "height": 60,
                        "fill_paint": [60, 180, 240, 255],
                    }
                }));
                layers.push(json!({
                    "id": base + 2, "type": "transform", "params": {
                        "translate_x": format!("=800 + 700 * sin(frame / 25 + {index})"),
                        "motion_blur": 180,
                    }
                }));
                wires.push((base + 1, base + 2, "source"));
            }
        }
    }
    stacked(layers, wires)
}

/// A 1080p video through a time remap, an exposure, a blur and a moving transform.
fn remapped_media() -> Value {
    let nodes = vec![
        json!({ "id": 1, "type": "media_in", "params": { "kind": "video", "source": "video" } }),
        json!({ "id": 2, "type": "time_remap", "params": {
            "frame": "=frame * 0.5", "loop_enabled": true, "loop_start": 0, "loop_end": 120
        } }),
        json!({ "id": 3, "type": "exposure", "params": { "exposure": "=sin(frame / 30) * 0.5" } }),
        json!({ "id": 4, "type": "blur", "params": { "radius": 6 } }),
        json!({ "id": 5, "type": "transform", "params": {
            "translate_x": "=80 * sin(frame / 20)", "scale_x": 0.8, "scale_y": 0.8
        } }),
        json!({ "id": 6, "type": "background", "params": { "paint": [8, 8, 10, 255] } }),
        json!({ "id": 7, "type": "merge" }),
    ];
    graph(
        600,
        nodes,
        vec![
            (1, 2, "source"),
            (2, 3, "source"),
            (3, 4, "source"),
            (4, 5, "source"),
            (6, 7, "base"),
            (5, 7, "overlay"),
        ],
    )
}

/// A shader whose source an expression selects, and a switch between two branches.
fn shader_switch() -> Value {
    let source = |red: &str| {
        format!(
            "@compute @workgroup_size(8, 8, 1) fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {{ let uv = vec2<f32>(id.xy) / lumen.resolution; let source = textureSampleLevel(input0, input_sampler, uv, 0.0); textureStore(output, vec2<i32>(id.xy), vec4<f32>({red}, source.gba)); }}"
        )
    };
    let options = vec![
        source("source.r"),
        source("1.0 - source.r"),
        source("source.r * uv.x"),
    ];
    let nodes = vec![
        json!({ "id": 1, "type": "background", "params": { "paint": [40, 90, 160, 255] } }),
        json!({ "id": 2, "type": "shape", "params": {
            "position": "=vec2(960 + 400 * sin(frame / 30), 540)",
            "width": 500, "height": 400, "fill_paint": [240, 200, 60, 255],
        } }),
        json!({ "id": 3, "type": "merge" }),
        json!({ "id": 4, "type": "wgsl_shader", "params": { "shader": selected(&options, 20) } }),
        json!({ "id": 5, "type": "exposure", "params": { "exposure": 0.3 } }),
        json!({ "id": 6, "type": "switch", "params": {
            "selected_layer": "=floor(frame / 15) % 2"
        } }),
    ];
    graph(
        600,
        nodes,
        vec![
            (1, 3, "base"),
            (2, 3, "overlay"),
            (3, 4, "source"),
            (4, 5, "source"),
            (4, 6, "layers"),
            (5, 6, "layers"),
        ],
    )
}

struct Fixture {
    fonts: Option<FontDir>,
    video: Option<MemoryVideo>,
    bench: BenchmarkMediaStore,
}

impl Fixture {
    fn fonts(fonts: FontDir) -> Self {
        Self {
            fonts: Some(fonts),
            video: None,
            bench: BenchmarkMediaStore::default(),
        }
    }

    fn video(video: MemoryVideo) -> Self {
        Self {
            fonts: None,
            video: Some(video),
            bench: BenchmarkMediaStore::default(),
        }
    }

    fn bench(bench: BenchmarkMediaStore) -> Self {
        Self {
            fonts: None,
            video: None,
            bench,
        }
    }
}

impl std::fmt::Debug for Fixture {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fixture").finish_non_exhaustive()
    }
}

impl MediaStore for Fixture {
    fn get_image_resolver(&self, source: &str) -> Result<Box<dyn ImageResolver>, MediaError> {
        self.bench.get_image_resolver(source)
    }

    fn get_video_resolver(
        &self,
        stream_id: &str,
    ) -> Result<Box<dyn VideoFrameResolver>, MediaError> {
        let video = self.video.as_ref().filter(|_| stream_id == "video");
        video
            .map(|video| Box::new(video.clone()) as Box<dyn VideoFrameResolver>)
            .ok_or_else(|| MediaError::source_not_found(stream_id, "the bench has no such video"))
    }

    fn font_resolver_id(&self, font_family: &str) -> Option<String> {
        self.fonts.as_ref()?.resolver_id(font_family)
    }

    fn get_font_resolver(
        &self,
        font_family: &str,
    ) -> Result<Option<Box<dyn FontResolver>>, MediaError> {
        Ok(self
            .fonts
            .as_ref()
            .and_then(|fonts| fonts.resolver(font_family)))
    }
}

/// Fonts of one directory, matched by family name.
struct FontDir {
    database: fontdb::Database,
    source: String,
}

impl FontDir {
    fn open(path: &Path) -> anyhow::Result<Self> {
        if !path.is_dir() {
            bail!("no font directory at {}", path.display());
        }
        let mut database = fontdb::Database::new();
        database.load_fonts_dir(path);
        Ok(Self {
            database,
            source: path.display().to_string(),
        })
    }

    fn faces(&self, family: &str) -> Vec<fontdb::ID> {
        self.database
            .faces()
            .filter(|face| {
                face.families
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(family))
            })
            .map(|face| face.id)
            .collect()
    }

    fn resolver_id(&self, family: &str) -> Option<String> {
        (!self.faces(family).is_empty()).then(|| format!("bench-font:{family}:{}", self.source))
    }

    fn resolver(&self, family: &str) -> Option<Box<dyn FontResolver>> {
        let mut data: Vec<Vec<u8>> = Vec::new();
        for face in self.faces(family) {
            self.database.with_face_data(face, |bytes, _| {
                if !data.iter().any(|existing| existing.as_slice() == bytes) {
                    data.push(bytes.to_vec());
                }
            })?;
        }
        (!data.is_empty()).then(|| {
            Box::new(FontFiles {
                id: format!("bench-font:{family}:{}", self.source),
                data,
            }) as Box<dyn FontResolver>
        })
    }
}

struct FontFiles {
    id: String,
    data: Vec<Vec<u8>>,
}

impl FontResolver for FontFiles {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        Ok(self.data.clone())
    }
}

/// A video of a few distinct frames held in memory, so decoding costs nothing in the comparison.
#[derive(Clone)]
struct MemoryVideo {
    frames: Arc<Vec<Arc<CpuMediaFrame>>>,
    frame_count: u32,
}

impl MemoryVideo {
    fn new(width: u32, height: u32, distinct: u32, frame_count: u32) -> Self {
        let frames = (0..distinct)
            .map(|frame| {
                let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
                for y in 0..height {
                    for x in 0..width {
                        let light = ((x + frame * 8) / 32 + y / 32) % 2 == 0;
                        rgba.extend_from_slice(&[
                            (x * 255 / width) as u8,
                            (y * 255 / height) as u8,
                            if light { 200 } else { 40 },
                            255,
                        ]);
                    }
                }
                Arc::new(CpuMediaFrame {
                    rgba: Arc::new(rgba),
                    width,
                    height,
                    row_bytes: width as usize * 4,
                })
            })
            .collect();
        Self {
            frames: Arc::new(frames),
            frame_count,
        }
    }
}

impl VideoFrameResolver for MemoryVideo {
    fn id(&self) -> &str {
        "memory-video"
    }

    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: self.frames[0].width,
            height: self.frames[0].height,
            frame_count: self.frame_count,
            fps: 30.0,
        }
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
        let index = frame as usize % self.frames.len();
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frames[index])))
    }
}
