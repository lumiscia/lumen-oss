use std::collections::HashMap;

use lumen::{
    composition::{Composition, RenderSettings, TimelineSettings},
    error::LumenError,
    gpu_image::GpuImageFrame,
    graph::{Connection, Graph},
    media::MediaStore,
    node::{
        NodeId, NodeKind, NodeProperty, PortRef,
        compositing::{
            boolean::Boolean, merge::Merge, raster_multimerge::RasterMultiMerge, switch::Switch,
        },
        media_output::MediaOutput,
        processing::{
            alpha_premultiply::AlphaPremultiply, blur::Blur, channel_shuffle::ChannelShuffle,
            color_grade::ColorGrade, crop::Crop, curves::Curves, exposure::Exposure,
            hue_saturation::HueSaturation, levels::Levels, matte_cleanup::MatteCleanup,
            resize::Resize, shadow::Shadow, time_remap::TimeRemap, transform::Transform,
        },
        source::solid_color::SolidColor,
        vector::{
            shape::Shape, shape_renderer::ShapeRenderer, vector_multimerge::VectorMultiMerge,
        },
    },
    render::{
        LumenRenderer,
        surface::{DefaultSurfacePool, SurfacePool, SurfacePoolStats},
    },
};

// ---- Null media store for tests that don't need media resolvers ----

#[derive(Debug)]
struct NullMediaStore;

impl MediaStore for NullMediaStore {
    fn get_image_resolver(&self, _source: &str) -> Option<Box<dyn lumen::media::ImageResolver>> {
        None
    }
    fn get_video_resolver(
        &self,
        _stream_id: &str,
    ) -> Option<Box<dyn lumen::media::VideoFrameResolver>> {
        None
    }
}

// ---- Test helpers ----

#[derive(Debug)]
struct TestSurfacePool;

impl SurfacePool for TestSurfacePool {
    fn with_surface<T>(
        &self,
        width: u32,
        height: u32,
        f: impl FnOnce(&mut skia_safe::Surface) -> Result<T, LumenError>,
    ) -> Result<T, LumenError> {
        let mut surface =
            skia_safe::surfaces::raster_n32_premul((width.max(1) as i32, height.max(1) as i32))
                .ok_or(lumen::error::RenderError::SurfaceAllocation { width, height })?;
        f(&mut surface)
    }

    fn stats(&self) -> SurfacePoolStats {
        SurfacePoolStats::default()
    }

    fn flush(&self) {}
}

fn node_id(id: u64) -> NodeId {
    NodeId::new(id)
}

fn connect(graph: &mut Graph, from: NodeId, from_port: &str, to: NodeId, to_port: &str) {
    graph
        .connect(Connection {
            from_node: from,
            from_port: from_port.to_string(),
            to_node: to,
            to_port: to_port.to_string(),
        })
        .expect("connection should succeed");
}

struct ReadbackFrame {
    pixels: Vec<u8>,
    storage_width: u32,
    storage_height: u32,
}

fn readback_frame(frame: GpuImageFrame) -> ReadbackFrame {
    let (storage_width, storage_height) = frame.storage_dimensions();
    let mut pixels = vec![0; (storage_width as usize) * (storage_height as usize) * 4];
    frame
        .read_pixels_into(pixels.as_mut_slice(), (storage_width as usize) * 4)
        .expect("read pixels");
    ReadbackFrame {
        pixels,
        storage_width,
        storage_height,
    }
}

fn render_frame(graph: Graph, width: u32, height: u32, frame: u32) -> ReadbackFrame {
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
    let pool = TestSurfacePool;
    let media = NullMediaStore;
    let mut renderer = LumenRenderer::new(&composition, &pool, &media).unwrap();
    let rendered = renderer.render(frame).unwrap();
    pool.flush();
    readback_frame(rendered)
}

fn render_single(graph: Graph, width: u32, height: u32) -> ReadbackFrame {
    render_frame(graph, width, height, 0)
}

fn pixel_at(bitmap: &ReadbackFrame, x: u32, y: u32) -> &[u8] {
    let offset = ((y * bitmap.storage_width + x) as usize) * 4;
    &bitmap.pixels[offset..offset + 4]
}

fn assert_pixel_near(actual: &[u8], expected: [u8; 4], tolerance: u8) {
    for (index, (&actual, expected)) in actual.iter().zip(expected).enumerate() {
        let delta = actual.abs_diff(expected);
        assert!(
            delta <= tolerance,
            "channel {index}: actual {actual}, expected {expected}, tolerance {tolerance}"
        );
    }
}

fn render_processing_node(
    mut node: NodeKind,
    source_color: [u8; 4],
    width: u32,
    height: u32,
) -> ReadbackFrame {
    let mut graph = Graph::new();
    let source_id = node_id(1);
    let process_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        source_id,
        NodeKind::SolidColor(SolidColor {
            id: source_id,
            color: NodeProperty::Color(source_color),
            width: NodeProperty::Int(width.into()),
            height: NodeProperty::Int(height.into()),
        }),
    );
    match &mut node {
        NodeKind::AlphaPremultiply(node) => {
            node.source = PortRef::new(source_id, "output".to_string())
        }
        NodeKind::ChannelShuffle(node) => {
            node.source = PortRef::new(source_id, "output".to_string())
        }
        NodeKind::ColorGrade(node) => node.source = PortRef::new(source_id, "output".to_string()),
        NodeKind::Curves(node) => node.source = PortRef::new(source_id, "output".to_string()),
        NodeKind::Exposure(node) => node.source = PortRef::new(source_id, "output".to_string()),
        NodeKind::HueSaturation(node) => {
            node.source = PortRef::new(source_id, "output".to_string())
        }
        NodeKind::Levels(node) => node.source = PortRef::new(source_id, "output".to_string()),
        NodeKind::MatteCleanup(node) => node.source = PortRef::new(source_id, "output".to_string()),
        _ => panic!("test helper only accepts processing nodes"),
    }
    graph.nodes.insert(process_id, node);
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(process_id, "output".to_string()),
        }),
    );
    connect(&mut graph, source_id, "output", process_id, "source");
    connect(&mut graph, process_id, "output", output_id, "source");

    render_single(graph, width, height)
}

