use lumen::{
    Canvas, Clip, ClipContent, ColorRgba, FitMode, ImageClip, Layer, LayerItem, Project, Scalar,
    Source, SourceKind, SourceMediaType, Timeline, Transform, backend::FrameImage,
    compile::compile_project, time::Rational,
};

struct SolidImageProvider {
    image: FrameImage,
}

impl lumen::backend::FrameProvider for SolidImageProvider {
    fn image(
        &mut self,
        source_id: &str,
    ) -> Result<Option<FrameImage>, lumen::backend::ProviderError> {
        if source_id == "img" {
            return Ok(Some(self.image.clone()));
        }
        Ok(None)
    }
}

fn test_project() -> Project {
    Project {
        canvas: Canvas {
            width: 20,
            height: 20,
            background: ColorRgba(0, 0, 0, 0),
        },
        timeline: Timeline {
            fps: Rational::new(30, 1).expect("fps"),
            total_frames: 1,
        },
        sources: vec![Source {
            id: "img".to_string(),
            kind: SourceKind::File {
                media: SourceMediaType::Image,
                path: "unused.png".to_string(),
            },
        }],
        layers: vec![Layer {
            id: "media".to_string(),
            z_index: 0,
            items: vec![LayerItem::Clip(Clip {
                id: "img_clip".to_string(),
                start_frame: 0,
                duration_frames: 1,
                opacity: 1.0,
                transform: Transform {
                    x: Scalar::Literal(0.0),
                    y: Scalar::Literal(0.0),
                    width: Some(Scalar::Literal(20.0)),
                    height: Some(Scalar::Literal(20.0)),
                    rotation_degrees: 0.0,
                },
                animation: Default::default(),
                mask: None,
                content: ClipContent::Image(ImageClip {
                    source: "img".to_string(),
                    fit: FitMode::Fill,
                    corner_radius: 6.0,
                }),
            })],
        }],
        audio: Default::default(),
    }
}

fn test_image() -> FrameImage {
    let pixel = [255u8, 255u8, 255u8, 255u8];
    let mut rgba = Vec::with_capacity(20 * 20 * 4);
    for _ in 0..(20 * 20) {
        rgba.extend_from_slice(&pixel);
    }
    FrameImage::new(20, 20, rgba).expect("frame image")
}

fn alpha_at(rgba: &[u8], x: usize, y: usize, width: usize) -> u8 {
    rgba[(y * width + x) * 4 + 3]
}

#[cfg(feature = "renderer-skia")]
#[test]
fn skia_corner_radius_clips_media_corners() {
    use lumen::backend::RenderBackend;

    let project = test_project();
    let timeline = compile_project(&project).expect("compile");
    let mut provider = SolidImageProvider {
        image: test_image(),
    };
    let mut renderer = lumen::backend::skia::SkiaRenderer::new(20, 20).expect("skia init");
    let frame = renderer
        .render_frame(&timeline, 0, &mut provider)
        .expect("render");

    assert_eq!(alpha_at(&frame, 0, 0, 20), 0);
    assert_eq!(alpha_at(&frame, 19, 0, 20), 0);
    assert_eq!(alpha_at(&frame, 0, 19, 20), 0);
    assert_eq!(alpha_at(&frame, 19, 19, 20), 0);
    assert!(alpha_at(&frame, 10, 10, 20) > 0);
}
