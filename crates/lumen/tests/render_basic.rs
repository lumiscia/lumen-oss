use std::{
    collections::HashMap,
    sync::{Arc, RwLock},
};

use lumen::raster::{AlphaMode, BitmapFrame, RectI, SurfaceFrame};
use lumen::{
    AssetCache, Composition, Connection, Graph, InputPort, LumenError, NodeEval, NodeId,
    NodeInputs, NodeKind, NullMediaStore, OutputPort, PortValue, RasterFrame, RenderContext,
    RenderSettings, RuntimeCapabilityProfile, SurfacePool, TimelineSettings, Warning,
    media::{MediaStore, MockImageResolver, MockMediaStore, MockVideoResolver},
    node::{
        Node, ShapeGeometry, blur::Blur, boolean::Boolean, crop::Crop, frame_hold::FrameHold,
        media_in::LoopMode, media_in::MediaIn, media_in::MediaInKind, media_output::MediaOutput,
        memo::Memo, merge::Merge, resize::Resize, resize::ResizeMode, resize::ResizeSampling,
        shadow::Shadow, shape::Shape, shape_renderer::ShapeRenderer, solid_color::SolidColor,
        switch::Switch, transform::Transform, transform::TransformSampling,
    },
};

fn connect(graph: &mut Graph, from: NodeId, to: NodeId, to_port: &str) {
    graph
        .connect(Connection {
            from_node: from,
            from_port: OutputPort::default(),
            to_node: to,
            to_port: InputPort::named(to_port),
        })
        .expect("connection should be valid for test setup")
}

fn render_single(graph: Graph, width: u32, height: u32) -> RasterFrame {
    render_at_frame(graph, width, height, 0)
}

fn render_at_frame(graph: Graph, width: u32, height: u32, frame: u32) -> RasterFrame {
    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width,
            height,
            background_color: [0, 0, 0, 0],
        },
    );

    let mut context = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    );

    composition
        .render_frame(frame, &mut context)
        .expect("render should succeed")
}

fn render_with_store(
    graph: Graph,
    width: u32,
    height: u32,
    frame: u32,
    media_store: Arc<dyn MediaStore>,
) -> RasterFrame {
    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width,
            height,
            background_color: [0, 0, 0, 0],
        },
    );
    let mut context = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        media_store,
        RuntimeCapabilityProfile {
            has_image_resolver: true,
            has_video_resolver: true,
            has_threading: false,
            sink_types: vec![lumen::SinkType::Bitmap],
        },
    );

    composition
        .render_frame(frame, &mut context)
        .expect("render should succeed")
}

fn expect_bitmap(frame: RasterFrame) -> (Arc<Vec<u8>>, u32, u32) {
    match frame {
        RasterFrame::Bitmap(bitmap) => (bitmap.pixels, bitmap.storage_width, bitmap.storage_height),
        RasterFrame::Surface(_) => panic!("expected bitmap output"),
    }
}

fn test_context(width: u32, height: u32) -> RenderContext {
    let composition = Composition::new(
        Graph::new(),
        TimelineSettings {
            fps: 30.0,
            duration_frames: 1,
        },
        RenderSettings {
            width,
            height,
            background_color: [0, 0, 0, 0],
        },
    );
    RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    )
}

fn rgba_fill(width: u32, height: u32, rgba: [u8; 4]) -> Arc<Vec<u8>> {
    let mut out = Vec::with_capacity((width as usize) * (height as usize) * 4);
    for _ in 0..(width as usize * height as usize) {
        out.extend_from_slice(&rgba);
    }
    Arc::new(out)
}

#[test]
fn solid_color_to_media_output_renders_expected_bitmap() {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [10, 20, 30, 255],
            width: Some(4),
            height: Some(3),
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, solid, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 4, 3));
    assert_eq!((width, height), (4, 3));
    assert_eq!(bytes.len(), 4 * 3 * 4);
    for chunk in bytes.chunks_exact(4) {
        assert_eq!(chunk, &[10, 20, 30, 255]);
    }
}