// ---- Tests ----

#[test]
fn solid_color_to_media_output_renders_expected_bitmap() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let output_id = node_id(2);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([10, 20, 30, 255]),
            width: NodeProperty::Int(4),
            height: NodeProperty::Int(3),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", output_id, "source");

    let bitmap = render_single(graph, 4, 3);
    assert_eq!(bitmap.storage_width, 4);
    assert_eq!(bitmap.storage_height, 3);
    assert_eq!(bitmap.pixels.len(), 4 * 3 * 4);
    for chunk in bitmap.pixels.chunks_exact(4) {
        assert_eq!(chunk, &[10, 20, 30, 255]);
    }
}

#[test]
fn solid_color_defaults_to_render_dimensions_when_zero() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let output_id = node_id(2);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            ..SolidColor::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", output_id, "source");

    let bitmap = render_single(graph, 8, 6);
    assert_eq!(bitmap.storage_width, 8);
    assert_eq!(bitmap.storage_height, 6);
}

#[test]
fn gpu_processing_nodes_render_expected_pixels() {
    let exposure_id = node_id(2);
    let exposure = render_processing_node(
        NodeKind::Exposure(Exposure {
            id: exposure_id,
            exposure: NodeProperty::Float(1.0),
            contrast: NodeProperty::Float(1.0),
            offset: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }),
        [64, 32, 16, 255],
        1,
        1,
    );
    assert_eq!(pixel_at(&exposure, 0, 0), &[128, 64, 32, 255]);

    let levels_id = node_id(2);
    let levels = render_processing_node(
        NodeKind::Levels(Levels {
            id: levels_id,
            black_point: NodeProperty::Float(64.0 / 255.0),
            white_point: NodeProperty::Float(192.0 / 255.0),
            gamma: NodeProperty::Float(1.0),
            output_black: NodeProperty::Float(0.0),
            output_white: NodeProperty::Float(1.0),
            source: PortRef::empty(),
        }),
        [128, 64, 192, 255],
        1,
        1,
    );
    assert_eq!(pixel_at(&levels, 0, 0), &[128, 0, 255, 255]);

    let hue_id = node_id(2);
    let hue = render_processing_node(
        NodeKind::HueSaturation(HueSaturation {
            id: hue_id,
            hue_degrees: NodeProperty::Float(120.0),
            saturation: NodeProperty::Float(1.0),
            lightness: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }),
        [255, 0, 0, 255],
        1,
        1,
    );
    assert_eq!(pixel_at(&hue, 0, 0), &[0, 255, 0, 255]);

    let shuffle_id = node_id(2);
    let shuffle = render_processing_node(
        NodeKind::ChannelShuffle(ChannelShuffle {
            id: shuffle_id,
            red: NodeProperty::String("blue".to_string()),
            green: NodeProperty::String("green".to_string()),
            blue: NodeProperty::String("red".to_string()),
            alpha: NodeProperty::String("0.5".to_string()),
            source: PortRef::empty(),
        }),
        [10, 20, 30, 255],
        1,
        1,
    );
    assert_eq!(pixel_at(&shuffle, 0, 0), &[30, 20, 10, 128]);
}

#[test]
fn gpu_lookup_and_alpha_nodes_render_expected_pixels() {
    let curves_id = node_id(2);
    let curves = render_processing_node(
        NodeKind::Curves(Curves {
            id: curves_id,
            curve: NodeProperty::String("0:0,1:1".to_string()),
            red_curve: NodeProperty::String("0:0,0.5:1,1:1".to_string()),
            green_curve: NodeProperty::String(String::new()),
            blue_curve: NodeProperty::String(String::new()),
            source: PortRef::empty(),
        }),
        [128, 128, 128, 255],
        1,
        1,
    );
    assert_pixel_near(pixel_at(&curves, 0, 0), [255, 128, 128, 255], 1);

    let color_grade_id = node_id(2);
    let color_grade = render_processing_node(
        NodeKind::ColorGrade(ColorGrade {
            id: color_grade_id,
            lut_source: NodeProperty::String("rgb1d: 0,0,0; 255,128,0".to_string()),
            strength: NodeProperty::Float(0.5),
            interpolation: NodeProperty::Int(1),
            source: PortRef::empty(),
        }),
        [128, 64, 255, 255],
        1,
        1,
    );
    assert_eq!(pixel_at(&color_grade, 0, 0), &[128, 48, 128, 255]);

    let matte_id = node_id(2);
    let matte = render_processing_node(
        NodeKind::MatteCleanup(MatteCleanup {
            id: matte_id,
            threshold: NodeProperty::Float(0.5),
            shrink: NodeProperty::Int(0),
            grow: NodeProperty::Int(0),
            source: PortRef::empty(),
        }),
        [10, 20, 30, 128],
        1,
        1,
    );
    assert_eq!(pixel_at(&matte, 0, 0), &[5, 10, 15, 255]);

    let premultiply_id = node_id(2);
    let premultiply = render_processing_node(
        NodeKind::AlphaPremultiply(AlphaPremultiply {
            id: premultiply_id,
            mode: NodeProperty::String("unpremultiply".to_string()),
            source: PortRef::empty(),
        }),
        [50, 25, 13, 128],
        1,
        1,
    );
    assert_pixel_near(pixel_at(&premultiply, 0, 0), [50, 25, 13, 128], 1);
}

