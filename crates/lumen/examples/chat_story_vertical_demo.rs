use std::{
    cell::RefCell,
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
    media::{ImageResolver, MediaStore, VideoFrameResolver},
    node::{
        Node, PropertyValue,
        crop::Crop,
        frame_hold::FrameHold,
        media_in::{LoopMode, MediaIn, MediaInKind},
        media_output::MediaOutput,
        memo::Memo,
        merge::Merge,
        resize::{Resize, ResizeMode, ResizeSampling},
        shadow::Shadow,
        solid_color::SolidColor,
        transform::{Transform, TransformSampling},
    },
};
use skia_safe::{
    AlphaType, Color, ColorType, FontMgr, FontStyle, ImageInfo, Paint, PaintStyle, RRect, Rect,
    Surface,
    font_style::Weight,
    surfaces,
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign as ParagraphTextAlign,
        TextStyle as ParagraphTextStyle,
    },
};

const WIDTH: u32 = 1080;
const HEIGHT: u32 = 1920;
const FPS: u32 = 30;
const DURATION_FRAMES: u32 = 150; // 5 seconds

struct TextLayoutCache {
    font_mgr: FontMgr,
    font_collection: FontCollection,
}

impl TextLayoutCache {
    fn new() -> Self {
        let font_mgr = FontMgr::default();
        let mut font_collection = FontCollection::new();
        font_collection.set_default_font_manager(font_mgr.clone(), None);
        Self {
            font_mgr,
            font_collection,
        }
    }
}

thread_local! {
    static TEXT_LAYOUT_CACHE: RefCell<Option<TextLayoutCache>> = const { RefCell::new(None) };
}

fn with_text_layout_cache<R>(f: impl FnOnce(&TextLayoutCache) -> R) -> R {
    TEXT_LAYOUT_CACHE.with(|cell| {
        let mut borrow = cell.borrow_mut();
        let cache = borrow.get_or_insert_with(TextLayoutCache::new);
        f(cache)
    })
}

#[derive(Default)]
struct CallStats {
    calls: usize,
    frames: Vec<u32>,
}

#[derive(Clone)]
struct StaticImageResolver {
    id: String,
    width: u32,
    height: u32,
    pixels: Arc<Vec<u8>>,
    stats: Arc<Mutex<CallStats>>,
}

impl ImageResolver for StaticImageResolver {
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
struct ProceduralVideoResolver {
    id: String,
    width: u32,
    height: u32,
    frame_count: u32,
    renderer: Arc<dyn Fn(u32, u32, u32) -> Vec<u8> + Send + Sync>,
    stats: Arc<Mutex<CallStats>>,
}

impl VideoFrameResolver for ProceduralVideoResolver {
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
            stats.frames.push(frame);
        }
        Ok(Arc::new((self.renderer)(self.width, self.height, frame)))
    }
}

struct StoryMediaStore {
    chrome: StaticImageResolver,
    bg: ProceduralVideoResolver,
    chat: ProceduralVideoResolver,
}

impl MediaStore for StoryMediaStore {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        (source == self.chrome.id())
            .then(|| Box::new(self.chrome.clone()) as Box<dyn ImageResolver>)
    }

    fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
        if source == self.bg.id() {
            Some(Box::new(self.bg.clone()) as Box<dyn VideoFrameResolver>)
        } else if source == self.chat.id() {
            Some(Box::new(self.chat.clone()) as Box<dyn VideoFrameResolver>)
        } else {
            None
        }
    }
}

#[derive(Default, Clone)]
struct RawSinkStats {
    frames_written: usize,
    bytes_written: u64,
}

struct RawRgbaSink {
    writer: BufWriter<File>,
    width: u32,
    height: u32,
    stats: Arc<Mutex<RawSinkStats>>,
}

