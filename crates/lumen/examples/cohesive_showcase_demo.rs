use std::{
    collections::HashMap,
    fs::{self, File},
    io::{BufWriter, Write},
    path::{Path, PathBuf},
    process::Command,
    sync::{Arc, Mutex, RwLock},
    time::Instant,
};

use anyhow::{Context, Result, anyhow, bail};
use lumen::{
    AssetCache, BlendMode, Composition, Connection, Extrapolation, Graph, InputPort,
    InterpolationMode, KeyframeTrack, NodeId, NodeKind, OutputPort, RasterFrame, RenderContext,
    RenderSettings, RuntimeCapabilityProfile, Sink, SinkType, SurfacePool, TimelineSettings,
    TrackId, Warning,
    animation::PropertyPath,
    error::SinkError,
    media::{ImageResolver, MediaStore, VideoFrameResolver, premultiply_rgba_in_place_if_needed},
    node::{
        Node, PropertyValue, ShapeGeometry,
        blur::Blur,
        boolean::{Boolean, MaskKind},
        crop::Crop,
        frame_hold::FrameHold,
        media_in::{LoopMode, MediaIn, MediaInKind},
        media_output::MediaOutput,
        memo::Memo,
        merge::Merge,
        resize::{Resize, ResizeMode, ResizeSampling},
        shadow::Shadow,
        shape::Shape,
        shape_renderer::ShapeRenderer,
        solid_color::SolidColor,
        switch::Switch,
        text::{
            Text, TextAlignment, TextAlignmentHorizontal, TextAlignmentVertical, TextFontStyle,
        },
        transform::Transform,
    },
    sink::BitmapSink,
};

const WIDTH: u32 = 960;
const HEIGHT: u32 = 540;
const FPS: u32 = 30;
const DURATION_FRAMES: u32 = 240;

#[derive(Default)]
struct ImageResolveStats {
    calls: usize,
}

#[derive(Default)]
struct VideoResolveStats {
    calls: usize,
    requested_frames: Vec<u32>,
}

#[derive(Clone)]
struct DemoImageResolver {
    id: String,
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
    stats: Arc<Mutex<ImageResolveStats>>,
}

impl DemoImageResolver {
    fn new(
        id: impl Into<String>,
        width: u32,
        height: u32,
        pixels: Vec<u8>,
        stats: Arc<Mutex<ImageResolveStats>>,
    ) -> Self {
        let mut pixels = pixels;
        premultiply_rgba_in_place_if_needed(&mut pixels);
        Self {
            id: id.into(),
            width,
            height,
            pixels: Arc::new(pixels),
            stats,
        }
    }
}

impl ImageResolver for DemoImageResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn resolve(&self) -> Result<Arc<Vec<u8>>, lumen::error::MediaError> {
        if let Ok(mut stats) = self.stats.lock() {
            stats.calls += 1;
        }
        Ok(Arc::clone(&self.pixels))
    }
}

#[derive(Clone)]
struct DemoVideoResolver {
    id: String,
    width: u32,
    height: u32,
    frame_count: u32,
    stats: Arc<Mutex<VideoResolveStats>>,
}

impl DemoVideoResolver {
    fn new(
        id: impl Into<String>,
        width: u32,
        height: u32,
        frame_count: u32,
        stats: Arc<Mutex<VideoResolveStats>>,
    ) -> Self {
        Self {
            id: id.into(),
            width,
            height,
            frame_count,
            stats,
        }
    }
}

impl VideoFrameResolver for DemoVideoResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn width(&self) -> u32 {
        self.width
    }

    fn height(&self) -> u32 {
        self.height
    }

    fn frame_count(&self) -> u32 {
        self.frame_count
    }

    fn resolve_frame(&self, frame: u32) -> Result<Arc<Vec<u8>>, lumen::error::MediaError> {
        if frame >= self.frame_count {
            return Err(lumen::error::MediaError::FrameOutOfRange {
                media_source: self.id.clone(),
                frame,
                frame_count: self.frame_count,
            });
        }
        if let Ok(mut stats) = self.stats.lock() {
            stats.calls += 1;
            stats.requested_frames.push(frame);
        }
        let mut pixels = procedural_video_frame(self.width, self.height, frame);
        premultiply_rgba_in_place_if_needed(&mut pixels);
        Ok(Arc::new(pixels))
    }
}

struct DemoMediaStore {
    image: DemoImageResolver,
    video: DemoVideoResolver,
}

impl MediaStore for DemoMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        (source == self.image.id()).then(|| Box::new(self.image.clone()) as Box<dyn ImageResolver>)
    }

    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        (source == self.video.id())
            .then(|| Box::new(self.video.clone()) as Box<dyn VideoFrameResolver>)
    }
}