#[test]
fn transform_translate_shifts_pixels() {
    let mut graph = Graph::new();
    let solid = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(4),
            height: Some(4),
        }),
    ));
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform {
            scale_x: 1.0,
            scale_y: 1.0,
            translate_x: 1.0,
            translate_y: 0.0,
            rotate: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
            sampling: lumen::node::transform::TransformSampling::Linear,
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, solid, transform, "source");
    connect(&mut graph, transform, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 4, 4));
    assert_eq!((width, height), (4, 4));

    let first_pixel = &bytes[0..4];
    let second_pixel = &bytes[4..8];
    assert_eq!(first_pixel, &[0, 0, 0, 0]);
    assert_eq!(second_pixel, &[255, 0, 0, 255]);
}

#[test]
fn merge_with_half_opacity_blends_base_and_overlay() {
    let mut graph = Graph::new();
    let base = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(2),
            height: Some(2),
        }),
    ));
    let overlay = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 0, 255, 255],
            width: Some(2),
            height: Some(2),
        }),
    ));
    let merge = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Merge(Merge {
            opacity: 0.5,
            ..Merge::default()
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, base, merge, "base");
    connect(&mut graph, overlay, merge, "overlay");
    connect(&mut graph, merge, output, "source");

    let (bytes, _, _) = expect_bitmap(render_single(graph, 2, 2));
    for chunk in bytes.chunks_exact(4) {
        assert!((120..=135).contains(&chunk[0]));
        assert_eq!(chunk[1], 0);
        assert!((120..=135).contains(&chunk[2]));
        assert_eq!(chunk[3], 255);
    }
}

#[test]
fn merge_with_smaller_overlay_preserves_base_dimensions() {
    let mut graph = Graph::new();
    let base = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(2),
            height: Some(2),
        }),
    ));
    let overlay = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 255, 0, 255],
            width: Some(1),
            height: Some(1),
        }),
    ));
    let merge = graph.add_node(Node::new(NodeId(0), NodeKind::Merge(Merge::default())));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, base, merge, "base");
    connect(&mut graph, overlay, merge, "overlay");
    connect(&mut graph, merge, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 2, 2));
    assert_eq!((width, height), (2, 2));
    assert_eq!(&bytes[0..4], &[0, 255, 0, 255]);
    assert_eq!(&bytes[4..8], &[255, 0, 0, 255]);
    assert_eq!(&bytes[8..12], &[255, 0, 0, 255]);
    assert_eq!(&bytes[12..16], &[255, 0, 0, 255]);
}

#[test]
fn boolean_with_smaller_mask_preserves_source_dimensions() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [10, 20, 30, 255],
            width: Some(2),
            height: Some(1),
        }),
    ));
    let mask = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 255, 255, 255],
            width: Some(1),
            height: Some(1),
        }),
    ));
    let boolean = graph.add_node(Node::new(NodeId(0), NodeKind::Boolean(Boolean::default())));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, source, boolean, "source");
    connect(&mut graph, mask, boolean, "mask");
    connect(&mut graph, boolean, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 2, 1));
    assert_eq!((width, height), (2, 1));
    assert_eq!(&bytes[0..4], &[10, 20, 30, 255]);
}

#[test]
fn media_output_pads_smaller_source_to_render_dimensions() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [12, 34, 56, 255],
            width: Some(1),
            height: Some(1),
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 2, 2));
    assert_eq!((width, height), (2, 2));
    assert_eq!(&bytes[0..4], &[12, 34, 56, 255]);
    for chunk in bytes[4..].chunks_exact(4) {
        assert_eq!(chunk, &[0, 0, 0, 0]);
    }
}