impl RawRgbaSink {
    fn create(
        path: &Path,
        width: u32,
        height: u32,
        stats: Arc<Mutex<RawSinkStats>>,
    ) -> Result<Self> {
        Ok(Self {
            writer: BufWriter::new(File::create(path)?),
            width,
            height,
            stats,
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
                details: "expected bitmap".to_string(),
            });
        };
        if bitmap.storage_width != self.width || bitmap.storage_height != self.height {
            return Err(SinkError::WriteFrame {
                frame,
                details: format!(
                    "unexpected dimensions {}x{}, expected {}x{}",
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
        if let Ok(mut stats) = self.stats.lock() {
            stats.frames_written += 1;
            stats.bytes_written = stats
                .bytes_written
                .saturating_add(bitmap.pixels.len() as u64);
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
        .join("lumen-chat-story-vertical");
    fs::create_dir_all(&artifact_dir)?;

    let raw_path = artifact_dir.join("chat_story_vertical.rgba");
    let video_path = artifact_dir.join("chat_story_vertical.mp4");
    let stats_path = artifact_dir.join("stats.txt");

    let chrome_stats = Arc::new(Mutex::new(CallStats::default()));
    let bg_stats = Arc::new(Mutex::new(CallStats::default()));
    let chat_stats = Arc::new(Mutex::new(CallStats::default()));

    let media_store: Arc<dyn MediaStore> = Arc::new(StoryMediaStore {
        chrome: StaticImageResolver {
            id: "ui-chrome".to_string(),
            width: WIDTH,
            height: HEIGHT,
            pixels: Arc::new(render_ui_chrome(WIDTH, HEIGHT)?),
            stats: Arc::clone(&chrome_stats),
        },
        bg: ProceduralVideoResolver {
            id: "bg-video".to_string(),
            width: WIDTH,
            height: HEIGHT,
            frame_count: DURATION_FRAMES + 60,
            renderer: Arc::new(render_background_frame),
            stats: Arc::clone(&bg_stats),
        },
        chat: ProceduralVideoResolver {
            id: "chat-video".to_string(),
            width: WIDTH,
            height: HEIGHT,
            frame_count: DURATION_FRAMES + 30,
            renderer: Arc::new(render_chat_overlay_frame),
            stats: Arc::clone(&chat_stats),
        },
    });

    let composition = build_chat_story_composition()?;
    let profile = RuntimeCapabilityProfile {
        has_image_resolver: true,
        has_video_resolver: true,
        has_threading: true,
        sink_types: vec![SinkType::Bitmap, SinkType::Video],
    };

    let validation_started = Instant::now();
    let warnings = composition
        .validate(&profile)
        .map_err(|errors| anyhow!("validation failed: {errors:?}"))?;
    let validation_elapsed = validation_started.elapsed();

    let surface_pool = Arc::new(SurfacePool::new());
    let asset_cache = Arc::new(RwLock::new(AssetCache::new()));

    let preview_started = Instant::now();
    let mut preview_ctx = RenderContext::new(
        &composition,
        Arc::clone(&surface_pool),
        Arc::clone(&asset_cache),
        Arc::clone(&media_store),
        profile.clone(),
    );
    let preview = composition.render_frame(0, &mut preview_ctx)?;
    let preview_dims = preview.dimensions();
    let preview_elapsed = preview_started.elapsed();

    let render_stats = Arc::new(Mutex::new(RawSinkStats::default()));
    let ctx = RenderContext::new(
        &composition,
        Arc::clone(&surface_pool),
        Arc::clone(&asset_cache),
        Arc::clone(&media_store),
        profile,
    );

    let render_started = Instant::now();
    composition.render_sequence(
        0..DURATION_FRAMES,
        ctx,
        Box::new(RawRgbaSink::create(
            &raw_path,
            WIDTH,
            HEIGHT,
            Arc::clone(&render_stats),
        )?),
        4,
    )?;
    let render_elapsed = render_started.elapsed();

    let encode_started = Instant::now();
    encode_raw_rgba_to_mp4(&raw_path, &video_path, WIDTH, HEIGHT, FPS)?;
    let encode_elapsed = encode_started.elapsed();
    let _ = fs::remove_file(&raw_path);

    let total_elapsed = started.elapsed();
    let video_meta = fs::metadata(&video_path)?;
    let render_fps = DURATION_FRAMES as f64 / render_elapsed.as_secs_f64().max(1e-9);
    let render_stats = render_stats.lock().map(|s| s.clone()).unwrap_or_default();
    let chrome_stats = chrome_stats
        .lock()
        .map(|s| (s.calls, s.frames.clone()))
        .unwrap_or_default();
    let bg_stats = bg_stats
        .lock()
        .map(|s| (s.calls, s.frames.clone()))
        .unwrap_or_default();
    let chat_stats = chat_stats
        .lock()
        .map(|s| (s.calls, s.frames.clone()))
        .unwrap_or_default();

    let mut report = String::new();
    use std::fmt::Write as _;
    writeln!(&mut report, "lumen vertical chat story demo")?;
    writeln!(&mut report, "video={}", video_path.display())?;
    writeln!(&mut report, "video_size_bytes={}", video_meta.len())?;
    writeln!(
        &mut report,
        "timeline={}x{} @ {}fps, frames={}",
        WIDTH, HEIGHT, FPS, DURATION_FRAMES
    )?;
    writeln!(
        &mut report,
        "preview_frame_dims={}x{}",
        preview_dims.0, preview_dims.1
    )?;
    writeln!(
        &mut report,
        "validation_ms={:.2}",
        validation_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut report,
        "preview_render_ms={:.2}",
        preview_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut report,
        "threaded_render_ms={:.2}",
        render_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(&mut report, "threaded_render_fps={:.2}", render_fps)?;
    writeln!(
        &mut report,
        "encode_ms={:.2}",
        encode_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut report,
        "total_ms={:.2}",
        total_elapsed.as_secs_f64() * 1000.0
    )?;
    writeln!(
        &mut report,
        "sink_frames={},sink_bytes={}",
        render_stats.frames_written, render_stats.bytes_written
    )?;
    writeln!(&mut report, "chrome_image_resolve_calls={}", chrome_stats.0)?;
    writeln!(
        &mut report,
        "bg_video_resolve_calls={},bg_min_frame={:?},bg_max_frame={:?}",
        bg_stats.0,
        bg_stats.1.iter().min(),
        bg_stats.1.iter().max()
    )?;
    writeln!(
        &mut report,
        "chat_video_resolve_calls={},chat_min_frame={:?},chat_max_frame={:?}",
        chat_stats.0,
        chat_stats.1.iter().min(),
        chat_stats.1.iter().max()
    )?;
    writeln!(&mut report, "warnings_count={}", warnings.len())?;
    writeln!(&mut report, "warnings={}", format_warnings(&warnings))?;

    fs::write(&stats_path, &report)?;
    println!("{report}");
    Ok(())
}

fn build_chat_story_composition() -> Result<Composition> {
    let mut graph = Graph::new();

    let bg_video = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "bg-video".to_string(),
                range: Some(0..DURATION_FRAMES),
                speed: 1.0,
                loop_mode: LoopMode::Repeat,
            },
        }),
    ));
    let bg_crop = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Crop(Crop {
            x: 0,
            y: 0,
            width: WIDTH,
            height: HEIGHT,
        }),
    ));
    let bg_resize = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Resize(Resize {
            width: WIDTH,
            height: HEIGHT,
            mode: ResizeMode::Fill,
            sampling: ResizeSampling::Linear,
        }),
    ));
    let bg_transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform {
            sampling: TransformSampling::Nearest,
            ..Transform::default()
        }),
    ));

    let dim_overlay = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 0, 0, 135],
            width: Some(WIDTH),
            height: Some(HEIGHT),
        }),
    ));
    let base_merge = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
        }),
    ));

    let chrome_image = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Image {
                source: "ui-chrome".to_string(),
            },
        }),
    ));
    let chrome_memo = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Memo(Memo {
            cache_id: "chat-story-vertical-chrome".to_string(),
            allow_expressions: false,
        }),
    ));

    let chat_video = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "chat-video".to_string(),
                range: Some(0..DURATION_FRAMES),
                speed: 1.0,
                loop_mode: LoopMode::Repeat,
            },
        }),
    ));
    let chat_hold = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::FrameHold(FrameHold { hold_frame: 124 }),
    ));
    let mut switch_map = std::collections::HashMap::new();
    switch_map.insert(0, 0..135);
    switch_map.insert(1, 135..DURATION_FRAMES);
    let chat_switch = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Switch(lumen::node::switch::Switch::new(switch_map)),
    ));
    let chat_shadow = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Shadow(Shadow {
            offset_x: 0,
            offset_y: 10,
            blur_radius: 0.0,
            color: [0, 0, 0, 70],
        }),
    ));
    let chat_transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform {
            sampling: TransformSampling::Nearest,
            ..Transform::default()
        }),
    ));

    let chrome_merge = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
        }),
    ));
    let final_merge = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Merge(Merge {
            blend_mode: BlendMode::Normal,
            opacity: 1.0,
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, bg_video, "output", bg_crop, "source")?;
    connect(&mut graph, bg_crop, "output", bg_resize, "source")?;
    connect(&mut graph, bg_resize, "output", bg_transform, "source")?;

    connect(&mut graph, bg_transform, "output", base_merge, "base")?;
    connect(&mut graph, dim_overlay, "output", base_merge, "overlay")?;

    connect(&mut graph, chrome_image, "output", chrome_memo, "source")?;
    connect(&mut graph, base_merge, "output", chrome_merge, "base")?;
    connect(&mut graph, chrome_memo, "output", chrome_merge, "overlay")?;

    connect(&mut graph, chat_video, "output", chat_hold, "source")?;
    graph.connect(Connection {
        from_node: chat_video,
        from_port: OutputPort::named("output"),
        to_node: chat_switch,
        to_port: InputPort::Indexed(0),
    })?;
    graph.connect(Connection {
        from_node: chat_hold,
        from_port: OutputPort::named("output"),
        to_node: chat_switch,
        to_port: InputPort::Indexed(1),
    })?;
    connect(&mut graph, chat_switch, "output", chat_shadow, "source")?;
    connect(&mut graph, chat_shadow, "output", chat_transform, "source")?;

    connect(&mut graph, chrome_merge, "output", final_merge, "base")?;
    connect(&mut graph, chat_transform, "output", final_merge, "overlay")?;
    connect(&mut graph, final_merge, "output", output, "source")?;

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

    // Background slow zoom/pan with keyframes + expressions.
    let mut sx = KeyframeTrack::new(
        TrackId(3001),
        bg_transform,
        PropertyPath::new("transform.scale_x"),
        lumen::AnimatableType::Float,
    );
    sx.before_extrapolation = Extrapolation::Hold;
    sx.after_extrapolation = Extrapolation::Hold;
    sx.set_key(0, PropertyValue::Float(1.0), InterpolationMode::Linear);
    sx.set_key(149, PropertyValue::Float(1.06), InterpolationMode::Linear);
    composition.add_track(sx);

    let mut sy = KeyframeTrack::new(
        TrackId(3002),
        bg_transform,
        PropertyPath::new("transform.scale_y"),
        lumen::AnimatableType::Float,
    );
    sy.set_key(0, PropertyValue::Float(1.0), InterpolationMode::Linear);
    sy.set_key(149, PropertyValue::Float(1.06), InterpolationMode::Linear);
    composition.add_track(sy);

    composition.set_expression(
        bg_transform,
        "transform.translate_y",
        lumen::Expression::parse("sin(time * 0.8) * 14")?,
    );
    composition.set_expression(
        chat_transform,
        "transform.translate_y",
        lumen::Expression::parse("sin(time * 2.4) * 3")?,
    );
    composition.set_expression(
        final_merge,
        "merge.opacity",
        lumen::Expression::parse("clamp(smoothstep(0, 8, frame), 0, 1)")?,
    );

    Ok(composition)
}