#[derive(Default, Clone)]
struct RawSinkSharedStats {
    frames_written: usize,
    bytes_written: u64,
    first_frame: Option<u32>,
    last_frame: Option<u32>,
    frame_checksum: u64,
}

struct RawRgbaSink {
    writer: BufWriter<File>,
    width: u32,
    height: u32,
    shared: Arc<Mutex<RawSinkSharedStats>>,
}

impl RawRgbaSink {
    fn create(
        path: &Path,
        width: u32,
        height: u32,
        shared: Arc<Mutex<RawSinkSharedStats>>,
    ) -> Result<Self> {
        let file = File::create(path)
            .with_context(|| format!("failed to create raw video file {}", path.display()))?;
        Ok(Self {
            writer: BufWriter::new(file),
            width,
            height,
            shared,
        })
    }
}

impl Sink for RawRgbaSink {
    fn write_frame(&mut self, frame: u32, data: &RasterFrame) -> Result<(), SinkError> {
        let bitmap = data
            .clone()
            .to_bitmap()
            .map_err(|error| SinkError::WriteFrame {
                frame,
                details: error.to_string(),
            })?;
        let RasterFrame::Bitmap(bitmap) = bitmap else {
            return Err(SinkError::WriteFrame {
                frame,
                details: "expected bitmap frame".to_string(),
            });
        };
        if bitmap.storage_width != self.width || bitmap.storage_height != self.height {
            return Err(SinkError::WriteFrame {
                frame,
                details: format!(
                    "unexpected frame dimensions {}x{}, expected {}x{}",
                    bitmap.storage_width, bitmap.storage_height, self.width, self.height
                ),
            });
        }

        self.writer
            .write_all(bitmap.pixels.as_slice())
            .map_err(|error| SinkError::WriteFrame {
                frame,
                details: error.to_string(),
            })?;

        if let Ok(mut stats) = self.shared.lock() {
            stats.frames_written += 1;
            stats.bytes_written = stats
                .bytes_written
                .saturating_add(u64::try_from(bitmap.pixels.len()).unwrap_or(0));
            stats.first_frame.get_or_insert(frame);
            stats.last_frame = Some(frame);
            stats.frame_checksum = stats.frame_checksum.wrapping_add(
                bitmap
                    .pixels
                    .iter()
                    .map(|byte| u64::from(*byte))
                    .sum::<u64>(),
            );
        }

        Ok(())
    }

    fn finalize(&mut self) -> Result<(), SinkError> {
        self.writer.flush().map_err(|error| SinkError::Finalize {
            details: error.to_string(),
        })
    }
}