#[test]
fn merge_preserves_base_domain_metadata() {
    let mut inputs = NodeInputs::new();
    let base_format = RectI::new(8, 12, 4, 4);
    let base_data = RectI::new(8, 12, 2, 2);
    inputs.insert(
        "base",
        PortValue::RasterFrame(RasterFrame::Bitmap(BitmapFrame::with_domain(
            rgba_fill(2, 2, [255, 0, 0, 255]),
            2,
            2,
            base_format,
            base_data,
        ))),
    );
    inputs.insert(
        "overlay",
        PortValue::RasterFrame(RasterFrame::bitmap(rgba_fill(1, 1, [0, 255, 0, 255]), 1, 1)),
    );

    let mut ctx = test_context(4, 4);
    let output = Merge::default()
        .evaluate(&inputs, &mut ctx)
        .expect("merge evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    assert_eq!(frame.format_rect, base_format);
    assert_eq!(frame.data_rect, base_format);
}

#[test]
fn boolean_preserves_source_domain_metadata() {
    let mut inputs = NodeInputs::new();
    let source_format = RectI::new(-2, 3, 3, 2);
    let source_data = RectI::new(-1, 3, 2, 1);
    inputs.insert(
        "source",
        PortValue::RasterFrame(RasterFrame::Bitmap(BitmapFrame::with_domain(
            rgba_fill(3, 2, [5, 6, 7, 255]),
            3,
            2,
            source_format,
            source_data,
        ))),
    );
    inputs.insert(
        "mask",
        PortValue::RasterFrame(RasterFrame::bitmap(
            rgba_fill(1, 1, [255, 255, 255, 255]),
            1,
            1,
        )),
    );

    let mut ctx = test_context(3, 2);
    let output = Boolean::default()
        .evaluate(&inputs, &mut ctx)
        .expect("boolean evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    assert_eq!(frame.format_rect, source_format);
    assert_eq!(frame.data_rect, source_data);
}

#[test]
fn media_output_normalizes_domain_to_render_rect() {
    let mut inputs = NodeInputs::new();
    inputs.insert(
        "source",
        PortValue::RasterFrame(RasterFrame::Bitmap(BitmapFrame::with_domain(
            rgba_fill(2, 2, [1, 2, 3, 255]),
            2,
            2,
            RectI::new(100, 200, 2, 2),
            RectI::new(100, 200, 1, 1),
        ))),
    );

    let mut ctx = test_context(3, 2);
    let output = MediaOutput
        .evaluate(&inputs, &mut ctx)
        .expect("media output evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    let expected = RectI::from_size(3, 2);
    assert_eq!(frame.format_rect, expected);
    assert_eq!(frame.data_rect, expected);
}

#[test]
fn crop_preserves_shifted_domain_metadata() {
    let mut inputs = NodeInputs::new();
    inputs.insert(
        "source",
        PortValue::RasterFrame(RasterFrame::Bitmap(BitmapFrame::with_domain(
            rgba_fill(4, 3, [90, 40, 10, 255]),
            4,
            3,
            RectI::new(100, 200, 4, 3),
            RectI::new(101, 201, 2, 2),
        ))),
    );

    let mut ctx = test_context(4, 3);
    let output = Crop {
        x: 1,
        y: 1,
        width: 2,
        height: 1,
    }
    .evaluate(&inputs, &mut ctx)
    .expect("crop evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    assert_eq!(frame.format_rect, RectI::new(101, 201, 2, 1));
    assert_eq!(frame.data_rect, RectI::new(101, 201, 2, 1));
}

#[test]
fn resize_updates_format_rect_size_and_preserves_origin() {
    let mut inputs = NodeInputs::new();
    inputs.insert(
        "source",
        PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                rgba_fill(2, 2, [11, 22, 33, 200]),
                2,
                2,
                RectI::new(-5, 7, 2, 2),
                RectI::new(-5, 7, 2, 2),
            )
            .with_alpha_mode(AlphaMode::Unpremultiplied),
        )),
    );

    let mut ctx = test_context(5, 4);
    let output = Resize {
        width: 5,
        height: 4,
        mode: ResizeMode::Stretch,
        sampling: ResizeSampling::Nearest,
    }
    .evaluate(&inputs, &mut ctx)
    .expect("resize evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    assert_eq!(frame.format_rect, RectI::new(-5, 7, 5, 4));
    assert_eq!(frame.data_rect, RectI::new(-5, 7, 5, 4));
    assert_eq!(frame.alpha_mode, AlphaMode::Unpremultiplied);
}

#[test]
fn transform_preserves_source_domain_metadata() {
    let mut inputs = NodeInputs::new();
    inputs.insert(
        "source",
        PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                rgba_fill(3, 2, [44, 55, 66, 255]),
                3,
                2,
                RectI::new(20, 30, 3, 2),
                RectI::new(21, 30, 2, 1),
            )
            .with_alpha_mode(AlphaMode::Unpremultiplied),
        )),
    );

    let mut ctx = test_context(3, 2);
    let output = Transform {
        scale_x: 1.0,
        scale_y: 1.0,
        translate_x: 1.0,
        translate_y: 0.0,
        rotate: 0.0,
        pivot_x: 0.0,
        pivot_y: 0.0,
        sampling: TransformSampling::Nearest,
    }
    .evaluate(&inputs, &mut ctx)
    .expect("transform evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    assert_eq!(frame.format_rect, RectI::new(20, 30, 3, 2));
    assert_eq!(frame.data_rect, RectI::new(21, 30, 2, 1));
    assert_eq!(frame.alpha_mode, AlphaMode::Unpremultiplied);
}