#[test]
fn merge_with_half_opacity_blends_colors() {
    let mut graph = Graph::new();
    let base_id = node_id(1);
    let overlay_id = node_id(2);
    let merge_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        base_id,
        NodeKind::SolidColor(SolidColor {
            id: base_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
        }),
    );
    graph.nodes.insert(
        overlay_id,
        NodeKind::SolidColor(SolidColor {
            id: overlay_id,
            color: NodeProperty::Color([0, 0, 255, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
        }),
    );
    graph.nodes.insert(
        merge_id,
        NodeKind::Merge(Merge {
            id: merge_id,
            opacity: NodeProperty::Float(0.5),
            base: PortRef::new(base_id, "output".to_string()),
            overlay: PortRef::new(overlay_id, "output".to_string()),
            ..Merge::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(merge_id, "output".to_string()),
        }),
    );
    connect(&mut graph, base_id, "output", merge_id, "base");
    connect(&mut graph, overlay_id, "output", merge_id, "overlay");
    connect(&mut graph, merge_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 2);
    for chunk in bitmap.pixels.chunks_exact(4) {
        // Premultiplied blend of red + blue at 50%
        assert!((120..=135).contains(&chunk[0]), "red channel: {}", chunk[0]);
        assert_eq!(chunk[1], 0);
        assert!(
            (120..=135).contains(&chunk[2]),
            "blue channel: {}",
            chunk[2]
        );
        assert_eq!(chunk[3], 255);
    }
}

#[test]
fn zero_opacity_merge_short_circuits_overlay() {
    let mut graph = Graph::new();
    let base_id = node_id(1);
    let overlay_id = node_id(2);
    let merge_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        base_id,
        NodeKind::SolidColor(SolidColor {
            id: base_id,
            color: NodeProperty::Color([7, 8, 9, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
        }),
    );
    // Overlay references a missing media source - should not be evaluated
    graph.nodes.insert(
        overlay_id,
        NodeKind::SolidColor(SolidColor {
            id: overlay_id,
            color: NodeProperty::Color([99, 99, 99, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
        }),
    );
    graph.nodes.insert(
        merge_id,
        NodeKind::Merge(Merge {
            id: merge_id,
            opacity: NodeProperty::Float(0.0),
            base: PortRef::new(base_id, "output".to_string()),
            overlay: PortRef::new(overlay_id, "output".to_string()),
            ..Merge::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(merge_id, "output".to_string()),
        }),
    );
    connect(&mut graph, base_id, "output", merge_id, "base");
    connect(&mut graph, overlay_id, "output", merge_id, "overlay");
    connect(&mut graph, merge_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 2);
    for chunk in bitmap.pixels.chunks_exact(4) {
        assert_eq!(chunk, &[7, 8, 9, 255]);
    }
}

#[test]
fn blur_with_zero_radius_is_passthrough() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let blur_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([0, 0, 255, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        blur_id,
        NodeKind::Blur(Blur {
            id: blur_id,
            radius: NodeProperty::Float(0.0),
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(blur_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", blur_id, "source");
    connect(&mut graph, blur_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 1);
    assert_eq!(bitmap.pixels.as_slice(), &[0, 0, 255, 255, 0, 0, 255, 255]);
}

#[test]
fn identity_transform_is_passthrough() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let transform_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([12, 34, 56, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        transform_id,
        NodeKind::Transform(Transform {
            id: transform_id,
            source: PortRef::new(solid_id, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(transform_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", transform_id, "source");
    connect(&mut graph, transform_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 1);
    assert_eq!(
        bitmap.pixels.as_slice(),
        &[12, 34, 56, 255, 12, 34, 56, 255]
    );
}

#[test]
fn transform_translate_shifts_pixels() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let transform_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(4),
            height: NodeProperty::Int(4),
        }),
    );
    graph.nodes.insert(
        transform_id,
        NodeKind::Transform(Transform {
            id: transform_id,
            translate_x: NodeProperty::Float(1.0),
            pivot_x: NodeProperty::Float(0.0),
            pivot_y: NodeProperty::Float(0.0),
            source: PortRef::new(solid_id, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(transform_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", transform_id, "source");
    connect(&mut graph, transform_id, "output", output_id, "source");

    let bitmap = render_single(graph, 4, 4);
    let first_pixel = &bitmap.pixels[0..4];
    let second_pixel = &bitmap.pixels[4..8];
    assert_eq!(first_pixel, &[0, 0, 0, 0]);
    assert_eq!(second_pixel, &[255, 0, 0, 255]);
}

#[test]
fn transform_preserves_size_for_offset_raster_inputs() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let crop_id = node_id(2);
    let transform_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(4),
            height: NodeProperty::Int(4),
        }),
    );
    graph.nodes.insert(
        crop_id,
        NodeKind::Crop(Crop {
            id: crop_id,
            x: NodeProperty::Int(1),
            y: NodeProperty::Int(1),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        transform_id,
        NodeKind::Transform(Transform {
            id: transform_id,
            translate_x: NodeProperty::Float(1.0),
            source: PortRef::new(crop_id, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(transform_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", crop_id, "source");
    connect(&mut graph, crop_id, "output", transform_id, "source");
    connect(&mut graph, transform_id, "output", output_id, "source");

    let bitmap = render_single(graph, 5, 5);
    let pixel_at = |x: usize, y: usize| -> &[u8] {
        let idx = (y * 5 + x) * 4;
        &bitmap.pixels[idx..idx + 4]
    };

    assert_eq!(pixel_at(2, 1), &[255, 0, 0, 255]);
    assert_eq!(pixel_at(3, 1), &[255, 0, 0, 255]);
    assert_eq!(pixel_at(2, 2), &[255, 0, 0, 255]);
    assert_eq!(pixel_at(3, 2), &[255, 0, 0, 255]);

    let red_pixels = bitmap
        .pixels
        .chunks_exact(4)
        .filter(|pixel| **pixel == [255, 0, 0, 255])
        .count();
    assert_eq!(red_pixels, 4);
}

#[test]
fn crop_extracts_subregion() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let crop_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([50, 100, 150, 255]),
            width: NodeProperty::Int(4),
            height: NodeProperty::Int(4),
        }),
    );
    graph.nodes.insert(
        crop_id,
        NodeKind::Crop(Crop {
            id: crop_id,
            x: NodeProperty::Int(1),
            y: NodeProperty::Int(1),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(crop_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", crop_id, "source");
    connect(&mut graph, crop_id, "output", output_id, "source");

    // Crop output has domain offset at (1,1), so MediaOutput will position
    // the cropped content within its render rect. Use render size 4x4 to
    // see the crop at offset (1,1).
    let bitmap = render_single(graph, 4, 4);
    assert_eq!(bitmap.storage_width, 4);
    assert_eq!(bitmap.storage_height, 4);
    // The cropped 2x2 region should appear at position (1,1) in the output
    let pixel_at = |x: usize, y: usize| -> &[u8] {
        let idx = (y * 4 + x) * 4;
        &bitmap.pixels[idx..idx + 4]
    };
    // (0,0) should be transparent (outside crop region)
    assert_eq!(pixel_at(0, 0), &[0, 0, 0, 0]);
    // (1,1) should contain the color
    assert_eq!(pixel_at(1, 1), &[50, 100, 150, 255]);
    // (2,2) should also be within the crop
    assert_eq!(pixel_at(2, 2), &[50, 100, 150, 255]);
    // (3,3) should be transparent
    assert_eq!(pixel_at(3, 3), &[0, 0, 0, 0]);
}

#[test]
fn resize_changes_output_dimensions() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let resize_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([220, 30, 40, 255]),
            width: NodeProperty::Int(320),
            height: NodeProperty::Int(180),
        }),
    );
    graph.nodes.insert(
        resize_id,
        NodeKind::Resize(Resize {
            id: resize_id,
            width: NodeProperty::Int(640),
            height: NodeProperty::Int(360),
            source: PortRef::new(solid_id, "output".to_string()),
            ..Resize::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(resize_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", resize_id, "source");
    connect(&mut graph, resize_id, "output", output_id, "source");

    let bitmap = render_single(graph, 640, 360);
    assert_eq!(bitmap.storage_width, 640);
    assert_eq!(bitmap.storage_height, 360);
    // Sample a pixel in the middle to verify content
    let idx = ((100 * 640) + 500) * 4;
    assert_eq!(&bitmap.pixels[idx..idx + 4], &[220, 30, 40, 255]);
}

#[test]
fn shadow_with_transparent_color_is_passthrough() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let shadow_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([40, 80, 120, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
        }),
    );
    graph.nodes.insert(
        shadow_id,
        NodeKind::Shadow(Shadow {
            id: shadow_id,
            color: NodeProperty::Color([0, 0, 0, 0]), // fully transparent shadow
            source: PortRef::new(solid_id, "output".to_string()),
            ..Shadow::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(shadow_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", shadow_id, "source");
    connect(&mut graph, shadow_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 2);
    for chunk in bitmap.pixels.chunks_exact(4) {
        assert_eq!(chunk, &[40, 80, 120, 255]);
    }
}

#[test]
fn switch_selects_red_layer_in_first_range() {
    let mut graph = Graph::new();
    let red_id = node_id(1);
    let blue_id = node_id(2);
    let switch_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        red_id,
        NodeKind::SolidColor(SolidColor {
            id: red_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        blue_id,
        NodeKind::SolidColor(SolidColor {
            id: blue_id,
            color: NodeProperty::Color([0, 0, 255, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );

    let mut map = HashMap::new();
    map.insert(0u16, 0u32..10u32);
    map.insert(1u16, 10u32..20u32);

    graph.nodes.insert(
        switch_id,
        NodeKind::Switch(Switch {
            id: switch_id,
            map,
            layers: vec![
                PortRef::new(red_id, "output".to_string()),
                PortRef::new(blue_id, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(switch_id, "output".to_string()),
        }),
    );
    connect(&mut graph, red_id, "output", switch_id, "layers");
    connect(&mut graph, blue_id, "output", switch_id, "layers");
    connect(&mut graph, switch_id, "output", output_id, "source");

    let bitmap = render_frame(graph, 1, 1, 5);
    assert_eq!(bitmap.pixels.as_slice(), &[255, 0, 0, 255]);
}

#[test]
fn switch_selects_blue_layer_in_second_range() {
    let mut graph = Graph::new();
    let red_id = node_id(1);
    let blue_id = node_id(2);
    let switch_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        red_id,
        NodeKind::SolidColor(SolidColor {
            id: red_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        blue_id,
        NodeKind::SolidColor(SolidColor {
            id: blue_id,
            color: NodeProperty::Color([0, 0, 255, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );

    let mut map = HashMap::new();
    map.insert(0u16, 0u32..10u32);
    map.insert(1u16, 10u32..20u32);

    graph.nodes.insert(
        switch_id,
        NodeKind::Switch(Switch {
            id: switch_id,
            map,
            layers: vec![
                PortRef::new(red_id, "output".to_string()),
                PortRef::new(blue_id, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(switch_id, "output".to_string()),
        }),
    );
    connect(&mut graph, red_id, "output", switch_id, "layers");
    connect(&mut graph, blue_id, "output", switch_id, "layers");
    connect(&mut graph, switch_id, "output", output_id, "source");

    let bitmap = render_frame(graph, 1, 1, 15);
    assert_eq!(bitmap.pixels.as_slice(), &[0, 0, 255, 255]);
}

#[test]
fn switch_returns_transparent_for_unmatched_frame() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let switch_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );

    let mut map = HashMap::new();
    map.insert(0u16, 0u32..5u32);

    graph.nodes.insert(
        switch_id,
        NodeKind::Switch(Switch {
            id: switch_id,
            map,
            layers: vec![PortRef::new(solid_id, "output".to_string())],
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(switch_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", switch_id, "layers");
    connect(&mut graph, switch_id, "output", output_id, "source");

    // Frame 10 is outside range 0..5, should return transparent
    let bitmap = render_frame(graph, 1, 1, 10);
    assert_eq!(bitmap.pixels.as_slice(), &[0, 0, 0, 0]);
}

#[test]
fn media_output_pads_smaller_source_to_render_dimensions() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let output_id = node_id(2);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([12, 34, 56, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(solid_id, "output".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 2);
    assert_eq!(bitmap.storage_width, 2);
    assert_eq!(bitmap.storage_height, 2);
    assert_eq!(&bitmap.pixels[0..4], &[12, 34, 56, 255]);
    // Remaining pixels should be transparent
    for chunk in bitmap.pixels[4..].chunks_exact(4) {
        assert_eq!(chunk, &[0, 0, 0, 0]);
    }
}

#[test]
fn raster_multimerge_composites_multiple_layers() {
    let mut graph = Graph::new();
    let red_id = node_id(1);
    let green_id = node_id(2);
    let merge_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        red_id,
        NodeKind::SolidColor(SolidColor {
            id: red_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
        }),
    );
    graph.nodes.insert(
        green_id,
        NodeKind::SolidColor(SolidColor {
            id: green_id,
            color: NodeProperty::Color([0, 255, 0, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
        }),
    );
    graph.nodes.insert(
        merge_id,
        NodeKind::RasterMultimerge(RasterMultiMerge {
            id: merge_id,
            opacity: NodeProperty::Float(1.0),
            layers: vec![
                PortRef::new(red_id, "output".to_string()),
                PortRef::new(green_id, "output".to_string()),
            ],
            ..RasterMultiMerge::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(merge_id, "output".to_string()),
        }),
    );
    connect(&mut graph, red_id, "output", merge_id, "layers");
    connect(&mut graph, green_id, "output", merge_id, "layers");
    connect(&mut graph, merge_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 2);
    // Green on top of red with Normal blend -> green
    for chunk in bitmap.pixels.chunks_exact(4) {
        assert_eq!(chunk, &[0, 255, 0, 255]);
    }
}

#[test]
fn shape_rectangle_renders_through_shape_renderer() {
    let mut graph = Graph::new();
    let shape_id = node_id(1);
    let renderer_id = node_id(2);
    let output_id = node_id(3);

    graph.nodes.insert(
        shape_id,
        NodeKind::Shape(Shape {
            id: shape_id,
            geometry_kind: NodeProperty::Int(0), // Rectangle
            width: NodeProperty::Int(3),
            height: NodeProperty::Int(2),
            fill_enabled: NodeProperty::Bool(true),
            fill_color: NodeProperty::Color([0, 255, 0, 255]),
            ..Shape::default()
        }),
    );
    graph.nodes.insert(
        renderer_id,
        NodeKind::ShapeRenderer(ShapeRenderer {
            id: renderer_id,
            vector: PortRef::new(shape_id, "vector".to_string()),
            ..ShapeRenderer::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(renderer_id, "output".to_string()),
        }),
    );
    connect(&mut graph, shape_id, "vector", renderer_id, "vector");
    connect(&mut graph, renderer_id, "output", output_id, "source");

    let bitmap = render_single(graph, 3, 2);
    assert_eq!(bitmap.storage_width, 3);
    assert_eq!(bitmap.storage_height, 2);
    for chunk in bitmap.pixels.chunks_exact(4) {
        assert_eq!(chunk, &[0, 255, 0, 255]);
    }
}

#[test]
fn shape_renderer_group_avoids_per_child_surface_allocations() {
    let mut graph = Graph::new();
    let left_shape_id = node_id(1);
    let right_shape_id = node_id(2);
    let vector_merge_id = node_id(3);
    let renderer_id = node_id(4);
    let output_id = node_id(5);

    graph.nodes.insert(
        left_shape_id,
        NodeKind::Shape(Shape {
            id: left_shape_id,
            geometry_kind: NodeProperty::Int(0),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
            position: NodeProperty::Vec2((0.0, 0.0)),
            fill_enabled: NodeProperty::Bool(true),
            fill_color: NodeProperty::Color([255, 0, 0, 255]),
            ..Shape::default()
        }),
    );
    graph.nodes.insert(
        right_shape_id,
        NodeKind::Shape(Shape {
            id: right_shape_id,
            geometry_kind: NodeProperty::Int(0),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(2),
            position: NodeProperty::Vec2((2.0, 0.0)),
            fill_enabled: NodeProperty::Bool(true),
            fill_color: NodeProperty::Color([0, 0, 255, 255]),
            ..Shape::default()
        }),
    );
    graph.nodes.insert(
        vector_merge_id,
        NodeKind::VectorMultimerge(VectorMultiMerge {
            id: vector_merge_id,
            layers: vec![
                PortRef::new(left_shape_id, "vector".to_string()),
                PortRef::new(right_shape_id, "vector".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        renderer_id,
        NodeKind::ShapeRenderer(ShapeRenderer {
            id: renderer_id,
            vector: PortRef::new(vector_merge_id, "output".to_string()),
            ..ShapeRenderer::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(renderer_id, "output".to_string()),
        }),
    );
    connect(
        &mut graph,
        left_shape_id,
        "vector",
        vector_merge_id,
        "layers",
    );
    connect(
        &mut graph,
        right_shape_id,
        "vector",
        vector_merge_id,
        "layers",
    );
    connect(&mut graph, vector_merge_id, "output", renderer_id, "vector");
    connect(&mut graph, renderer_id, "output", output_id, "source");

    let composition = Composition::new(
        graph,
        TimelineSettings {
            fps: 30.0,
            duration_frames: 60,
        },
        RenderSettings {
            width: 4,
            height: 2,
            background_color: [0, 0, 0, 0],
        },
    );
    let pool = TestSurfacePool;
    let media = NullMediaStore;
    let mut renderer = LumenRenderer::new(&composition, &pool, &media).unwrap();
    let rendered = renderer.render(0).unwrap();
    pool.flush();

    let stats = pool.stats();
    let bitmap = readback_frame(rendered);
    assert_eq!(bitmap.storage_width, 4);
    assert_eq!(bitmap.storage_height, 2);
    assert_eq!(&bitmap.pixels[0..4], &[255, 0, 0, 255]);
    assert_eq!(&bitmap.pixels[8..12], &[0, 0, 255, 255]);
    assert!(
        stats.fresh_allocations <= 2,
        "expected grouped vector render to stay within one main render target allocation; stats={stats:?}"
    );
}

#[test]
fn graph_validation_rejects_missing_media_output() {
    let graph = Graph::new();
    let errors = graph
        .validate()
        .expect_err("should fail without media output");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::MissingMediaOutput)
    )));
}

#[test]
fn graph_validation_rejects_multiple_media_outputs() {
    let mut graph = Graph::new();
    graph.nodes.insert(
        node_id(1),
        NodeKind::MediaOutput(MediaOutput {
            id: node_id(1),
            source: PortRef::empty(),
        }),
    );
    graph.nodes.insert(
        node_id(2),
        NodeKind::MediaOutput(MediaOutput {
            id: node_id(2),
            source: PortRef::empty(),
        }),
    );
    let errors = graph
        .validate()
        .expect_err("should fail with multiple outputs");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(
            lumen::error::GraphValidationError::MultipleMediaOutputs { .. }
        )
    )));
}

#[test]
fn graph_validation_detects_cycle() {
    let mut graph = Graph::new();
    let a = node_id(1);
    let b = node_id(2);
    let output = node_id(3);

    graph.nodes.insert(
        a,
        NodeKind::Transform(Transform {
            id: a,
            source: PortRef::new(b, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        b,
        NodeKind::Transform(Transform {
            id: b,
            source: PortRef::new(a, "output".to_string()),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        output,
        NodeKind::MediaOutput(MediaOutput {
            id: output,
            source: PortRef::new(a, "output".to_string()),
        }),
    );
    connect(&mut graph, a, "output", b, "source");
    connect(&mut graph, b, "output", a, "source");
    connect(&mut graph, a, "output", output, "source");

    let errors = graph.validate().expect_err("should detect cycle");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::Cycle { .. })
    )));
}

#[test]
fn graph_validation_detects_port_kind_mismatch() {
    let mut graph = Graph::new();
    let shape_id = node_id(1);
    let output_id = node_id(2);

    graph.nodes.insert(
        shape_id,
        NodeKind::Shape(Shape {
            id: shape_id,
            ..Shape::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(shape_id, "vector".to_string()),
        }),
    );
    // Shape outputs Vector, MediaOutput expects Raster
    connect(&mut graph, shape_id, "vector", output_id, "source");

    let errors = graph
        .validate()
        .expect_err("should detect port kind mismatch");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::PortKindMismatch { .. })
    )));
}

#[test]
fn graph_validation_detects_missing_required_input() {
    let mut graph = Graph::new();
    let transform_id = node_id(1);
    let output_id = node_id(2);

    graph.nodes.insert(
        transform_id,
        NodeKind::Transform(Transform {
            id: transform_id,
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(transform_id, "output".to_string()),
        }),
    );
    connect(&mut graph, transform_id, "output", output_id, "source");
    // Transform's "source" input is not connected

    let errors = graph
        .validate()
        .expect_err("should detect missing required input");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::MissingRequiredInput {
            node_id,
            ..
        }) if *node_id == transform_id
    )));
}

#[test]
fn graph_validation_reports_missing_source_port_not_missing_node() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let output_id = node_id(2);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(solid_id, "missing".to_string()),
        }),
    );
    connect(&mut graph, solid_id, "missing", output_id, "source");

    let errors = graph
        .validate()
        .expect_err("should detect missing source port");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::MissingSourcePort {
            node_id,
            port,
        }) if *node_id == solid_id && port == "missing"
    )));
    assert!(!errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::MissingSourceNode {
            node_id,
        }) if *node_id == solid_id
    )));
}

#[test]
fn graph_validation_reports_missing_target_port_not_missing_node() {
    let mut graph = Graph::new();
    let solid_id = node_id(1);
    let output_id = node_id(2);

    graph.nodes.insert(
        solid_id,
        NodeKind::SolidColor(SolidColor {
            id: solid_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::empty(),
        }),
    );
    connect(&mut graph, solid_id, "output", output_id, "missing");

    let errors = graph
        .validate()
        .expect_err("should detect missing target port");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::MissingTargetPort {
            node_id,
            port,
        }) if *node_id == output_id && port == "missing"
    )));
    assert!(!errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::MissingTargetNode {
            node_id,
        }) if *node_id == output_id
    )));
}

#[test]
fn graph_validation_detects_overlapping_switch_ranges() {
    let mut graph = Graph::new();
    let switch_id = node_id(1);
    let output_id = node_id(2);

    let mut map = HashMap::new();
    map.insert(0u16, 0u32..10u32);
    map.insert(1u16, 5u32..15u32);

    graph.nodes.insert(
        switch_id,
        NodeKind::Switch(Switch {
            id: switch_id,
            map,
            layers: Vec::new(),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(switch_id, "output".to_string()),
        }),
    );
    connect(&mut graph, switch_id, "output", output_id, "source");

    let errors = graph
        .validate()
        .expect_err("should detect overlapping switch ranges");
    assert!(errors.iter().any(|e| matches!(
        e,
        LumenError::GraphValidation(lumen::error::GraphValidationError::SwitchRangeOverlap { .. })
    )));
}

#[test]
fn time_remap_evaluates_upstream_at_explicit_frame() {
    let mut graph = Graph::new();
    let red_id = node_id(1);
    let blue_id = node_id(2);
    let switch_id = node_id(3);
    let remap_id = node_id(4);
    let output_id = node_id(5);

    graph.nodes.insert(
        red_id,
        NodeKind::SolidColor(SolidColor {
            id: red_id,
            color: NodeProperty::Color([255, 0, 0, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        blue_id,
        NodeKind::SolidColor(SolidColor {
            id: blue_id,
            color: NodeProperty::Color([0, 0, 255, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );

    let mut map = HashMap::new();
    map.insert(0u16, 0u32..10u32);
    map.insert(1u16, 10u32..20u32);

    graph.nodes.insert(
        switch_id,
        NodeKind::Switch(Switch {
            id: switch_id,
            map,
            layers: vec![
                PortRef::new(red_id, "output".to_string()),
                PortRef::new(blue_id, "output".to_string()),
            ],
        }),
    );
    graph.nodes.insert(
        remap_id,
        NodeKind::TimeRemap(TimeRemap {
            id: remap_id,
            frame: NodeProperty::Float(0.0), // Evaluate frame 0 -> red
            loop_enabled: NodeProperty::Bool(false),
            loop_start: NodeProperty::Int(0),
            loop_end: NodeProperty::Int(0),
            source: PortRef::new(switch_id, "output".to_string()),
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(remap_id, "output".to_string()),
        }),
    );
    connect(&mut graph, red_id, "output", switch_id, "layers");
    connect(&mut graph, blue_id, "output", switch_id, "layers");
    connect(&mut graph, switch_id, "output", remap_id, "source");
    connect(&mut graph, remap_id, "output", output_id, "source");

    // Even though we render frame 15 (which would select blue via switch),
    // time_remap forces evaluation at frame 0, which selects red.
    let bitmap = render_frame(graph, 1, 1, 15);
    assert_eq!(bitmap.pixels.as_slice(), &[255, 0, 0, 255]);
}

#[test]
fn surface_allocator_ping_pongs_between_two_scratch_surfaces() {
    let pool = DefaultSurfacePool::new();
    pool.with_surface(3, 2, |_surface_a| {
        pool.with_surface(3, 2, |_surface_b| Ok(()))
    })
    .expect("nested use should consume both scratch surfaces");

    pool.with_surface(3, 2, |_surface_a| {
        pool.with_surface(3, 2, |_surface_b| Ok(()))
    })
    .expect("second nested use should reuse both scratch surfaces");

    pool.with_surface(3, 2, |_surface_a| {
        pool.with_surface(3, 2, |_surface_b| {
            let err = pool
                .with_surface(3, 2, |_surface_c| Ok(()))
                .expect_err("third nested acquire should fail");
            assert!(err.to_string().contains("scratch surfaces"));
            Ok(())
        })
    })
    .expect("third nested scenario should complete");

    let stats = pool.stats();
    assert_eq!(stats.total_acquires, 7);
    assert_eq!(stats.fresh_allocations, 2);
    assert_eq!(stats.reused_acquires, 4);
}

#[test]
fn boolean_with_mask_preserves_source_dimensions() {
    let mut graph = Graph::new();
    let source_id = node_id(1);
    let mask_id = node_id(2);
    let boolean_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        source_id,
        NodeKind::SolidColor(SolidColor {
            id: source_id,
            color: NodeProperty::Color([10, 20, 30, 255]),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        mask_id,
        NodeKind::SolidColor(SolidColor {
            id: mask_id,
            color: NodeProperty::Color([255, 255, 255, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        boolean_id,
        NodeKind::Boolean(Boolean {
            id: boolean_id,
            source: PortRef::new(source_id, "output".to_string()),
            mask: PortRef::new(mask_id, "output".to_string()),
            ..Boolean::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(boolean_id, "output".to_string()),
        }),
    );
    connect(&mut graph, source_id, "output", boolean_id, "source");
    connect(&mut graph, mask_id, "output", boolean_id, "mask");
    connect(&mut graph, boolean_id, "output", output_id, "source");

    let bitmap = render_single(graph, 2, 1);
    assert_eq!(bitmap.storage_width, 2);
    assert_eq!(bitmap.storage_height, 1);
    // First pixel should be preserved (white mask = fully opaque)
    assert_eq!(&bitmap.pixels[0..4], &[10, 20, 30, 255]);
}

#[test]
fn boolean_raster_mask_respects_transformed_mask_domain() {
    let mut graph = Graph::new();
    let source_id = node_id(1);
    let mask_id = node_id(2);
    let mask_transform_id = node_id(3);
    let boolean_id = node_id(4);
    let output_id = node_id(5);

    graph.nodes.insert(
        source_id,
        NodeKind::SolidColor(SolidColor {
            id: source_id,
            color: NodeProperty::Color([40, 80, 120, 255]),
            width: NodeProperty::Int(3),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        mask_id,
        NodeKind::SolidColor(SolidColor {
            id: mask_id,
            color: NodeProperty::Color([255, 255, 255, 255]),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        mask_transform_id,
        NodeKind::Transform(Transform {
            id: mask_transform_id,
            source: PortRef::new(mask_id, "output".to_string()),
            translate_x: NodeProperty::Float(1.0),
            sampling: NodeProperty::Int(0),
            ..Transform::default()
        }),
    );
    graph.nodes.insert(
        boolean_id,
        NodeKind::Boolean(Boolean {
            id: boolean_id,
            source: PortRef::new(source_id, "output".to_string()),
            mask: PortRef::new(mask_transform_id, "output".to_string()),
            ..Boolean::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(boolean_id, "output".to_string()),
        }),
    );
    connect(&mut graph, source_id, "output", boolean_id, "source");
    connect(&mut graph, mask_id, "output", mask_transform_id, "source");
    connect(&mut graph, mask_transform_id, "output", boolean_id, "mask");
    connect(&mut graph, boolean_id, "output", output_id, "source");

    let bitmap = render_single(graph, 3, 1);
    assert_eq!(pixel_at(&bitmap, 0, 0), &[0, 0, 0, 0]);
    assert_eq!(pixel_at(&bitmap, 1, 0), &[40, 80, 120, 255]);
    assert_eq!(pixel_at(&bitmap, 2, 0), &[0, 0, 0, 0]);
}

#[test]
fn boolean_vector_mask_respects_rasterized_vector_domain() {
    let mut graph = Graph::new();
    let source_id = node_id(1);
    let shape_id = node_id(2);
    let boolean_id = node_id(3);
    let output_id = node_id(4);

    graph.nodes.insert(
        source_id,
        NodeKind::SolidColor(SolidColor {
            id: source_id,
            color: NodeProperty::Color([70, 90, 110, 255]),
            width: NodeProperty::Int(7),
            height: NodeProperty::Int(1),
        }),
    );
    graph.nodes.insert(
        shape_id,
        NodeKind::Shape(Shape {
            id: shape_id,
            geometry_kind: NodeProperty::Int(0),
            width: NodeProperty::Int(2),
            height: NodeProperty::Int(1),
            position: NodeProperty::Vec2((4.0, 0.0)),
            fill_enabled: NodeProperty::Bool(true),
            fill_color: NodeProperty::Color([255, 255, 255, 255]),
            ..Shape::default()
        }),
    );
    graph.nodes.insert(
        boolean_id,
        NodeKind::Boolean(Boolean {
            id: boolean_id,
            source: PortRef::new(source_id, "output".to_string()),
            vector: PortRef::new(shape_id, "vector".to_string()),
            ..Boolean::default()
        }),
    );
    graph.nodes.insert(
        output_id,
        NodeKind::MediaOutput(MediaOutput {
            id: output_id,
            source: PortRef::new(boolean_id, "output".to_string()),
        }),
    );
    connect(&mut graph, source_id, "output", boolean_id, "source");
    connect(&mut graph, shape_id, "vector", boolean_id, "vector");
    connect(&mut graph, boolean_id, "output", output_id, "source");

    let bitmap = render_single(graph, 7, 1);
    assert_eq!(pixel_at(&bitmap, 1, 0), &[0, 0, 0, 0]);
    assert_eq!(pixel_at(&bitmap, 4, 0), &[70, 90, 110, 255]);
    assert_eq!(pixel_at(&bitmap, 5, 0), &[70, 90, 110, 255]);
}

#[cfg(feature = "json")]
#[test]
fn vector_merge_json_builds_and_evaluates() {
    let json = r#"{
        "timeline": { "fps": 30, "duration_frames": 1 },
        "render_settings": { "width": 4, "height": 1 },
        "nodes": [
            { "id": 1, "type": "shape", "properties": {
                "geometry_kind": 0,
                "width": 2,
                "height": 1,
                "position": [0, 0],
                "fill_enabled": true,
                "fill_color": [255, 0, 0, 255]
            }},
            { "id": 2, "type": "shape", "properties": {
                "geometry_kind": 0,
                "width": 2,
                "height": 1,
                "position": [2, 0],
                "fill_enabled": true,
                "fill_color": [0, 0, 255, 255]
            }},
            { "id": 3, "type": "vector_merge" },
            { "id": 4, "type": "shape_renderer" },
            { "id": 5, "type": "media_output" }
        ],
        "connections": [
            { "from_node": 1, "from_port": "vector", "to_node": 3, "to_port": "base" },
            { "from_node": 2, "from_port": "vector", "to_node": 3, "to_port": "overlay" },
            { "from_node": 3, "from_port": "output", "to_node": 4, "to_port": "vector" },
            { "from_node": 4, "from_port": "output", "to_node": 5, "to_port": "source" }
        ]
    }"#;

    let composition = lumen::json::parse(json).expect("vector_merge JSON should parse");
    let pool = TestSurfacePool;
    let media = NullMediaStore;
    let mut renderer = LumenRenderer::new(&composition, &pool, &media).unwrap();
    let bitmap = readback_frame(renderer.render(0).unwrap());
    pool.flush();

    assert_eq!(pixel_at(&bitmap, 0, 0), &[255, 0, 0, 255]);
    assert_eq!(pixel_at(&bitmap, 3, 0), &[0, 0, 255, 255]);
}