fn main() -> Result<()> {
    let started = Instant::now();
    let artifact_dir = workspace_root()
        .join("artifacts")
        .join("lumen-cohesive-showcase-demo");
    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("failed to create {}", artifact_dir.display()))?;

    let raw_warmup = artifact_dir.join("warmup_pass.rgba");
    let raw_final = artifact_dir.join("final_pass.rgba");
    let video_path = artifact_dir.join("lumen_cohesive_showcase_demo.mp4");
    let stats_path = artifact_dir.join("stats.txt");

    let image_stats = Arc::new(Mutex::new(ImageResolveStats::default()));
    let video_stats = Arc::new(Mutex::new(VideoResolveStats::default()));
    let media_store: Arc<dyn MediaStore> = Arc::new(DemoMediaStore {
        image: DemoImageResolver::new(
            "demo-image",
            512,
            512,
            procedural_image_rgba(512, 512),
            Arc::clone(&image_stats),
        ),
        video: DemoVideoResolver::new("demo-video", WIDTH, HEIGHT, 300, Arc::clone(&video_stats)),
    });

    let (composition, feature_nodes) = build_feature_composition()?;
    let profile = RuntimeCapabilityProfile {
        has_image_resolver: true,
        has_video_resolver: true,
        has_threading: true,
        sink_types: vec![SinkType::Bitmap, SinkType::Video, SinkType::ImageSequence],
    };

    let validation_started = Instant::now();
    let warnings = composition
        .validate(&profile)
        .map_err(|errors| anyhow!("composition validation failed: {errors:?}"))?;
    let validation_elapsed = validation_started.elapsed();

    let json_smoke_started = Instant::now();
    let json_smoke_status = run_json_delegate_smoke()?;
    let json_smoke_elapsed = json_smoke_started.elapsed();

    let pool_smoke_started = Instant::now();
    let pool_smoke_status = run_surface_pool_smoke()?;
    let pool_smoke_elapsed = pool_smoke_started.elapsed();

    let shared_asset_cache = Arc::new(RwLock::new(AssetCache::new()));
    let surface_pool = Arc::new(SurfacePool::new());

    let single_frame_started = Instant::now();
    let mut single_ctx = RenderContext::new(
        &composition,
        Arc::clone(&surface_pool),
        Arc::clone(&shared_asset_cache),
        Arc::clone(&media_store),
        profile.clone(),
    );
    let preview_frame = composition.render_frame(0, &mut single_ctx)?;
    let preview_dims = preview_frame.dimensions();
    let single_frame_elapsed = single_frame_started.elapsed();

    let warmup_stats = Arc::new(Mutex::new(RawSinkSharedStats::default()));
    let warmup_ctx = RenderContext::new(
        &composition,
        Arc::clone(&surface_pool),
        Arc::clone(&shared_asset_cache),
        Arc::clone(&media_store),
        profile.clone(),
    );
    let warmup_started = Instant::now();
    composition.render_sequence(
        0..DURATION_FRAMES,
        warmup_ctx,
        Box::new(RawRgbaSink::create(
            &raw_warmup,
            WIDTH,
            HEIGHT,
            Arc::clone(&warmup_stats),
        )?),
        4,
    )?;
    let warmup_elapsed = warmup_started.elapsed();

    let final_stats = Arc::new(Mutex::new(RawSinkSharedStats::default()));
    let final_ctx = RenderContext::new(
        &composition,
        Arc::clone(&surface_pool),
        Arc::clone(&shared_asset_cache),
        Arc::clone(&media_store),
        profile,
    );
    let final_render_started = Instant::now();
    composition.render_sequence(
        0..DURATION_FRAMES,
        final_ctx,
        Box::new(RawRgbaSink::create(
            &raw_final,
            WIDTH,
            HEIGHT,
            Arc::clone(&final_stats),
        )?),
        4,
    )?;
    let final_render_elapsed = final_render_started.elapsed();

    let encode_started = Instant::now();
    encode_raw_rgba_to_mp4(&raw_final, &video_path, WIDTH, HEIGHT, FPS)?;
    let encode_elapsed = encode_started.elapsed();

    let _ = fs::remove_file(&raw_warmup);
    let _ = fs::remove_file(&raw_final);

    let video_meta = fs::metadata(&video_path)
        .with_context(|| format!("failed to read {}", video_path.display()))?;
    let warmup_stats_snapshot = warmup_stats.lock().map(|s| s.clone()).unwrap_or_default();
    let final_stats_snapshot = final_stats.lock().map(|s| s.clone()).unwrap_or_default();
    let image_stats_snapshot = image_stats.lock().map(|s| s.calls).unwrap_or_default();
    let video_stats_snapshot = video_stats
        .lock()
        .map(|s| (s.calls, s.requested_frames.clone()))
        .unwrap_or_default();

    let warnings_text = format_warnings(&warnings);
    let render_fps = DURATION_FRAMES as f64 / final_render_elapsed.as_secs_f64().max(1e-9);
    let total_elapsed = started.elapsed();

    let mut stats_report = String::new();
    use std::fmt::Write as _;
    writeln!(&mut stats_report, "lumen cohesive showcase demo")?;
    writeln!(&mut stats_report, "video={}", video_path.display())?;
    writeln!(&mut stats_report, "video_size_bytes={}", video_meta.len())?;
    writeln!(
        &mut stats_report,
        "timeline={}x{} @ {}fps, frames={}",
        WIDTH, HEIGHT, FPS, DURATION_FRAMES
    )?;
    writeln!(
        &mut stats_report,
        "preview_frame_dims={}x{}",
        preview_dims.0, preview_dims.1
    )?;
    writeln!(&mut stats_report, "nodes_used={}", feature_nodes.join(","))?;
    writeln!(&mut stats_report, "tracks={}", composition.tracks.len())?;
    writeln!(
        &mut stats_report,
        "expressions={}",
        composition
            .expressions
            .values()
            .map(|m| m.len())
            .sum::<usize>()
    )?;
    writeln!(
        &mut stats_report,
        "validation_ms={:.2}",
        validation_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "json_delegate_smoke={} ({:.2} ms)",
        json_smoke_status,
        json_smoke_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "surface_pool_smoke={} ({:.2} ms)",
        pool_smoke_status,
        pool_smoke_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "single_frame_render_ms={:.2}",
        single_frame_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "threaded_warmup_render_ms={:.2}",
        warmup_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "threaded_final_render_ms={:.2}",
        final_render_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "threaded_final_render_fps={:.2}",
        render_fps
    )?;
    writeln!(
        &mut stats_report,
        "encode_ms={:.2}",
        encode_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "total_ms={:.2}",
        total_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut stats_report,
        "warmup_sink_frames={},bytes={},checksum={}",
        warmup_stats_snapshot.frames_written,
        warmup_stats_snapshot.bytes_written,
        warmup_stats_snapshot.frame_checksum
    )?;
    writeln!(
        &mut stats_report,
        "final_sink_frames={},bytes={},checksum={},first_frame={:?},last_frame={:?}",
        final_stats_snapshot.frames_written,
        final_stats_snapshot.bytes_written,
        final_stats_snapshot.frame_checksum,
        final_stats_snapshot.first_frame,
        final_stats_snapshot.last_frame
    )?;
    writeln!(
        &mut stats_report,
        "image_resolve_calls={image_stats_snapshot}"
    )?;
    writeln!(
        &mut stats_report,
        "video_resolve_calls={}",
        video_stats_snapshot.0
    )?;
    if !video_stats_snapshot.1.is_empty() {
        let min = *video_stats_snapshot.1.iter().min().unwrap_or(&0);
        let max = *video_stats_snapshot.1.iter().max().unwrap_or(&0);
        let unique = video_stats_snapshot
            .1
            .iter()
            .copied()
            .collect::<std::collections::BTreeSet<_>>()
            .len();
        writeln!(
            &mut stats_report,
            "video_requested_frames_min={min},max={max},unique={unique}"
        )?;
    }
    writeln!(&mut stats_report, "warnings_count={}", warnings.len())?;
    writeln!(&mut stats_report, "warnings={warnings_text}")?;

    fs::write(&stats_path, &stats_report)
        .with_context(|| format!("failed to write {}", stats_path.display()))?;

    println!("{stats_report}");
    Ok(())
}

