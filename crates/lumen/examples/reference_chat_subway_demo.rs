#[cfg(not(all(feature = "ffmpeg", feature = "threading", feature = "json")))]
fn main() {
    eprintln!(
        "enable features \"ffmpeg threading json\" to run this example (e.g. cargo run -p lumen --example reference_chat_subway_demo --features \"ffmpeg threading json\" --release)"
    );
}

#[cfg(all(feature = "ffmpeg", feature = "threading", feature = "json"))]
mod app {
    use std::{
        cell::RefCell,
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
        AssetCache, BlendMode, Composition, Connection, FfmpegMediaStore, Graph, InputPort, NodeId,
        NodeKind, OutputPort, RasterFrame, RenderContext, RenderSettings, RuntimeCapabilityProfile,
        Sink, SinkType, SurfacePool, TimelineSettings, Warning,
        error::SinkError,
        media::{ImageResolver, MediaStore, VideoFrameResolver},
        node::{
            Node,
            boolean::{Boolean, MaskKind},
            media_in::{LoopMode, MediaIn, MediaInKind},
            media_output::MediaOutput,
            memo::Memo,
            merge::Merge,
            resize::{Resize, ResizeMode, ResizeSampling},
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
    const DURATION_FRAMES: u32 = 210;

    const BG_VIDEO_PATH: &str = "/Users/oglass/Downloads/subway_surfers_background.mp4";
    const REFERENCE_PATH: &str = "/Users/oglass/Downloads/reference.mp4";

    const PANEL_X: f32 = 160.0;
    const PANEL_Y: f32 = 220.0;
    const PANEL_W: f32 = 760.0;
    const PANEL_H: f32 = 1040.0;
    const HEADER_H: f32 = 108.0;
    const PANEL_INSET_X: f32 = 28.0;
    const BUBBLE_GAP: f32 = 20.0;
    const TEXT_PAD_X: f32 = 16.0;
    const TEXT_PAD_Y: f32 = 11.0;
    const MAX_TEXT_WRAP: f32 = 560.0;
    const REVEAL_FRAMES: u32 = 8;

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

    struct ReferenceMediaStore {
        images: HashMap<String, StaticImageResolver>,
        ffmpeg: FfmpegMediaStore,
    }

    impl MediaStore for ReferenceMediaStore {
        fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
            if let Some(image) = self.images.get(source) {
                return Some(Box::new(image.clone()) as Box<dyn ImageResolver>);
            }
            self.ffmpeg.get_image_resolver(source)
        }

        fn get_video_resolver(&self, source: &str) -> Option<Box<dyn VideoFrameResolver>> {
            self.ffmpeg.get_video_resolver(source)
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

    #[derive(Clone)]
    struct MessageSpec {
        text: &'static str,
        outgoing: bool,
        start_frame: u32,
        font_size: f32,
    }

    #[derive(Clone)]
    struct PlacedMessage {
        spec: MessageSpec,
        bubble_rect: Rect,
        text_rect: Rect,
        wrap_width: f32,
    }

    fn message_specs() -> Vec<MessageSpec> {
        vec![
            MessageSpec {
                text: "I deliver your food sir.",
                outgoing: false,
                start_frame: 0,
                font_size: 20.0,
            },
            MessageSpec {
                text: "Good luck amir.",
                outgoing: true,
                start_frame: 26,
                font_size: 20.0,
            },
            MessageSpec {
                text: "I am in a forest in Madagascar.",
                outgoing: true,
                start_frame: 56,
                font_size: 19.0,
            },
            MessageSpec {
                text: "It is full of dangerous species here.",
                outgoing: true,
                start_frame: 84,
                font_size: 19.0,
            },
            MessageSpec {
                text: "No problem sir.",
                outgoing: false,
                start_frame: 122,
                font_size: 19.0,
            },
            MessageSpec {
                text: "I built different.",
                outgoing: false,
                start_frame: 150,
                font_size: 19.0,
            },
            MessageSpec {
                text: "HOLY YAPPING OF THE YAPPINGTONGS!",
                outgoing: false,
                start_frame: 176,
                font_size: 18.5,
            },
        ]
    }

    fn panel_rect() -> Rect {
        Rect::from_xywh(PANEL_X, PANEL_Y, PANEL_W, PANEL_H)
    }

    fn inner_rect() -> Rect {
        let p = panel_rect();
        Rect::from_xywh(
            p.left + PANEL_INSET_X,
            p.top + HEADER_H + 20.0,
            p.width() - PANEL_INSET_X * 2.0,
            p.height() - HEADER_H - 36.0,
        )
    }

    fn layout_messages(specs: &[MessageSpec]) -> Vec<PlacedMessage> {
        let inner = inner_rect();
        let mut y = inner.top + 18.0;
        let mut placed = Vec::with_capacity(specs.len());
        for spec in specs {
            let max_wrap_width = MAX_TEXT_WRAP.min(inner.width() - 70.0).max(180.0);
            let intrinsic_w = measure_text_intrinsic_width(spec.text, spec.font_size);
            let needs_wrap = intrinsic_w > max_wrap_width;
            let wrap_width = if needs_wrap {
                max_wrap_width
            } else {
                // Give Skia a tiny margin so "exact fit" text doesn't wrap from float rounding.
                (intrinsic_w + 4.0).max(1.0)
            };
            let (wrapped_text_w, text_h) =
                measure_text_paragraph(spec.text, spec.font_size, wrap_width);
            let bubble_text_w = if needs_wrap {
                wrapped_text_w
            } else {
                intrinsic_w
            };
            let bubble_w =
                (bubble_text_w + TEXT_PAD_X * 2.0).clamp(116.0, max_wrap_width + TEXT_PAD_X * 2.0);
            let bubble_h = (text_h + TEXT_PAD_Y * 2.0).max(44.0);
            let bubble_x = if spec.outgoing {
                inner.right - 6.0 - bubble_w
            } else {
                inner.left + 6.0
            };
            let bubble_y = y;
            let bubble_rect = Rect::from_xywh(bubble_x, bubble_y, bubble_w, bubble_h);
            let text_rect = Rect::from_xywh(
                bubble_x + TEXT_PAD_X,
                bubble_y + TEXT_PAD_Y - 1.0,
                bubble_w - TEXT_PAD_X * 2.0,
                bubble_h - TEXT_PAD_Y * 2.0,
            );
            placed.push(PlacedMessage {
                spec: spec.clone(),
                bubble_rect,
                text_rect,
                wrap_width,
            });
            y += bubble_h + BUBBLE_GAP;
        }
        placed
    }

    fn render_chat_screen(placed: &[PlacedMessage]) -> Result<Vec<u8>> {
        let mut surface =
            new_surface(WIDTH, HEIGHT).ok_or_else(|| anyhow!("failed to create chat surface"))?;
        let canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        draw_panel_chrome(canvas);
        for message in placed {
            draw_message_bubble(canvas, message);
        }
        Ok(read_surface_rgba(&mut surface, WIDTH, HEIGHT))
    }

    fn render_panel_mask_canonical() -> Result<Vec<u8>> {
        let mut surface =
            new_surface(WIDTH, HEIGHT).ok_or_else(|| anyhow!("failed to create mask surface"))?;
        let canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);

        draw_round_rect(canvas, panel_rect(), 12.0, [255, 255, 255, 255]);
        Ok(read_surface_rgba(&mut surface, WIDTH, HEIGHT))
    }

    fn draw_panel_chrome(canvas: &skia_safe::Canvas) {
        let panel = panel_rect();
        draw_round_rect(canvas, panel, 12.0, [0, 0, 0, 255]);
        draw_round_rect(
            canvas,
            Rect::from_xywh(panel.left, panel.top, panel.width(), HEADER_H),
            12.0,
            [18, 18, 22, 255],
        );
        // flatten header bottom corners visually
        draw_rect(
            canvas,
            Rect::from_xywh(panel.left, panel.top + HEADER_H - 12.0, panel.width(), 12.0),
            [18, 18, 22, 255],
        );

        draw_text_paragraph(
            canvas,
            "<",
            panel.left + 18.0,
            panel.top + 18.0,
            32.0,
            28.0,
            500,
            [68, 147, 255, 255],
            ParagraphTextAlign::Left,
        );
        draw_circle(
            canvas,
            panel.center_x(),
            panel.top + 26.0,
            18.0,
            [241, 221, 178, 255],
        );
        draw_circle(
            canvas,
            panel.center_x(),
            panel.top + 26.0,
            12.0,
            [198, 146, 86, 255],
        );
        let _ = draw_text_paragraph(
            canvas,
            "UBER EATS guy(smash) >",
            panel.left + 120.0,
            panel.top + 16.0,
            panel.width() - 240.0,
            22.0,
            600,
            [255, 255, 255, 245],
            ParagraphTextAlign::Center,
        );
        let _ = draw_text_paragraph(
            canvas,
            "Message",
            panel.left + 120.0,
            panel.top + 50.0,
            panel.width() - 240.0,
            13.0,
            400,
            [160, 160, 168, 220],
            ParagraphTextAlign::Center,
        );
        draw_text_paragraph(
            canvas,
            "▢",
            panel.right - 40.0,
            panel.top + 19.0,
            24.0,
            20.0,
            400,
            [84, 161, 255, 240],
            ParagraphTextAlign::Center,
        );
    }

    fn draw_message_bubble(canvas: &skia_safe::Canvas, msg: &PlacedMessage) {
        let color = if msg.spec.outgoing {
            [52, 145, 255, 255]
        } else {
            [44, 44, 48, 255]
        };
        draw_round_rect(canvas, msg.bubble_rect, 16.0, color);

        let tail_rect = if msg.spec.outgoing {
            Rect::from_xywh(
                msg.bubble_rect.right - 14.0,
                msg.bubble_rect.bottom - 12.0,
                12.0,
                12.0,
            )
        } else {
            Rect::from_xywh(
                msg.bubble_rect.left + 2.0,
                msg.bubble_rect.bottom - 12.0,
                12.0,
                12.0,
            )
        };
        draw_round_rect(canvas, tail_rect, 2.5, color);

        let text_color = if msg.spec.outgoing {
            [255, 255, 255, 255]
        } else {
            [248, 248, 250, 255]
        };
        let _ = draw_text_paragraph(
            canvas,
            msg.spec.text,
            msg.text_rect.left,
            msg.text_rect.top,
            msg.text_rect.width(),
            msg.spec.font_size,
            500,
            text_color,
            ParagraphTextAlign::Left,
        );
    }

    fn build_composition(placed: &[PlacedMessage]) -> Result<Composition> {
        let mut graph = Graph::new();

        let bg_video = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::MediaIn(MediaIn {
                kind: MediaInKind::Video {
                    source: BG_VIDEO_PATH.to_string(),
                    range: Some(0..(DURATION_FRAMES + 60)),
                    speed: 1.0,
                    loop_mode: LoopMode::Repeat,
                },
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

        let chat_screen = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::MediaIn(MediaIn {
                kind: MediaInKind::Image {
                    source: "chat-screen".to_string(),
                },
            }),
        ));
        let chat_screen_memo = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::Memo(Memo {
                cache_id: "ref-chat-screen".to_string(),
                allow_expressions: false,
            }),
        ));
        let panel_mask = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::MediaIn(MediaIn {
                kind: MediaInKind::Image {
                    source: "panel-mask".to_string(),
                },
            }),
        ));
        let panel_mask_transform = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::Transform(Transform {
                scale_x: 1.0,
                scale_y: 1.0,
                translate_x: 0.0,
                translate_y: 0.0,
                rotate: 0.0,
                pivot_x: PANEL_X,
                pivot_y: PANEL_Y,
                sampling: TransformSampling::Nearest,
            }),
        ));
        let chat_reveal = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::Boolean(Boolean {
                mask_kind: MaskKind::Alpha,
                invert: false,
            }),
        ));
        let final_merge = graph.add_node(Node::new(
            NodeId(0),
            NodeKind::Merge(Merge {
                blend_mode: BlendMode::Normal,
                opacity: 1.0,
            }),
        ));

        connect(&mut graph, bg_video, "output", bg_resize, "source")?;
        connect(
            &mut graph,
            chat_screen,
            "output",
            chat_screen_memo,
            "source",
        )?;
        connect(
            &mut graph,
            panel_mask,
            "output",
            panel_mask_transform,
            "source",
        )?;
        connect(
            &mut graph,
            chat_screen_memo,
            "output",
            chat_reveal,
            "source",
        )?;
        connect(
            &mut graph,
            panel_mask_transform,
            "output",
            chat_reveal,
            "mask",
        )?;
        connect(&mut graph, bg_resize, "output", final_merge, "base")?;
        connect(&mut graph, chat_reveal, "output", final_merge, "overlay")?;

        let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
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

        let inner = inner_rect();
        let content_start = (inner.top + 18.0) - PANEL_Y;
        let reveal_margin = 14.0_f32;
        let bubble_height_expr = |msg: &PlacedMessage| {
            let safe_text = escape_expr_string(msg.spec.text);
            let fs = msg.spec.font_size;
            let wrap = msg.wrap_width;
            format!(
                "max(text_height('{safe_text}', {fs}, min(text_width('{safe_text}', {fs}), {wrap})) + {}, 58)",
                TEXT_PAD_Y * 2.0
            )
        };
        let heights: Vec<String> = placed.iter().map(bubble_height_expr).collect();
        let mut reveal_height_expr =
            format!("({content_start} + {} + {reveal_margin})", heights[0]);
        for (index, msg) in placed.iter().enumerate().skip(1) {
            let reveal_start = msg.spec.start_frame;
            let delta_expr = format!("({} + {})", heights[index], BUBBLE_GAP);
            reveal_height_expr = format!(
                "({reveal_height_expr} + {delta_expr} * clamp((frame - {reveal_start}) / {REVEAL_FRAMES}, 0, 1))"
            );
        }
        let scale_y_expr = format!("max(0.001, min(1, ({reveal_height_expr}) / {}))", PANEL_H);
        composition.set_expression(
            panel_mask_transform,
            "transform.scale_y",
            lumen::Expression::parse(&scale_y_expr)?,
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

    fn escape_expr_string(input: &str) -> String {
        let mut out = String::with_capacity(input.len() + 8);
        for ch in input.chars() {
            match ch {
                '\\' => out.push_str("\\\\"),
                '\'' => out.push_str("\\'"),
                '\n' => out.push_str("\\n"),
                _ => out.push(ch),
            }
        }
        out
    }

    fn measure_text_paragraph(text: &str, font_size: f32, width: f32) -> (f32, f32) {
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_align(ParagraphTextAlign::Left);

        let mut text_style = ParagraphTextStyle::new();
        text_style.set_font_size(font_size.max(1.0));
        text_style.set_color(Color::WHITE);
        text_style.set_font_style(FontStyle::new(
            Weight::from(500),
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
        (paragraph.longest_line(), paragraph.height())
    }

    fn measure_text_intrinsic_width(text: &str, font_size: f32) -> f32 {
        let mut paragraph_style = ParagraphStyle::new();
        paragraph_style.set_text_align(ParagraphTextAlign::Left);

        let mut text_style = ParagraphTextStyle::new();
        text_style.set_font_size(font_size.max(1.0));
        text_style.set_color(Color::WHITE);
        text_style.set_font_style(FontStyle::new(
            Weight::from(500),
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
        paragraph.layout(16_384.0);
        paragraph.max_intrinsic_width().ceil()
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

    fn draw_round_rect(canvas: &skia_safe::Canvas, rect: Rect, radius: f32, color: [u8; 4]) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_style(PaintStyle::Fill);
        paint.set_color(Color::from_argb(color[3], color[0], color[1], color[2]));
        canvas.draw_rrect(RRect::new_rect_xy(rect, radius, radius), &paint);
    }

    fn draw_rect(canvas: &skia_safe::Canvas, rect: Rect, color: [u8; 4]) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_style(PaintStyle::Fill);
        paint.set_color(Color::from_argb(color[3], color[0], color[1], color[2]));
        canvas.draw_rect(rect, &paint);
    }

    fn draw_circle(canvas: &skia_safe::Canvas, cx: f32, cy: f32, radius: f32, color: [u8; 4]) {
        let mut paint = Paint::default();
        paint.set_anti_alias(true);
        paint.set_style(PaintStyle::Fill);
        paint.set_color(Color::from_argb(color[3], color[0], color[1], color[2]));
        canvas.draw_circle((cx, cy), radius, &paint);
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

    pub fn main() -> Result<()> {
        let started = Instant::now();
        let artifact_dir = workspace_root()
            .join("artifacts")
            .join("lumen-reference-chat-subway");
        fs::create_dir_all(&artifact_dir)?;

        let raw_path = artifact_dir.join("reference_chat_subway.rgba");
        let video_path = artifact_dir.join("reference_chat_subway.mp4");
        let stats_path = artifact_dir.join("stats.txt");

        let specs = message_specs();
        let placed = layout_messages(&specs);

        let mut images = HashMap::new();
        let mut image_resolve_stats: Vec<(String, Arc<Mutex<CallStats>>)> = Vec::new();
        let chat_screen_stats = Arc::new(Mutex::new(CallStats::default()));
        images.insert(
            "chat-screen".to_string(),
            StaticImageResolver {
                id: "chat-screen".to_string(),
                width: WIDTH,
                height: HEIGHT,
                pixels: Arc::new(render_chat_screen(&placed)?),
                stats: Arc::clone(&chat_screen_stats),
            },
        );
        image_resolve_stats.push(("chat-screen".to_string(), chat_screen_stats));

        let panel_mask_stats = Arc::new(Mutex::new(CallStats::default()));
        images.insert(
            "panel-mask".to_string(),
            StaticImageResolver {
                id: "panel-mask".to_string(),
                width: WIDTH,
                height: HEIGHT,
                pixels: Arc::new(render_panel_mask_canonical()?),
                stats: Arc::clone(&panel_mask_stats),
            },
        );
        image_resolve_stats.push(("panel-mask".to_string(), panel_mask_stats));

        let media_store: Arc<dyn MediaStore> = Arc::new(ReferenceMediaStore {
            images,
            ffmpeg: FfmpegMediaStore::new(),
        });

        let composition = build_composition(&placed)?;
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

        let mut report = String::new();
        use std::fmt::Write as _;
        writeln!(&mut report, "lumen reference chat subway demo")?;
        writeln!(&mut report, "reference={REFERENCE_PATH}")?;
        writeln!(&mut report, "background={BG_VIDEO_PATH}")?;
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
        for (id, stats) in &image_resolve_stats {
            let calls = stats.lock().map(|s| s.calls).unwrap_or(0);
            writeln!(&mut report, "image_resolve_calls[{id}]={calls}")?;
        }
        writeln!(&mut report, "warnings_count={}", warnings.len())?;
        writeln!(&mut report, "warnings={}", format_warnings(&warnings))?;

        fs::write(&stats_path, &report)?;
        println!("{report}");
        Ok(())
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
}

#[cfg(all(feature = "ffmpeg", feature = "threading", feature = "json"))]
fn main() -> anyhow::Result<()> {
    app::main()
}