fn connect(
    graph: &mut Graph,
    from_node: NodeId,
    from_port: &str,
    to_node: NodeId,
    to_port: &str,
) -> Result<()> {
    graph.connect(Connection {
        from_node,
        from_port: OutputPort::named(from_port),
        to_node,
        to_port: InputPort::named(to_port),
    })?;
    Ok(())
}

fn render_background_frame(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let mut bytes = vec![0_u8; (width * height * 4) as usize];
    let t = frame as f32 / FPS as f32;
    for y in 0..height {
        for x in 0..width {
            let idx = ((y * width + x) * 4) as usize;
            let xf = x as f32 / width as f32;
            let yf = y as f32 / height as f32;
            let band_a = ((yf * 9.0 + t * 1.8).sin() * 0.5 + 0.5).powf(1.4);
            let band_b = ((xf * 6.0 - t * 1.2).cos() * 0.5 + 0.5).powf(1.6);
            let glow = ((((xf - 0.55).powi(2) + (yf - 0.35).powi(2)).sqrt() * 9.0) - t * 0.8).sin()
                * 0.5
                + 0.5;
            let r = (45.0 + 120.0 * band_b + 80.0 * glow).clamp(0.0, 255.0) as u8;
            let g = (18.0 + 70.0 * band_a + 30.0 * glow).clamp(0.0, 255.0) as u8;
            let b = (58.0 + 160.0 * band_a + 25.0 * band_b).clamp(0.0, 255.0) as u8;
            bytes[idx..idx + 4].copy_from_slice(&[r, g, b, 255]);
        }
    }
    bytes
}