#[test]
fn blur_preserves_source_domain_metadata() {
    let mut inputs = NodeInputs::new();
    inputs.insert(
        "source",
        PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                rgba_fill(2, 2, [120, 10, 80, 180]),
                2,
                2,
                RectI::new(3, 4, 2, 2),
                RectI::new(3, 4, 1, 1),
            )
            .with_alpha_mode(AlphaMode::Unpremultiplied),
        )),
    );

    let mut ctx = test_context(2, 2);
    let output = Blur { radius: 1.0 }
        .evaluate(&inputs, &mut ctx)
        .expect("blur evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    assert_eq!(frame.format_rect, RectI::new(3, 4, 2, 2));
    assert_eq!(frame.data_rect, RectI::new(3, 4, 1, 1));
    assert_eq!(frame.alpha_mode, AlphaMode::Unpremultiplied);
}

#[test]
fn shadow_preserves_source_domain_metadata() {
    let mut inputs = NodeInputs::new();
    inputs.insert(
        "source",
        PortValue::RasterFrame(RasterFrame::Bitmap(
            BitmapFrame::with_domain(
                rgba_fill(2, 1, [10, 200, 30, 255]),
                2,
                1,
                RectI::new(-10, -2, 2, 1),
                RectI::new(-10, -2, 2, 1),
            )
            .with_alpha_mode(AlphaMode::Unpremultiplied),
        )),
    );

    let mut ctx = test_context(2, 1);
    let output = Shadow {
        offset_x: 1,
        offset_y: 1,
        color: [0, 0, 0, 255],
    }
    .evaluate(&inputs, &mut ctx)
    .expect("shadow evaluation should succeed");
    let PortValue::RasterFrame(RasterFrame::Bitmap(frame)) = output else {
        panic!("expected bitmap frame output");
    };

    assert_eq!(frame.format_rect, RectI::new(-10, -2, 2, 1));
    assert_eq!(frame.data_rect, RectI::new(-10, -2, 2, 1));
    assert_eq!(frame.alpha_mode, AlphaMode::Unpremultiplied);
}

#[test]
fn raster_surface_clone_preserves_pixels_and_metadata() {
    let pool = Arc::new(SurfacePool::new());
    let mut surface_ref = pool
        .acquire(2, 1)
        .expect("surface allocation should succeed");
    surface_ref
        .surface_mut()
        .expect("surface should be available")
        .canvas()
        .clear(skia_safe::Color::from_argb(255, 11, 22, 33));

    let mut surface_frame = SurfaceFrame::new(surface_ref);
    surface_frame.format_rect = RectI::new(9, 8, 2, 1);
    surface_frame.data_rect = RectI::new(9, 8, 1, 1);
    surface_frame.alpha_mode = AlphaMode::Unpremultiplied;

    let cloned = RasterFrame::Surface(surface_frame).clone();
    let RasterFrame::Bitmap(bitmap) = cloned else {
        panic!("expected bitmap clone from surface frame");
    };

    assert_eq!(&bitmap.pixels[0..4], &[11, 22, 33, 255]);
    assert_eq!(&bitmap.pixels[4..8], &[11, 22, 33, 255]);
    assert_eq!(bitmap.format_rect, RectI::new(9, 8, 2, 1));
    assert_eq!(bitmap.data_rect, RectI::new(9, 8, 1, 1));
    assert_eq!(bitmap.alpha_mode, AlphaMode::Unpremultiplied);
}

