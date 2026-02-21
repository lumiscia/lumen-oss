#![cfg(feature = "renderer-skia")]

use lumen::{
    Canvas, Clip, ClipContent, ClipGroup, ClipShadow, ColorRgba, FitMode, GroupTransform,
    ImageClip, Layer, LayerItem, LayoutClip, LayoutNode, LayoutNodeKind, LayoutNodeStyle, Project,
    Scalar, Shape, ShapeClip, Source, SourceKind, SourceMediaType, TextAlign, TextClip, Timeline,
    Transform, VideoClip,
    backend::{FrameImage, FrameProvider, ProviderError, RenderBackend},
    compile_project,
    time::Rational,
};

const CANVAS_WIDTH: u32 = 24;
const CANVAS_HEIGHT: u32 = 24;

#[derive(Clone)]
struct TestProvider {
    frame: FrameImage,
}

impl FrameProvider for TestProvider {
    fn image(&mut self, source_id: &str) -> Result<Option<FrameImage>, ProviderError> {
        if source_id == "img" {
            return Ok(Some(self.frame.clone()));
        }
        Ok(None)
    }

    fn video_frame(
        &mut self,
        source_id: &str,
        _source_frame: u64,
    ) -> Result<Option<FrameImage>, ProviderError> {
        if source_id == "vid" {
            return Ok(Some(self.frame.clone()));
        }
        Ok(None)
    }
}

fn white() -> ColorRgba {
    ColorRgba(255, 255, 255, 255)
}

fn shadow() -> ClipShadow {
    ClipShadow {
        offset_x: 8.0,
        offset_y: 0.0,
        blur_sigma: 0.0,
        color: ColorRgba(0, 0, 0, 255),
    }
}

fn base_transform() -> Transform {
    Transform {
        x: Scalar::Literal(2.0),
        y: Scalar::Literal(8.0),
        width: Some(Scalar::Literal(6.0)),
        height: Some(Scalar::Literal(6.0)),
        rotation_degrees: 0.0,
    }
}

fn clip_with_shadow(content: ClipContent) -> Clip {
    Clip {
        id: "clip".to_string(),
        start_frame: 0,
        duration_frames: 1,
        opacity: 1.0,
        transform: base_transform(),
        animation: Default::default(),
        shadow: Some(shadow()),
        mask: None,
        content,
    }
}

fn frame_image(width: u32, height: u32) -> FrameImage {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    for _ in 0..(width * height) {
        rgba.extend_from_slice(&[255, 255, 255, 255]);
    }
    FrameImage::new(width, height, rgba).expect("frame image")
}

fn render_item(item: LayerItem, sources: Vec<Source>) -> Vec<u8> {
    let project = Project {
        canvas: Canvas {
            width: CANVAS_WIDTH,
            height: CANVAS_HEIGHT,
            background: ColorRgba(0, 0, 0, 0),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).expect("fps"),
            total_frames: 1,
        },
        sources,
        layers: vec![Layer {
            id: "layer_1".to_string(),
            z_index: 0,
            items: vec![item],
        }],
        audio: Default::default(),
    };

    let timeline = compile_project(&project).expect("compile");
    let mut renderer =
        lumen::backend::skia::SkiaRenderer::new(CANVAS_WIDTH, CANVAS_HEIGHT).expect("renderer");
    let mut provider = TestProvider {
        frame: frame_image(6, 6),
    };
    renderer
        .render_frame(&timeline, 0, &mut provider)
        .expect("render frame")
}

fn render_clip_pair(content: ClipContent, sources: Vec<Source>) -> (Vec<u8>, Vec<u8>) {
    let with_shadow = clip_with_shadow(content.clone());
    let mut without_shadow = with_shadow.clone();
    without_shadow.shadow = None;

    let frame_without = render_item(LayerItem::Clip(without_shadow), sources.clone());
    let frame_with = render_item(LayerItem::Clip(with_shadow), sources);
    (frame_without, frame_with)
}

fn region_alpha_increase(
    without_shadow: &[u8],
    with_shadow: &[u8],
    x_min: usize,
    x_max: usize,
    y_min: usize,
    y_max: usize,
) -> bool {
    for y in y_min..y_max {
        for x in x_min..x_max {
            let idx = (y * CANVAS_WIDTH as usize + x) * 4 + 3;
            if with_shadow[idx] > without_shadow[idx] {
                return true;
            }
        }
    }
    false
}