fn render_chat_overlay_frame(width: u32, height: u32, frame: u32) -> Vec<u8> {
    let Some(mut surface) = new_surface(width, height) else {
        let byte_len = (width as usize)
            .checked_mul(height as usize)
            .and_then(|px| px.checked_mul(4))
            .unwrap_or(4);
        return vec![0_u8; byte_len];
    };
    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);

    let t = frame as f32 / FPS as f32;
    let progress = frame as f32 / (DURATION_FRAMES - 1) as f32;

    // subtle glass panel behind chat
    draw_round_rect(
        canvas,
        Rect::from_xywh(44.0, 180.0, width as f32 - 88.0, height as f32 - 280.0),
        48.0,
        [255, 255, 255, 16],
    );
    draw_round_rect(
        canvas,
        Rect::from_xywh(44.0, 180.0, width as f32 - 88.0, height as f32 - 280.0),
        48.0,
        [255, 255, 255, 8],
    );

    let messages = chat_timeline_messages();
    for (index, message) in messages.iter().enumerate() {
        let appear = ((frame as i32 - message.appear_frame as i32) as f32 / 8.0).clamp(0.0, 1.0);
        if appear <= 0.0 {
            continue;
        }
        let bubble_alpha = ease_out(appear);
        let slide = (1.0 - bubble_alpha) * if message.outgoing { 70.0 } else { -70.0 };

        let base_y = message.y;
        let x = if message.outgoing {
            width as f32 - 56.0 - message.width
        } else {
            56.0
        };
        let y = base_y + (1.0 - bubble_alpha) * 10.0 + (t * 1.1 + index as f32 * 0.35).sin() * 1.5;
        let bubble_rect = Rect::from_xywh(x + slide, y, message.width, message.height);

        let mut color = if message.outgoing {
            [65, 168, 255, 255]
        } else {
            [245, 247, 252, 255]
        };
        color[3] = ((color[3] as f32) * bubble_alpha) as u8;
        let text_color = if message.outgoing {
            [255, 255, 255, (255.0 * bubble_alpha) as u8]
        } else {
            [18, 24, 35, (255.0 * bubble_alpha) as u8]
        };
        let shadow_alpha = (40.0 * bubble_alpha) as u8;
        draw_round_rect(
            canvas,
            Rect::from_xywh(
                bubble_rect.left + 0.0,
                bubble_rect.top + 6.0,
                bubble_rect.width(),
                bubble_rect.height(),
            ),
            28.0,
            [0, 0, 0, shadow_alpha],
        );
        draw_round_rect(canvas, bubble_rect, 28.0, color);
        if message.outgoing {
            draw_round_rect(
                canvas,
                Rect::from_xywh(
                    bubble_rect.right - 20.0,
                    bubble_rect.bottom - 12.0,
                    12.0,
                    12.0,
                ),
                4.0,
                color,
            );
        } else {
            draw_round_rect(
                canvas,
                Rect::from_xywh(
                    bubble_rect.left + 8.0,
                    bubble_rect.bottom - 12.0,
                    12.0,
                    12.0,
                ),
                4.0,
                color,
            );
        }

        let _ = draw_text_paragraph(
            canvas,
            &message.text,
            bubble_rect.left + 22.0,
            bubble_rect.top + 18.0,
            bubble_rect.width() - 44.0,
            34.0,
            if message.outgoing { 600 } else { 500 },
            text_color,
            ParagraphTextAlign::Left,
        );
    }

    // typing indicator in the final phase
    let typing_phase = ((progress - 0.76) / 0.12).clamp(0.0, 1.0);
    if typing_phase > 0.0 {
        let y = height as f32 - 360.0;
        let alpha = (typing_phase * 255.0) as u8;
        draw_round_rect(
            canvas,
            Rect::from_xywh(56.0, y, 250.0, 78.0),
            30.0,
            [245, 247, 252, alpha],
        );
        for i in 0..3 {
            let pulse = (((t * 3.5) + i as f32 * 0.45).sin() * 0.5 + 0.5) * 180.0 + 60.0;
            let cx = 112.0 + i as f32 * 40.0;
            draw_circle(
                canvas,
                cx,
                y + 39.0,
                8.0,
                [120, 132, 150, ((pulse / 255.0) * alpha as f32) as u8],
            );
        }
    }

    read_surface_rgba(&mut surface, width, height)
}