#[test]
fn zero_opacity_merge_short_circuits_overlay_evaluation() {
    let mut graph = Graph::new();
    let base = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [7, 8, 9, 255],
            width: Some(2),
            height: Some(2),
        }),
    ));
    let overlay = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Image {
                source: "missing-image".to_string(),
            },
        }),
    ));
    let merge = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Merge(Merge {
            opacity: 0.0,
            ..Merge::default()
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, base, merge, "base");
    connect(&mut graph, overlay, merge, "overlay");
    connect(&mut graph, merge, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 2, 2));
    assert_eq!((width, height), (2, 2));
    for chunk in bytes.chunks_exact(4) {
        assert_eq!(chunk, &[7, 8, 9, 255]);
    }
}

#[test]
fn shape_to_shape_renderer_to_media_output_renders_rectangle() {
    let mut graph = Graph::new();
    let shape = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 3,
                height: 2,
            },
        }),
    ));
    let renderer = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_color: [0, 255, 0, 255],
            ..ShapeRenderer::default()
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    connect(&mut graph, shape, renderer, "vector");
    connect(&mut graph, renderer, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 3, 2));
    assert_eq!((width, height), (3, 2));
    for chunk in bytes.chunks_exact(4) {
        assert_eq!(chunk, &[0, 255, 0, 255]);
    }
}

#[test]
fn blur_spreads_single_pixel_into_neighbors() {
    let mut graph = Graph::new();
    let shape = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Shape(Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 3,
                height: 3,
            },
        }),
    ));
    let renderer = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::ShapeRenderer(ShapeRenderer {
            fill_enabled: false,
            stroke_enabled: true,
            stroke_width: 1.0,
            stroke_color: [255, 0, 0, 255],
            ..ShapeRenderer::default()
        }),
    ));
    let blur = graph.add_node(Node::new(NodeId(0), NodeKind::Blur(Blur { radius: 1.0 })));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, shape, renderer, "vector");
    connect(&mut graph, renderer, blur, "source");
    connect(&mut graph, blur, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 3, 3));
    assert_eq!((width, height), (3, 3));

    let center_alpha = bytes[((1 * 3 + 1) * 4 + 3) as usize];
    let corner_alpha = bytes[3];
    assert!(
        center_alpha > 0,
        "blur should spread border alpha into the transparent center"
    );
    assert!(corner_alpha < 255, "blur should soften edge alpha values");
}

#[test]
fn blur_with_zero_radius_is_passthrough() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 0, 255, 255],
            width: Some(2),
            height: Some(1),
        }),
    ));
    let blur = graph.add_node(Node::new(NodeId(0), NodeKind::Blur(Blur { radius: 0.0 })));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, blur, "source");
    connect(&mut graph, blur, output, "source");

    let (bytes, _, _) = expect_bitmap(render_single(graph, 2, 1));
    assert_eq!(bytes.as_slice(), &[0, 0, 255, 255, 0, 0, 255, 255]);
}

#[test]
fn frame_hold_uses_held_frame_for_upstream_evaluation() {
    let mut graph = Graph::new();
    let red = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(1),
            height: Some(1),
        }),
    ));
    let blue = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [0, 0, 255, 255],
            width: Some(1),
            height: Some(1),
        }),
    ));
    let mut switch_map = HashMap::new();
    switch_map.insert(0_u16, 0_u32..10_u32);
    switch_map.insert(1_u16, 10_u32..20_u32);
    let switch = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Switch(Switch::new(switch_map)),
    ));
    let hold = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::FrameHold(FrameHold { hold_frame: 0 }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));

    graph
        .connect(Connection {
            from_node: red,
            from_port: OutputPort::default(),
            to_node: switch,
            to_port: InputPort::Indexed(0),
        })
        .expect("switch input_0 should connect");
    graph
        .connect(Connection {
            from_node: blue,
            from_port: OutputPort::default(),
            to_node: switch,
            to_port: InputPort::Indexed(1),
        })
        .expect("switch input_1 should connect");
    connect(&mut graph, switch, hold, "source");
    connect(&mut graph, hold, output, "source");

    let (bytes, _, _) = expect_bitmap(render_at_frame(graph, 1, 1, 15));
    assert_eq!(bytes.as_slice(), &[255, 0, 0, 255]);
}

#[test]
fn identity_transform_is_passthrough() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [12, 34, 56, 255],
            width: Some(2),
            height: Some(1),
        }),
    ));
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform::default()),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, transform, "source");
    connect(&mut graph, transform, output, "source");

    let (bytes, _, _) = expect_bitmap(render_single(graph, 2, 1));
    assert_eq!(bytes.as_slice(), &[12, 34, 56, 255, 12, 34, 56, 255]);
}