fn build_feature_composition() -> Result<(Composition, Vec<&'static str>)> {
    let mut graph = Graph::new();
    let mut feature_nodes = Vec::new();
    let bg = add_node(
        &mut graph,
        &mut feature_nodes,
        "SolidColor",
        NodeKind::SolidColor(SolidColor {
            color: [8, 12, 20, 255],
            width: Some(WIDTH),
            height: Some(HEIGHT),
        }),
    );

    // Large circular vignette mask used in multiple phases.
    let vignette_shape = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shape",
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Ellipse {
                width: 780,
                height: 420,
            },
        }),
    );
    let vignette_renderer = add_node(
        &mut graph,
        &mut feature_nodes,
        "ShapeRenderer",
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [255, 255, 255, 235],
            stroke_enabled: true,
            stroke_width: 4.0,
            stroke_color: [255, 255, 255, 255],
            ..ShapeRenderer::default()
        }),
    );
    connect(
        &mut graph,
        vignette_shape,
        "vector",
        vignette_renderer,
        InputPort::named("vector"),
    )?;
    let vignette_full = add_node(
        &mut graph,
        &mut feature_nodes,
        "Resize",
        NodeKind::Resize(Resize {
            width: WIDTH,
            height: HEIGHT,
            mode: ResizeMode::Fill,
            sampling: ResizeSampling::Linear,
        }),
    );
    connect(
        &mut graph,
        vignette_renderer,
        "output",
        vignette_full,
        InputPort::named("source"),
    )?;

    // A polygon pattern card and title become a memoized "hero plate".
    let poly_shape = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shape",
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Polygon {
                points: vec![
                    (40.0, 20.0),
                    (900.0, 40.0),
                    (860.0, 500.0),
                    (120.0, 520.0),
                    (20.0, 280.0),
                ],
            },
        }),
    );
    let poly_renderer = add_node(
        &mut graph,
        &mut feature_nodes,
        "ShapeRenderer",
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [28, 58, 86, 235],
            stroke_enabled: true,
            stroke_width: 3.0,
            stroke_color: [90, 160, 220, 255],
            ..ShapeRenderer::default()
        }),
    );
    connect(
        &mut graph,
        poly_shape,
        "vector",
        poly_renderer,
        InputPort::named("vector"),
    )?;
    let poly_full = add_node(
        &mut graph,
        &mut feature_nodes,
        "Resize",
        NodeKind::Resize(Resize {
            width: WIDTH,
            height: HEIGHT,
            mode: ResizeMode::Stretch,
            sampling: ResizeSampling::Nearest,
        }),
    );
    connect(
        &mut graph,
        poly_renderer,
        "output",
        poly_full,
        InputPort::named("source"),
    )?;

    let panel_shape = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shape",
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 640,
                height: 220,
            },
        }),
    );
    let panel_renderer = add_node(
        &mut graph,
        &mut feature_nodes,
        "ShapeRenderer",
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [246, 106, 62, 180],
            stroke_enabled: true,
            stroke_width: 2.0,
            stroke_color: [255, 238, 230, 255],
            ..ShapeRenderer::default()
        }),
    );
    connect(
        &mut graph,
        panel_shape,
        "vector",
        panel_renderer,
        InputPort::named("vector"),
    )?;
    let panel_full = add_node(
        &mut graph,
        &mut feature_nodes,
        "Resize",
        NodeKind::Resize(Resize {
            width: WIDTH,
            height: HEIGHT,
            mode: ResizeMode::Fit,
            sampling: ResizeSampling::Linear,
        }),
    );
    connect(
        &mut graph,
        panel_renderer,
        "output",
        panel_full,
        InputPort::named("source"),
    )?;

    let title_text = add_node(
        &mut graph,
        &mut feature_nodes,
        "Text",
        NodeKind::Text(Text {
            content: "Lumen / Next Showcase\nA phased render tour (not all at once)".to_string(),
            font_family: "Helvetica".to_string(),
            font_size: 46.0,
            font_weight: 700,
            font_style: TextFontStyle::Italic,
            max_width: Some(WIDTH as f32),
            color: [248, 250, 245, 255],
            alignment: TextAlignment {
                horizontal: TextAlignmentHorizontal::Center,
                vertical: TextAlignmentVertical::Middle,
            },
        }),
    );
    let title_full = add_node(
        &mut graph,
        &mut feature_nodes,
        "Resize",
        NodeKind::Resize(Resize {
            width: WIDTH,
            height: HEIGHT,
            mode: ResizeMode::Stretch,
            sampling: ResizeSampling::Linear,
        }),
    );
    connect(
        &mut graph,
        title_text,
        "output",
        title_full,
        InputPort::named("source"),
    )?;

    let hero_merge_a = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Screen,
            opacity: 0.95,
        }),
    );
    connect(
        &mut graph,
        poly_full,
        "output",
        hero_merge_a,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        title_full,
        "output",
        hero_merge_a,
        InputPort::named("overlay"),
    )?;
    connect(
        &mut graph,
        vignette_full,
        "output",
        hero_merge_a,
        InputPort::named("mask"),
    )?;

    let hero_merge_b = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Overlay,
            opacity: 0.7,
        }),
    );
    connect(
        &mut graph,
        hero_merge_a,
        "output",
        hero_merge_b,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        panel_full,
        "output",
        hero_merge_b,
        InputPort::named("overlay"),
    )?;

    let hero_shadow = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shadow",
        NodeKind::Shadow(Shadow {
            offset_x: 18,
            offset_y: 14,
            color: [0, 0, 0, 120],
        }),
    );
    connect(
        &mut graph,
        hero_merge_b,
        "output",
        hero_shadow,
        InputPort::named("source"),
    )?;

    let hero_blur = add_node(
        &mut graph,
        &mut feature_nodes,
        "Blur",
        NodeKind::Blur(Blur { radius: 2.0 }),
    );
    connect(
        &mut graph,
        hero_shadow,
        "output",
        hero_blur,
        InputPort::named("source"),
    )?;

    let hero_memo = add_node(
        &mut graph,
        &mut feature_nodes,
        "Memo",
        NodeKind::Memo(Memo {
            cache_id: "cohesive-showcase-hero-plate".to_string(),
            allow_expressions: false,
        }),
    );
    connect(
        &mut graph,
        hero_blur,
        "output",
        hero_memo,
        InputPort::named("source"),
    )?;

    let hero_transform = add_node(
        &mut graph,
        &mut feature_nodes,
        "Transform",
        NodeKind::Transform(Transform::default()),
    );
    connect(
        &mut graph,
        hero_memo,
        "output",
        hero_transform,
        InputPort::named("source"),
    )?;

    // Image branch: crop + resize + boolean mask + transform.
    let image_in = add_node(
        &mut graph,
        &mut feature_nodes,
        "MediaIn",
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Image {
                source: "demo-image".to_string(),
            },
        }),
    );
    let image_crop = add_node(
        &mut graph,
        &mut feature_nodes,
        "Crop",
        NodeKind::Crop(Crop {
            x: 32,
            y: 32,
            width: 420,
            height: 420,
        }),
    );
    connect(
        &mut graph,
        image_in,
        "output",
        image_crop,
        InputPort::named("source"),
    )?;
    let image_resize = add_node(
        &mut graph,
        &mut feature_nodes,
        "Resize",
        NodeKind::Resize(Resize {
            width: WIDTH,
            height: HEIGHT,
            mode: ResizeMode::Fit,
            sampling: ResizeSampling::Linear,
        }),
    );
    connect(
        &mut graph,
        image_crop,
        "output",
        image_resize,
        InputPort::named("source"),
    )?;
    let image_boolean = add_node(
        &mut graph,
        &mut feature_nodes,
        "Boolean",
        NodeKind::Boolean(Boolean {
            mask_kind: MaskKind::Luma,
            invert: false,
        }),
    );
    connect(
        &mut graph,
        image_resize,
        "output",
        image_boolean,
        InputPort::named("source"),
    )?;
    connect(
        &mut graph,
        vignette_full,
        "output",
        image_boolean,
        InputPort::named("mask"),
    )?;
    connect(
        &mut graph,
        panel_shape,
        "vector",
        image_boolean,
        InputPort::named("vector"),
    )?;
    let image_transform = add_node(
        &mut graph,
        &mut feature_nodes,
        "Transform",
        NodeKind::Transform(Transform::default()),
    );
    connect(
        &mut graph,
        image_boolean,
        "output",
        image_transform,
        InputPort::named("source"),
    )?;

    // Video branch: live video, freeze-frame variant, then switch between them by timeline phase.
    let video_in = add_node(
        &mut graph,
        &mut feature_nodes,
        "MediaIn",
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "demo-video".to_string(),
                range: Some(0..180),
                speed: 1.0,
                loop_mode: LoopMode::Repeat,
            },
        }),
    );
    let video_crop = add_node(
        &mut graph,
        &mut feature_nodes,
        "Crop",
        NodeKind::Crop(Crop {
            x: 80,
            y: 45,
            width: 800,
            height: 450,
        }),
    );
    connect(
        &mut graph,
        video_in,
        "output",
        video_crop,
        InputPort::named("source"),
    )?;
    let video_resize = add_node(
        &mut graph,
        &mut feature_nodes,
        "Resize",
        NodeKind::Resize(Resize {
            width: WIDTH,
            height: HEIGHT,
            mode: ResizeMode::Fill,
            sampling: ResizeSampling::Linear,
        }),
    );
    connect(
        &mut graph,
        video_crop,
        "output",
        video_resize,
        InputPort::named("source"),
    )?;
    let video_transform = add_node(
        &mut graph,
        &mut feature_nodes,
        "Transform",
        NodeKind::Transform(Transform::default()),
    );
    connect(
        &mut graph,
        video_resize,
        "output",
        video_transform,
        InputPort::named("source"),
    )?;

    let video_hold = add_node(
        &mut graph,
        &mut feature_nodes,
        "FrameHold",
        NodeKind::FrameHold(FrameHold { hold_frame: 96 }),
    );
    connect(
        &mut graph,
        video_transform,
        "output",
        video_hold,
        InputPort::named("source"),
    )?;

    let mut switch_map = HashMap::new();
    switch_map.insert(0, 0..80);
    switch_map.insert(1, 80..190);
    switch_map.insert(2, 190..240);
    let scene_switch = add_node(
        &mut graph,
        &mut feature_nodes,
        "Switch",
        NodeKind::Switch(Switch::new(switch_map)),
    );
    connect(
        &mut graph,
        image_transform,
        "output",
        scene_switch,
        InputPort::Indexed(0),
    )?;
    connect(
        &mut graph,
        video_transform,
        "output",
        scene_switch,
        InputPort::Indexed(1),
    )?;
    connect(
        &mut graph,
        video_hold,
        "output",
        scene_switch,
        InputPort::Indexed(2),
    )?;

    let switched_shadow = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shadow",
        NodeKind::Shadow(Shadow {
            offset_x: -10,
            offset_y: 10,
            color: [0, 0, 0, 90],
        }),
    );
    connect(
        &mut graph,
        scene_switch,
        "output",
        switched_shadow,
        InputPort::named("source"),
    )?;

    // Layering: background + memoized hero, then blended media, then an intentional no-op merge.
    let base_merge = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
        }),
    );
    connect(
        &mut graph,
        bg,
        "output",
        base_merge,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        hero_transform,
        "output",
        base_merge,
        InputPort::named("overlay"),
    )?;
    connect(
        &mut graph,
        vignette_full,
        "output",
        base_merge,
        InputPort::named("mask"),
    )?;

    let media_merge = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Lighten,
            opacity: 0.85,
        }),
    );
    connect(
        &mut graph,
        base_merge,
        "output",
        media_merge,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        switched_shadow,
        "output",
        media_merge,
        InputPort::named("overlay"),
    )?;
    connect(
        &mut graph,
        panel_full,
        "output",
        media_merge,
        InputPort::named("mask"),
    )?;

    let noop_merge = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Multiply,
            opacity: 0.0,
        }),
    );
    connect(
        &mut graph,
        media_merge,
        "output",
        noop_merge,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        title_full,
        "output",
        noop_merge,
        InputPort::named("overlay"),
    )?;

    let output = add_node(
        &mut graph,
        &mut feature_nodes,
        "MediaOutput",
        NodeKind::MediaOutput(MediaOutput),
    );
    connect(
        &mut graph,
        noop_merge,
        "output",
        output,
        InputPort::named("source"),
    )?;

    let mut composition = Composition::new(
        graph,
        TimelineSettings {
            fps: FPS as f32,
            duration_frames: DURATION_FRAMES,
        },
        RenderSettings {
            width: WIDTH,
            height: HEIGHT,
            background_color: [0, 0, 0, 255],
        },
    );

    // Keyframes: staged image motion during intro (step + linear mix).
    let mut image_x = KeyframeTrack::new(
        TrackId(2001),
        image_transform,
        PropertyPath::new("transform.translate_x"),
        lumen::AnimatableType::Float,
    );
    image_x.before_extrapolation = Extrapolation::Hold;
    image_x.after_extrapolation = Extrapolation::Hold;
    image_x.set_key(0, PropertyValue::Float(-160.0), InterpolationMode::Linear);
    image_x.set_key(40, PropertyValue::Float(0.0), InterpolationMode::Linear);
    image_x.set_key(79, PropertyValue::Float(120.0), InterpolationMode::Linear);
    composition.add_track(image_x);

    let mut image_y = KeyframeTrack::new(
        TrackId(2002),
        image_transform,
        PropertyPath::new("transform.translate_y"),
        lumen::AnimatableType::Float,
    );
    image_y.set_key(0, PropertyValue::Float(-40.0), InterpolationMode::Step);
    image_y.set_key(26, PropertyValue::Float(0.0), InterpolationMode::Step);
    image_y.set_key(53, PropertyValue::Float(35.0), InterpolationMode::Step);
    image_y.set_key(79, PropertyValue::Float(-15.0), InterpolationMode::Step);
    composition.add_track(image_y);

    // Video branch motion track.
    let mut video_ty = KeyframeTrack::new(
        TrackId(2003),
        video_transform,
        PropertyPath::new("transform.translate_y"),
        lumen::AnimatableType::Float,
    );
    video_ty.set_key(80, PropertyValue::Float(0.0), InterpolationMode::Linear);
    video_ty.set_key(140, PropertyValue::Float(-26.0), InterpolationMode::Linear);
    video_ty.set_key(189, PropertyValue::Float(12.0), InterpolationMode::Linear);
    composition.add_track(video_ty);

    // Expression-driven hero plate drift/rotation.
    composition.set_expression(
        hero_transform,
        "transform.rotate",
        lumen::Expression::parse(
            "sin(time * 0.8) * 2 + clamp(text_width('showcase', 24) / 200, 0, 3)",
        )?,
    );
    composition.set_expression(
        hero_transform,
        "transform.translate_y",
        lumen::Expression::parse("sin(time * 1.7) * 10")?,
    );
    composition.set_expression(
        video_transform,
        "transform.translate_x",
        lumen::Expression::parse("sin(time * 0.9) * (width / 30)")?,
    );

    // Precedence demo: keyframe exists, expression wins for media layer opacity.
    let mut media_opacity_track = KeyframeTrack::new(
        TrackId(2004),
        media_merge,
        PropertyPath::new("merge.opacity"),
        lumen::AnimatableType::Float,
    );
    media_opacity_track.set_key(0, PropertyValue::Float(0.0), InterpolationMode::Linear);
    media_opacity_track.set_key(239, PropertyValue::Float(1.0), InterpolationMode::Linear);
    composition.add_track(media_opacity_track);
    composition.set_expression(
        media_merge,
        "merge.opacity",
        lumen::Expression::parse(
            "clamp(smoothstep(40, 90, frame) - smoothstep(215, 239, frame) + 0.25, 0.0, 0.92)",
        )?,
    );

    Ok((composition, feature_nodes))
}