fn render_ui_chrome(width: u32, height: u32) -> Result<Vec<u8>> {
    let mut surface =
        new_surface(width, height).ok_or_else(|| anyhow!("failed to create chrome surface"))?;
    let canvas = surface.canvas();
    canvas.clear(Color::TRANSPARENT);

    // top progress bars
    let bars = 6;
    let gap = 12.0;
    let total_w = width as f32 - 56.0;
    let bar_w = (total_w - gap * (bars as f32 - 1.0)) / bars as f32;
    for i in 0..bars {
        let x = 28.0 + i as f32 * (bar_w + gap);
        draw_round_rect(
            canvas,
            Rect::from_xywh(x, 52.0, bar_w, 8.0),
            4.0,
            [255, 255, 255, 60],
        );
    }

    // profile row
    draw_circle(canvas, 72.0, 116.0, 28.0, [255, 122, 82, 255]);
    draw_circle(canvas, 72.0, 116.0, 23.0, [255, 209, 191, 255]);
    let _ = draw_text_paragraph(
        canvas,
        "storytime.daily",
        114.0,
        88.0,
        420.0,
        34.0,
        700,
        [255, 255, 255, 255],
        ParagraphTextAlign::Left,
    );
    let _ = draw_text_paragraph(
        canvas,
        "5m ago",
        114.0,
        124.0,
        220.0,
        28.0,
        500,
        [240, 240, 240, 210],
        ParagraphTextAlign::Left,
    );

    // top-right controls
    draw_circle(
        canvas,
        width as f32 - 84.0,
        116.0,
        20.0,
        [255, 255, 255, 35],
    );
    draw_circle(
        canvas,
        width as f32 - 36.0,
        116.0,
        20.0,
        [255, 255, 255, 35],
    );

    // bottom caption pill
    draw_round_rect(
        canvas,
        Rect::from_xywh(46.0, height as f32 - 170.0, width as f32 - 92.0, 84.0),
        34.0,
        [18, 22, 30, 180],
    );
    let _ = draw_text_paragraph(
        canvas,
        "POV: the group chat finds out who leaked the screenshots",
        78.0,
        height as f32 - 145.0,
        width as f32 - 156.0,
        32.0,
        600,
        [255, 255, 255, 235],
        ParagraphTextAlign::Center,
    );

    Ok(read_surface_rgba(&mut surface, width, height))
}