#[test]
fn transform_translation_is_not_clipped_to_source_dimensions() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [255, 0, 0, 255],
            width: Some(320),
            height: Some(180),
        }),
    ));
    let transform = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Transform(Transform {
            scale_x: 1.0,
            scale_y: 1.0,
            translate_x: 200.0,
            translate_y: 0.0,
            rotate: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
            sampling: TransformSampling::Nearest,
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, transform, "source");
    connect(&mut graph, transform, output, "source");

    let (bytes, width, height) = expect_bitmap(render_single(graph, 640, 360));
    assert_eq!((width, height), (640, 360));

    let sample_y = 20usize;
    let pre_translate_x = 100usize;
    let translated_x = 350usize;
    let pre_idx = ((sample_y * width as usize) + pre_translate_x) * 4;
    let translated_idx = ((sample_y * width as usize) + translated_x) * 4;

    assert_eq!(&bytes[pre_idx..pre_idx + 4], &[0, 0, 0, 0]);
    assert_eq!(
        &bytes[translated_idx..translated_idx + 4],
        &[255, 0, 0, 255]
    );
}

#[test]
fn media_in_image_uses_image_resolver_and_renders_bitmap() {
    let mut graph = Graph::new();
    let media_in = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Image {
                source: "test-image".to_string(),
            },
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, media_in, output, "source");

    let mut store = MockMediaStore::default();
    store.insert_image(MockImageResolver::new(
        "test-image",
        2,
        1,
        vec![100, 50, 25, 128, 20, 40, 60, 255],
    ));

    let (bytes, width, height) = expect_bitmap(render_with_store(graph, 2, 1, 0, Arc::new(store)));
    assert_eq!((width, height), (2, 1));
    assert_eq!(bytes.as_slice(), &[50, 25, 13, 128, 20, 40, 60, 255]);
}

#[test]
fn media_in_video_speed_one_requests_matching_source_frame() {
    let mut graph = Graph::new();
    let media_in = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "video".to_string(),
                range: None,
                speed: 1.0,
                loop_mode: LoopMode::None,
            },
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, media_in, output, "source");

    let resolver = MockVideoResolver::new("video", 1, 1, 120, vec![10, 20, 30, 255]);
    let requested_frames = resolver.requested_frames();
    let mut store = MockMediaStore::default();
    store.insert_video(resolver);

    let _ = render_with_store(graph, 1, 1, 7, Arc::new(store));
    let calls = requested_frames.lock().expect("frames lock").clone();
    assert_eq!(calls, vec![7]);
}

#[test]
fn media_in_video_speed_two_requests_double_source_frame_rate() {
    let mut graph = Graph::new();
    let media_in = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "video".to_string(),
                range: None,
                speed: 2.0,
                loop_mode: LoopMode::None,
            },
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, media_in, output, "source");

    let resolver = MockVideoResolver::new("video", 1, 1, 240, vec![40, 80, 120, 255]);
    let requested_frames = resolver.requested_frames();
    let mut store = MockMediaStore::default();
    store.insert_video(resolver);

    let _ = render_with_store(graph, 1, 1, 7, Arc::new(store));
    let calls = requested_frames.lock().expect("frames lock").clone();
    assert_eq!(calls, vec![14]);
}

#[test]
fn composition_validate_rejects_video_node_without_video_resolver_capability() {
    let mut graph = Graph::new();
    let media_in = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "video".to_string(),
                range: None,
                speed: 1.0,
                loop_mode: LoopMode::None,
            },
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, media_in, output, "source");

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width: 1,
            height: 1,
            background_color: [0, 0, 0, 0],
        },
    );

    let profile = RuntimeCapabilityProfile {
        has_image_resolver: true,
        has_video_resolver: false,
        has_threading: false,
        sink_types: vec![lumen::SinkType::Bitmap],
    };

    let errors = composition
        .validate_against_profile(&profile)
        .expect_err("video resolver capability should be required");
    assert!(errors.iter().any(|error| matches!(
        error,
        LumenError::Render(lumen::error::RenderError::NodeEvaluation { node_id, .. }) if *node_id == media_in
    )));
}