fn assert_shadow_appears_to_right(without_shadow: &[u8], with_shadow: &[u8]) {
    assert_ne!(without_shadow, with_shadow);
    assert!(
        region_alpha_increase(without_shadow, with_shadow, 10, 20, 8, 16),
        "expected shadow alpha to increase in right-side region"
    );
}

#[test]
fn applies_shadow_to_solid_clips() {
    let (without_shadow, with_shadow) =
        render_clip_pair(ClipContent::Solid { color: white() }, vec![]);
    assert_shadow_appears_to_right(&without_shadow, &with_shadow);
}

#[test]
fn applies_shadow_to_shape_clips() {
    let (without_shadow, with_shadow) = render_clip_pair(
        ClipContent::Shape(ShapeClip {
            shape: Shape::Rectangle {
                fill: white(),
                radius: 0.0,
            },
        }),
        vec![],
    );
    assert_shadow_appears_to_right(&without_shadow, &with_shadow);
}

#[test]
fn applies_shadow_to_text_clips() {
    let (without_shadow, with_shadow) = render_clip_pair(
        ClipContent::Text(TextClip {
            text: "X".to_string(),
            font_size: 8.0,
            color: white(),
            align: TextAlign::Left,
        }),
        vec![],
    );
    assert_shadow_appears_to_right(&without_shadow, &with_shadow);
}

#[test]
fn applies_shadow_to_image_clips() {
    let sources = vec![Source {
        id: "img".to_string(),
        kind: SourceKind::File {
            media: SourceMediaType::Image,
            path: "unused.png".to_string(),
        },
    }];
    let (without_shadow, with_shadow) = render_clip_pair(
        ClipContent::Image(ImageClip {
            source: "img".to_string(),
            fit: FitMode::Fill,
            corner_radius: 0.0,
        }),
        sources,
    );
    assert_shadow_appears_to_right(&without_shadow, &with_shadow);
}

#[test]
fn applies_shadow_to_video_clips() {
    let sources = vec![Source {
        id: "vid".to_string(),
        kind: SourceKind::File {
            media: SourceMediaType::Video,
            path: "unused.mp4".to_string(),
        },
    }];
    let (without_shadow, with_shadow) = render_clip_pair(
        ClipContent::Video(VideoClip {
            source: "vid".to_string(),
            pipeline: Default::default(),
            fit: FitMode::Fill,
            corner_radius: 0.0,
        }),
        sources,
    );
    assert_shadow_appears_to_right(&without_shadow, &with_shadow);
}

#[test]
fn applies_shadow_to_layout_clips() {
    let (without_shadow, with_shadow) = render_clip_pair(
        ClipContent::Layout(LayoutClip {
            root: LayoutNode {
                id: None,
                style: LayoutNodeStyle {
                    width: Some(Scalar::Literal(6.0)),
                    height: Some(Scalar::Literal(6.0)),
                    background: Some(white()),
                    ..Default::default()
                },
                kind: LayoutNodeKind::Container { children: vec![] },
            },
        }),
        vec![],
    );
    assert_shadow_appears_to_right(&without_shadow, &with_shadow);
}

#[test]
fn applies_group_shadow_after_masked_children() {
    let masked_clip = Clip {
        id: "masked".to_string(),
        start_frame: 0,
        duration_frames: 1,
        opacity: 1.0,
        transform: base_transform(),
        animation: Default::default(),
        shadow: None,
        mask: Some(Box::new(LayerItem::Clip(Clip {
            id: "mask".to_string(),
            start_frame: 0,
            duration_frames: 1,
            opacity: 1.0,
            transform: base_transform(),
            animation: Default::default(),
            shadow: None,
            mask: None,
            content: ClipContent::Shape(ShapeClip {
                shape: Shape::Ellipse { fill: white() },
            }),
        }))),
        content: ClipContent::Solid { color: white() },
    };

    let group_with_shadow = ClipGroup {
        id: "group".to_string(),
        opacity: 1.0,
        transform: GroupTransform::default(),
        items: vec![LayerItem::Clip(masked_clip.clone())],
        shadow: Some(shadow()),
        mask: None,
    };
    let group_without_shadow = ClipGroup {
        shadow: None,
        ..group_with_shadow.clone()
    };

    let without_shadow = render_item(LayerItem::Group(group_without_shadow), vec![]);
    let with_shadow = render_item(LayerItem::Group(group_with_shadow), vec![]);
    assert_shadow_appears_to_right(&without_shadow, &with_shadow);
}