fn add_node(
    graph: &mut Graph,
    feature_nodes: &mut Vec<&'static str>,
    feature_name: &'static str,
    kind: NodeKind,
) -> NodeId {
    if !feature_nodes.contains(&feature_name) {
        feature_nodes.push(feature_name);
    }
    graph.add_node(Node::new(NodeId(0), kind))
}

fn connect(
    graph: &mut Graph,
    from_node: NodeId,
    from_port: &str,
    to_node: NodeId,
    to_port: InputPort,
) -> Result<()> {
    graph.connect(Connection {
        from_node,
        from_port: OutputPort::named(from_port),
        to_node,
        to_port,
    })?;
    Ok(())
}

fn procedural_image_rgba(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = vec![0_u8; (width * height * 4) as usize];
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let checker = ((x / 16) + (y / 16)) % 2;
            let r = ((x * 255) / width) as u8;
            let g = ((y * 255) / height) as u8;
            let b = if checker == 0 { 220 } else { 60 };
            bytes[idx..idx + 4].copy_from_slice(&[r, g, b, 220]);
        }
    }
    bytes
}

fn procedural_video_frame(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let mut bytes = vec![0_u8; (width * height * 4) as usize];
    let t = frame as f32 / FPS as f32;
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let xf = x as f32 / width as f32;
            let yf = y as f32 / height as f32;
            let wave = ((xf * 8.0 + t * 3.0).sin() * 0.5 + 0.5) * 255.0;
            let pulse = ((yf * 10.0 - t * 4.0).cos() * 0.5 + 0.5) * 255.0;
            let ring = ((((xf - 0.5).powi(2) + (yf - 0.5).powi(2)).sqrt() * 18.0) - t * 6.0).sin()
                * 0.5
                + 0.5;
            let r = (wave as u8).saturating_add(((frame * 3) % 30) as u8);
            let g = pulse as u8;
            let b = (ring * 255.0) as u8;
            bytes[idx..idx + 4].copy_from_slice(&[r, g, b, 200]);
        }
    }
    bytes
}

