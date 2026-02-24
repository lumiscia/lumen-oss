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

const WIDTH: u32 = 320;
const HEIGHT: u32 = 180;
const FPS: u32 = 30;
const DURATION_FRAMES: u32 = 90;

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
        let RasterFrame::Bitmap(bytes, width, height) = bitmap else {
            return Err(SinkError::WriteFrame {
                frame,
                details: "expected bitmap frame".to_string(),
            });
        };
        if width != self.width || height != self.height {
            return Err(SinkError::WriteFrame {
                frame,
                details: format!(
                    "unexpected frame dimensions {width}x{height}, expected {}x{}",
                    self.width, self.height
                ),
            });
        }

        self.writer
            .write_all(bytes.as_slice())
            .map_err(|error| SinkError::WriteFrame {
                frame,
                details: error.to_string(),
            })?;

        if let Ok(mut stats) = self.shared.lock() {
            stats.frames_written += 1;
            stats.bytes_written = stats
                .bytes_written
                .saturating_add(u64::try_from(bytes.len()).unwrap_or(0));
            stats.first_frame.get_or_insert(frame);
            stats.last_frame = Some(frame);
            stats.frame_checksum = stats
                .frame_checksum
                .wrapping_add(bytes.iter().map(|byte| u64::from(*byte)).sum::<u64>());
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
        .join("lumen-all-features-demo");
    fs::create_dir_all(&artifact_dir)
        .with_context(|| format!("failed to create {}", artifact_dir.display()))?;

    let raw_warmup = artifact_dir.join("warmup_pass.rgba");
    let raw_final = artifact_dir.join("final_pass.rgba");
    let video_path = artifact_dir.join("lumen_all_features_demo.mp4");
    let stats_path = artifact_dir.join("stats.txt");

    let image_stats = Arc::new(Mutex::new(ImageResolveStats::default()));
    let video_stats = Arc::new(Mutex::new(VideoResolveStats::default()));
    let media_store: Arc<dyn MediaStore> = Arc::new(DemoMediaStore {
        image: DemoImageResolver::new(
            "demo-image",
            128,
            128,
            procedural_image_rgba(128, 128),
            Arc::clone(&image_stats),
        ),
        video: DemoVideoResolver::new("demo-video", WIDTH, HEIGHT, 180, Arc::clone(&video_stats)),
    });

    let (mut composition, feature_nodes) = build_feature_composition()?;
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
    writeln!(&mut stats_report, "lumen all-features demo")?;
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
            color: [12, 18, 28, 255],
            width: Some(WIDTH),
            height: Some(HEIGHT),
        }),
    );

    let shape_ellipse = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shape",
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Ellipse {
                width: 180,
                height: 120,
            },
        }),
    );
    let ellipse_renderer = add_node(
        &mut graph,
        &mut feature_nodes,
        "ShapeRenderer",
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [255, 255, 255, 210],
            stroke_enabled: true,
            stroke_width: 2.0,
            stroke_color: [255, 255, 255, 255],
            ..ShapeRenderer::default()
        }),
    );
    connect(
        &mut graph,
        shape_ellipse,
        "vector",
        ellipse_renderer,
        InputPort::named("vector"),
    )?;
    let ellipse_mask_full = add_node(
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
        ellipse_renderer,
        "output",
        ellipse_mask_full,
        InputPort::named("source"),
    )?;

    let shape_polygon = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shape",
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Polygon {
                points: vec![(20.0, 10.0), (170.0, 25.0), (140.0, 110.0), (30.0, 95.0)],
            },
        }),
    );
    let polygon_renderer = add_node(
        &mut graph,
        &mut feature_nodes,
        "ShapeRenderer",
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [40, 180, 170, 255],
            stroke_enabled: true,
            stroke_width: 3.0,
            stroke_color: [220, 255, 250, 255],
            ..ShapeRenderer::default()
        }),
    );
    connect(
        &mut graph,
        shape_polygon,
        "vector",
        polygon_renderer,
        InputPort::named("vector"),
    )?;
    let polygon_full = add_node(
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
        polygon_renderer,
        "output",
        polygon_full,
        InputPort::named("source"),
    )?;

    let shape_rect = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shape",
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 96,
                height: 48,
            },
        }),
    );
    let rect_renderer = add_node(
        &mut graph,
        &mut feature_nodes,
        "ShapeRenderer",
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [242, 96, 53, 220],
            stroke_enabled: true,
            stroke_width: 2.0,
            stroke_color: [255, 225, 214, 255],
            ..ShapeRenderer::default()
        }),
    );
    connect(
        &mut graph,
        shape_rect,
        "vector",
        rect_renderer,
        InputPort::named("vector"),
    )?;
    let rect_full = add_node(
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
        rect_renderer,
        "output",
        rect_full,
        InputPort::named("source"),
    )?;

    let text_node = add_node(
        &mut graph,
        &mut feature_nodes,
        "Text",
        NodeKind::Text(Text {
            content: "LUMEN NEXT\nall nodes + animation + expressions + memo + threading"
                .to_string(),
            font_family: "Helvetica".to_string(),
            font_size: 18.0,
            font_weight: 700,
            font_style: TextFontStyle::Italic,
            max_width: Some(WIDTH as f32),
            color: [250, 250, 245, 255],
            alignment: TextAlignment {
                horizontal: TextAlignmentHorizontal::Center,
                vertical: TextAlignmentVertical::Middle,
            },
        }),
    );
    let text_full = add_node(
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
        text_node,
        "output",
        text_full,
        InputPort::named("source"),
    )?;

    let static_merge_a = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Screen,
            opacity: 0.92,
        }),
    );
    connect(
        &mut graph,
        polygon_full,
        "output",
        static_merge_a,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        text_full,
        "output",
        static_merge_a,
        InputPort::named("overlay"),
    )?;
    connect(
        &mut graph,
        ellipse_mask_full,
        "output",
        static_merge_a,
        InputPort::named("mask"),
    )?;

    let static_merge_b = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Overlay,
            opacity: 0.75,
        }),
    );
    connect(
        &mut graph,
        static_merge_a,
        "output",
        static_merge_b,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        rect_full,
        "output",
        static_merge_b,
        InputPort::named("overlay"),
    )?;

    let static_shadow = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shadow",
        NodeKind::Shadow(Shadow {
            offset_x: 8,
            offset_y: 6,
            color: [0, 0, 0, 140],
        }),
    );
    connect(
        &mut graph,
        static_merge_b,
        "output",
        static_shadow,
        InputPort::named("source"),
    )?;

    let static_blur = add_node(
        &mut graph,
        &mut feature_nodes,
        "Blur",
        NodeKind::Blur(Blur { radius: 1.5 }),
    );
    connect(
        &mut graph,
        static_shadow,
        "output",
        static_blur,
        InputPort::named("source"),
    )?;

    let static_memo = add_node(
        &mut graph,
        &mut feature_nodes,
        "Memo",
        NodeKind::Memo(Memo {
            cache_id: "feature-demo-static-layer".to_string(),
            allow_expressions: false,
        }),
    );
    connect(
        &mut graph,
        static_blur,
        "output",
        static_memo,
        InputPort::named("source"),
    )?;

    let static_transform = add_node(
        &mut graph,
        &mut feature_nodes,
        "Transform",
        NodeKind::Transform(Transform::default()),
    );
    connect(
        &mut graph,
        static_memo,
        "output",
        static_transform,
        InputPort::named("source"),
    )?;

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
            x: 12,
            y: 8,
            width: 100,
            height: 100,
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
            mode: ResizeMode::Fill,
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
        ellipse_mask_full,
        "output",
        image_boolean,
        InputPort::named("mask"),
    )?;
    connect(
        &mut graph,
        shape_rect,
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

    let video_in = add_node(
        &mut graph,
        &mut feature_nodes,
        "MediaIn",
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "demo-video".to_string(),
                range: Some(5..95),
                speed: 1.5,
                loop_mode: LoopMode::Repeat,
            },
        }),
    );

    let video_hold = add_node(
        &mut graph,
        &mut feature_nodes,
        "FrameHold",
        NodeKind::FrameHold(FrameHold { hold_frame: 12 }),
    );
    connect(
        &mut graph,
        video_in,
        "output",
        video_hold,
        InputPort::named("source"),
    )?;

    let mut switch_map = HashMap::new();
    switch_map.insert(0, 0..30);
    switch_map.insert(1, 30..60);
    switch_map.insert(2, 60..90);
    let switch_node = add_node(
        &mut graph,
        &mut feature_nodes,
        "Switch",
        NodeKind::Switch(Switch::new(switch_map)),
    );
    connect(
        &mut graph,
        video_in,
        "output",
        switch_node,
        InputPort::Indexed(0),
    )?;
    connect(
        &mut graph,
        video_hold,
        "output",
        switch_node,
        InputPort::Indexed(1),
    )?;
    connect(
        &mut graph,
        image_transform,
        "output",
        switch_node,
        InputPort::Indexed(2),
    )?;

    let switch_shadow = add_node(
        &mut graph,
        &mut feature_nodes,
        "Shadow",
        NodeKind::Shadow(Shadow {
            offset_x: -5,
            offset_y: 4,
            color: [0, 0, 0, 100],
        }),
    );
    connect(
        &mut graph,
        switch_node,
        "output",
        switch_shadow,
        InputPort::named("source"),
    )?;

    let merge_1 = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
        }),
    );
    connect(&mut graph, bg, "output", merge_1, InputPort::named("base"))?;
    connect(
        &mut graph,
        static_transform,
        "output",
        merge_1,
        InputPort::named("overlay"),
    )?;
    connect(
        &mut graph,
        ellipse_mask_full,
        "output",
        merge_1,
        InputPort::named("mask"),
    )?;

    let merge_2 = add_node(
        &mut graph,
        &mut feature_nodes,
        "Merge",
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Lighten,
            opacity: 0.8,
        }),
    );
    connect(
        &mut graph,
        merge_1,
        "output",
        merge_2,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        switch_shadow,
        "output",
        merge_2,
        InputPort::named("overlay"),
    )?;
    connect(
        &mut graph,
        rect_full,
        "output",
        merge_2,
        InputPort::named("mask"),
    )?;

    let merge_noop = add_node(
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
        merge_2,
        "output",
        merge_noop,
        InputPort::named("base"),
    )?;
    connect(
        &mut graph,
        text_full,
        "output",
        merge_noop,
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
        merge_noop,
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

    // Keyframe animation: move the masked image branch across the screen.
    let mut image_track = KeyframeTrack::new(
        TrackId(1001),
        image_transform,
        PropertyPath::new("transform.translate_x"),
        lumen::AnimatableType::Float,
    );
    image_track.before_extrapolation = Extrapolation::Hold;
    image_track.after_extrapolation = Extrapolation::Hold;
    image_track.set_key(0, PropertyValue::Float(-40.0), InterpolationMode::Linear);
    image_track.set_key(45, PropertyValue::Float(22.0), InterpolationMode::Linear);
    image_track.set_key(89, PropertyValue::Float(-12.0), InterpolationMode::Linear);
    composition.add_track(image_track);

    let mut image_track_y = KeyframeTrack::new(
        TrackId(1002),
        image_transform,
        PropertyPath::new("transform.translate_y"),
        lumen::AnimatableType::Float,
    );
    image_track_y.set_key(0, PropertyValue::Float(-8.0), InterpolationMode::Step);
    image_track_y.set_key(30, PropertyValue::Float(0.0), InterpolationMode::Step);
    image_track_y.set_key(60, PropertyValue::Float(8.0), InterpolationMode::Step);
    composition.add_track(image_track_y);

    // Expression-driven transform using globals + math + text metric builtins.
    composition.set_expression(
        static_transform,
        "transform.rotate",
        lumen::Expression::parse(
            "sin(time * 2.0) * 8 + clamp(text_width('lumen', 20) / 20, 0, 5)",
        )?,
    );
    composition.set_expression(
        static_transform,
        "transform.translate_y",
        lumen::Expression::parse("sin(time * 3.0) * 6")?,
    );

    // Expression precedence over keyframe on merge opacity (keyframe exists but expression wins).
    let mut merge_opacity_track = KeyframeTrack::new(
        TrackId(1003),
        merge_2,
        PropertyPath::new("merge.opacity"),
        lumen::AnimatableType::Float,
    );
    merge_opacity_track.set_key(0, PropertyValue::Float(0.2), InterpolationMode::Linear);
    merge_opacity_track.set_key(89, PropertyValue::Float(1.0), InterpolationMode::Linear);
    composition.add_track(merge_opacity_track);
    composition.set_expression(
        merge_2,
        "merge.opacity",
        lumen::Expression::parse("clamp((sin(time * 2.4) + 1) / 2, 0.25, 0.95)")?,
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
    let raster = RasterFrame::Bitmap(Arc::new(vec![255, 0, 0, 255]), 1, 1);
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