struct ChatMessageSpec {
    appear_frame: u32,
    y: f32,
    width: f32,
    height: f32,
    outgoing: bool,
    text: &'static str,
}

fn chat_timeline_messages() -> Vec<ChatMessageSpec> {
    vec![
        ChatMessageSpec {
            appear_frame: 6,
            y: 260.0,
            width: 660.0,
            height: 128.0,
            outgoing: false,
            text: "wait... did you send that to HIM\nor the group chat??",
        },
        ChatMessageSpec {
            appear_frame: 24,
            y: 430.0,
            width: 560.0,
            height: 104.0,
            outgoing: true,
            text: "no no NO delete this rn",
        },
        ChatMessageSpec {
            appear_frame: 43,
            y: 580.0,
            width: 720.0,
            height: 154.0,
            outgoing: false,
            text: "too late 😭 kayla already replied\n\"interesting screenshot\"",
        },
        ChatMessageSpec {
            appear_frame: 68,
            y: 775.0,
            width: 520.0,
            height: 104.0,
            outgoing: true,
            text: "i'm actually moving",
        },
        ChatMessageSpec {
            appear_frame: 95,
            y: 930.0,
            width: 760.0,
            height: 178.0,
            outgoing: false,
            text: "SHE JUST POSTED \"trust is fragile\"\nand now everyone thinks it's about you",
        },
        ChatMessageSpec {
            appear_frame: 122,
            y: 1145.0,
            width: 620.0,
            height: 126.0,
            outgoing: true,
            text: "tell me exactly what she said",
        },
    ]
}