fn encode_raw_rgba_to_mp4(
    raw_path: &Path,
    mp4_path: &Path,
    width: u32,
    height: u32,
    fps: u32,
) -> Result<()> {
    let status = Command::new("ffmpeg")
        .args([
            "-y",
            "-f",
            "rawvideo",
            "-pix_fmt",
            "rgba",
            "-s",
            &format!("{width}x{height}"),
            "-r",
            &fps.to_string(),
            "-i",
            raw_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid raw path utf-8"))?,
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "medium",
            "-crf",
            "18",
            "-pix_fmt",
            "yuv420p",
            mp4_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid mp4 path utf-8"))?,
        ])
        .status()
        .context("failed to execute ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg encode failed with status {status}");
    }
    Ok(())
}

fn run_json_delegate_smoke() -> Result<&'static str> {
    let payload = r#"
{
  "schema_revision": "lumen_graph_v1",
  "timeline": { "fps": 30.0, "duration_frames": 2 },
  "render_settings": { "width": 2, "height": 1, "background_color": [0,0,0,0] },
  "graph": {
    "nodes": [
      { "id": 1, "kind": { "type": "solid_color", "color": [255,0,0,255], "width": 2, "height": 1 } },
      { "id": 2, "kind": { "type": "media_output" } }
    ],
    "connections": [
      { "from_node": 1, "from_port": "output", "to_node": 2, "to_port": "source" }
    ]
  }
}
"#;
    let result = Composition::from_json(payload);
    if result.composition.is_some() && result.errors.is_empty() {
        Ok("ok")
    } else {
        bail!("json smoke failed: {:?}", result.errors);
    }
}