#[test]
fn composition_validate_reports_fps_mismatch_warning_for_video_speed_change() {
    let mut graph = Graph::new();
    let media_in = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::MediaIn(MediaIn {
            kind: MediaInKind::Video {
                source: "video".to_string(),
                range: None,
                speed: 2.0,
                loop_mode: LoopMode::None,
            },
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, media_in, output, "source");

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width: 1,
            height: 1,
            background_color: [0, 0, 0, 0],
        },
    );

    let profile = RuntimeCapabilityProfile {
        has_image_resolver: true,
        has_video_resolver: true,
        has_threading: false,
        sink_types: vec![lumen::SinkType::Bitmap],
    };

    let warnings = composition
        .validate_against_profile(&profile)
        .expect("profile should validate with warning");
    assert!(warnings.iter().any(|warning| matches!(
        warning,
        Warning::FpsMismatch {
            node_id,
            composition_fps,
            source_fps,
        } if *node_id == media_in && (*composition_fps - 30.0).abs() < f32::EPSILON && (*source_fps - 60.0).abs() < f32::EPSILON
    )));
}

#[test]
fn instrumentation_tracks_node_cache_behavior() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [200, 10, 20, 255],
            width: Some(2),
            height: Some(1),
        }),
    ));
    let merge = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Merge(Merge {
            opacity: 0.5,
            ..Merge::default()
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, merge, "base");
    connect(&mut graph, source, merge, "overlay");
    connect(&mut graph, merge, output, "source");

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width: 2,
            height: 1,
            background_color: [0, 0, 0, 0],
        },
    );
    let mut context = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    );

    composition
        .render_frame(0, &mut context)
        .expect("render should succeed");
    let stats = context.instrumentation_snapshot();
    assert_eq!(stats.node_evaluations, 3);
    assert_eq!(stats.node_output_cache_misses, 3);
    assert_eq!(stats.node_output_cache_hits, 1);
    assert!(stats.pixel_allocation_bytes >= 8);
}

#[test]
fn instrumentation_tracks_memo_hits_and_misses_per_render() {
    let mut graph = Graph::new();
    let source = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::SolidColor(SolidColor {
            color: [25, 50, 75, 255],
            width: Some(2),
            height: Some(2),
        }),
    ));
    let memo = graph.add_node(Node::new(
        NodeId(0),
        NodeKind::Memo(Memo {
            cache_id: "stats-memo".to_string(),
            allow_expressions: false,
        }),
    ));
    let output = graph.add_node(Node::new(NodeId(0), NodeKind::MediaOutput(MediaOutput)));
    connect(&mut graph, source, memo, "source");
    connect(&mut graph, memo, output, "source");

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width: 2,
            height: 2,
            background_color: [0, 0, 0, 0],
        },
    );
    let mut context = RenderContext::new(
        &composition,
        Arc::new(SurfacePool::new()),
        Arc::new(RwLock::new(AssetCache::new())),
        Arc::new(NullMediaStore),
        RuntimeCapabilityProfile::cpu_only(),
    );

    composition
        .render_frame(0, &mut context)
        .expect("initial render should succeed");
    let first = context.instrumentation_snapshot();
    assert_eq!(first.memo_cache_misses, 1);
    assert_eq!(first.memo_cache_hits, 0);

    composition
        .render_frame(0, &mut context)
        .expect("second render should succeed");
    let second = context.instrumentation_snapshot();
    assert_eq!(second.memo_cache_hits, 1);
    assert_eq!(second.memo_cache_misses, 0);
}

#[test]
fn instrumentation_snapshots_surface_pool_acquires() {
    let mut context = test_context(1, 1);
    context.reset_instrumentation();

    {
        let _surface = context
            .surface_pool
            .acquire(3, 2)
            .expect("initial acquire should allocate");
    }
    {
        let _surface = context
            .surface_pool
            .acquire(3, 2)
            .expect("second acquire should reuse pooled surface");
    }

    let stats = context.instrumentation_snapshot();
    assert_eq!(stats.surface_acquires, 2);
    assert_eq!(stats.surface_fresh_allocations, 1);
    assert_eq!(stats.surface_reuses, 1);
    assert_eq!(stats.surface_fresh_allocation_bytes, 24);
    assert_eq!(
        stats.surface_acquires_by_size.get(&(3, 2)).copied(),
        Some(2)
    );
}