fn ease_out(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    1.0 - (1.0 - t) * (1.0 - t)
}

fn draw_circle(canvas: &skia_safe::Canvas, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    paint.set_color(Color::from_argb(color[3], color[0], color[1], color[2]));
    canvas.draw_circle((cx, cy), radius, &paint);
}

fn draw_round_rect(canvas: &skia_safe::Canvas, rect: Rect, radius: f32, color: [u8; 4]) {
    let mut paint = Paint::default();
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    paint.set_color(Color::from_argb(color[3], color[0], color[1], color[2]));
    let rrect = RRect::new_rect_xy(rect, radius, radius);
    canvas.draw_rrect(rrect, &paint);
}

fn draw_text_paragraph(
    canvas: &skia_safe::Canvas,
    text: &str,
    x: f32,
    y: f32,
    width: f32,
    font_size: f32,
    font_weight: u16,
    color: [u8; 4],
    align: ParagraphTextAlign,
) -> f32 {
    let mut paragraph_style = ParagraphStyle::new();
    paragraph_style.set_text_align(align);

    let mut text_style = ParagraphTextStyle::new();
    text_style.set_font_size(font_size.max(1.0));
    text_style.set_color(Color::from_argb(color[3], color[0], color[1], color[2]));
    text_style.set_font_style(FontStyle::new(
        Weight::from(i32::from(font_weight.clamp(100, 900))),
        skia_safe::font_style::Width::NORMAL,
        skia_safe::font_style::Slant::Upright,
    ));
    text_style.set_font_families(&["Helvetica"]);
    paragraph_style.set_text_style(&text_style);

    let (font_mgr, font_collection) =
        with_text_layout_cache(|cache| (cache.font_mgr.clone(), cache.font_collection.clone()));

    let mut font_collection = font_collection;
    font_collection.set_default_font_manager(font_mgr, None);
    let mut builder = ParagraphBuilder::new(&paragraph_style, font_collection);
    builder.push_style(&text_style);
    builder.add_text(text);
    let mut paragraph = builder.build();
    paragraph.layout(width.max(1.0));
    paragraph.paint(canvas, (x, y));
    paragraph.height()
}

fn new_surface(width: u32, height: u32) -> Option<Surface> {
    surfaces::raster_n32_premul((width as i32, height as i32))
}

fn read_surface_rgba(surface: &mut Surface, width: u32, height: u32) -> Vec<u8> {
    let byte_len = (width as usize)
        .checked_mul(height as usize)
        .and_then(|px| px.checked_mul(4))
        .unwrap_or(4);
    let mut bytes = vec![0_u8; byte_len];
    let info = ImageInfo::new(
        (width as i32, height as i32),
        ColorType::RGBA8888,
        AlphaType::Premul,
        None,
    );
    if surface.read_pixels(&info, bytes.as_mut_slice(), (width * 4) as usize, (0, 0)) {
        bytes
    } else {
        vec![0_u8; byte_len]
    }
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
                .ok_or_else(|| anyhow!("invalid raw path"))?,
            "-an",
            "-c:v",
            "libx264",
            "-preset",
            "veryfast",
            "-crf",
            "20",
            "-pix_fmt",
            "yuv420p",
            mp4_path
                .to_str()
                .ok_or_else(|| anyhow!("invalid mp4 path"))?,
        ])
        .status()
        .context("failed to run ffmpeg")?;
    if !status.success() {
        bail!("ffmpeg encode failed with status {status}");
    }
    Ok(())
}

fn format_warnings(warnings: &[Warning]) -> String {
    if warnings.is_empty() {
        return "none".to_string();
    }
    warnings
        .iter()
        .map(|warning| format!("{warning:?}"))
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