fn run_surface_pool_smoke() -> Result<&'static str> {
    let pool = Arc::new(SurfacePool::new());
    let raster = RasterFrame::bitmap(Arc::new(vec![255, 0, 0, 255]), 1, 1);
    let promoted = raster.promote_to_surface(&pool)?;
    let _roundtrip = promoted.to_bitmap()?;
    Ok("ok")
}

fn format_warnings(warnings: &[Warning]) -> String {
    if warnings.is_empty() {
        return "none".to_string();
    }
    warnings
        .iter()
        .map(|warning| match warning {
            Warning::FpsMismatch {
                node_id,
                composition_fps,
                source_fps,
            } => format!(
                "FpsMismatch(node={node_id},composition_fps={composition_fps},source_fps={source_fps})"
            ),
            Warning::CapabilityMissing {
                node_id,
                requirement,
            } => format!("CapabilityMissing(node={node_id},requirement={requirement})"),
        })
        .collect::<Vec<_>>()
        .join("; ")
}

fn workspace_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."))
}

#[allow(dead_code)]
fn bitmap_sink_smoke(composition: &Composition, media_store: Arc<dyn MediaStore>) -> Result<usize> {
    let mut sink = BitmapSink::new();
    let mut ctx = RenderContext::new(
        composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        media_store,
        RuntimeCapabilityProfile {
            has_image_resolver: true,
            has_video_resolver: true,
            has_threading: false,
            sink_types: vec![SinkType::Bitmap],
        },
    );
    let frame = composition.render_frame(0, &mut ctx)?;
    sink.write_frame(0, &frame)?;
    sink.finalize()?;
    Ok(sink.frames().len())
}
