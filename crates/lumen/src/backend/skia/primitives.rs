// -- Drawing primitives -------------------------------------------------------

#[derive(Debug, Clone, Copy)]
struct LayoutRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

fn layout_rect(
    transform: CompiledTransform,
    source_width: f64,
    source_height: f64,
    fit: FitMode,
) -> LayoutRect {
    let target_width = transform
        .width
        .map(|v| v as f64)
        .unwrap_or(source_width)
        .max(1.0);
    let target_height = transform
        .height
        .map(|v| v as f64)
        .unwrap_or(source_height)
        .max(1.0);

    let (draw_width, draw_height) = match fit {
        FitMode::Fill => (target_width, target_height),
        FitMode::Contain => {
            let scale = (target_width / source_width).min(target_height / source_height);
            (source_width * scale, source_height * scale)
        }
        FitMode::Cover => {
            let scale = (target_width / source_width).max(target_height / source_height);
            (source_width * scale, source_height * scale)
        }
    };

    let x = transform.x as f64 + (target_width - draw_width) / 2.0;
    let y = transform.y as f64 + (target_height - draw_height) / 2.0;

    LayoutRect {
        x,
        y,
        width: draw_width,
        height: draw_height,
    }
}

fn alpha_scaled(alpha: u8, opacity: f32) -> u8 {
    ((alpha as f32) * opacity).round() as u8
}

fn to_sk_color(c: ColorRgba, opacity: f32) -> Color {
    Color::from_argb(alpha_scaled(c.a(), opacity), c.r(), c.g(), c.b())
}

fn to_sk_color4f(c: ColorRgba) -> Color4f {
    Color4f::new(
        c.r() as f32 / 255.0,
        c.g() as f32 / 255.0,
        c.b() as f32 / 255.0,
        c.a() as f32 / 255.0,
    )
}

fn shadow_is_visible(shadow: CompiledClipShadow) -> bool {
    if shadow.color.a() == 0 {
        return false;
    }
    shadow.blur_sigma > f32::EPSILON
        || shadow.offset_x.abs() > f32::EPSILON
        || shadow.offset_y.abs() > f32::EPSILON
}

fn draw_solid(
    canvas: &Canvas,
    transform: CompiledTransform,
    opacity: f32,
    color: ColorRgba,
    blend_mode: BlendMode,
) {
    let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(color, opacity));
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    paint.set_blend_mode(blend_mode);

    canvas.draw_rect(
        Rect::from_xywh(
            rect.x as f32,
            rect.y as f32,
            rect.width as f32,
            rect.height as f32,
        ),
        &paint,
    );
}

fn draw_shape(
    canvas: &Canvas,
    transform: CompiledTransform,
    opacity: f32,
    shape: &ShapeClip,
    blend_mode: BlendMode,
) {
    let rect = layout_rect(transform, 1.0, 1.0, FitMode::Fill);
    let sk_rect = Rect::from_xywh(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );

    match shape.shape {
        Shape::Rectangle { fill, radius } => {
            let mut paint = Paint::default();
            paint.set_color(to_sk_color(fill, opacity));
            paint.set_anti_alias(true);
            paint.set_style(PaintStyle::Fill);
            paint.set_blend_mode(blend_mode);

            if radius > 0.0 {
                let rrect = RRect::new_rect_xy(sk_rect, radius, radius);
                canvas.draw_rrect(rrect, &paint);
            } else {
                canvas.draw_rect(sk_rect, &paint);
            }
        }
        Shape::Ellipse { fill } => {
            let mut paint = Paint::default();
            paint.set_color(to_sk_color(fill, opacity));
            paint.set_anti_alias(true);
            paint.set_style(PaintStyle::Fill);
            paint.set_blend_mode(blend_mode);

            canvas.draw_oval(sk_rect, &paint);
        }
    }
}

fn draw_text(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    transform: CompiledTransform,
    opacity: f32,
    text: &TextClip,
    blend_mode: BlendMode,
) {
    let font_size = text.font_size.max(1.0);
    let font = font_cache
        .entry(font_size.to_bits())
        .or_insert_with(|| Font::from_typeface(typeface, font_size));

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(text.color, opacity));
    paint.set_anti_alias(true);
    paint.set_blend_mode(blend_mode);

    let (_, metrics) = font.metrics();
    let line_height = (metrics.descent - metrics.ascent + metrics.leading).max(font_size);

    let lines: Vec<&str> = text.text.lines().collect();
    let lines = if lines.is_empty() { vec![""] } else { lines };

    let line_widths: Vec<f32> = lines
        .iter()
        .map(|line| {
            let (width, _) = font.measure_str(line, None);
            width
        })
        .collect();

    let width_max = line_widths.iter().cloned().fold(0.0f32, f32::max);
    let target_width = transform.width.unwrap_or(width_max.max(1.0));

    let mut y_cursor = transform.y;
    let has_rotation = transform.rotation_degrees != 0.0;

    if has_rotation {
        canvas.save();
        let cx = transform.x + target_width / 2.0;
        let cy = transform.y;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    for (i, line) in lines.iter().enumerate() {
        let x = match text.align {
            TextAlign::Left => transform.x,
            TextAlign::Center => transform.x + (target_width - line_widths[i]) / 2.0,
            TextAlign::Right => transform.x + (target_width - line_widths[i]),
        };

        canvas.draw_str(line, Point::new(x, y_cursor + font_size), font, &paint);
        y_cursor += line_height;
    }

    if has_rotation {
        canvas.restore();
    }
}

fn draw_image(
    canvas: &Canvas,
    transform: CompiledTransform,
    opacity: f32,
    fit: FitMode,
    corner_radius: f32,
    image: &FrameImage,
    blend_mode: BlendMode,
) {
    let info = ImageInfo::new(
        (image.width as i32, image.height as i32),
        ColorType::RGBA8888,
        skia_safe::AlphaType::Unpremul,
        None,
    );

    let row_bytes = image.width as usize * 4;
    // SAFETY: `image.rgba` is borrowed from the FrameProvider and outlives
    // both the `Data` and the `sk_image` created from it – they are local
    // to this function and dropped before it returns.
    let data = unsafe { Data::new_bytes(&image.rgba) };
    let sk_image = match images::raster_from_data(&info, data, row_bytes) {
        Some(img) => img,
        None => return,
    };

    let rect = layout_rect(transform, image.width as f64, image.height as f64, fit);
    let dst_rect = Rect::from_xywh(
        rect.x as f32,
        rect.y as f32,
        rect.width as f32,
        rect.height as f32,
    );

    let mut paint = Paint::default();
    paint.set_alpha_f(opacity);
    paint.set_blend_mode(blend_mode);

    // Fast path: the common case (video/image layers) doesn't need a local
    // canvas transform or clip stack.
    if transform.rotation_degrees == 0.0 && corner_radius <= 0.0 {
        canvas.draw_image_rect(&sk_image, None, dst_rect, &paint);
        return;
    }

    let scale_x = rect.width as f32 / image.width as f32;
    let scale_y = rect.height as f32 / image.height as f32;

    canvas.save();

    if transform.rotation_degrees != 0.0 {
        let cx = rect.x as f32 + rect.width as f32 / 2.0;
        let cy = rect.y as f32 + rect.height as f32 / 2.0;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    canvas.translate(Point::new(rect.x as f32, rect.y as f32));
    canvas.scale((scale_x, scale_y));

    if corner_radius > 0.0 {
        let min_scale = scale_x.abs().min(scale_y.abs()).max(f32::EPSILON);
        let source_radius =
            (corner_radius / min_scale).min((image.width.min(image.height) as f32) * 0.5);
        let clip = RRect::new_rect_xy(
            Rect::from_xywh(0.0, 0.0, image.width as f32, image.height as f32),
            source_radius,
            source_radius,
        );
        canvas.clip_rrect(clip, None, Some(true));
    }

    canvas.draw_image(&sk_image, Point::new(0.0, 0.0), Some(&paint));
    canvas.restore();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn contain_fit_keeps_aspect_ratio() {
        let rect = layout_rect(
            CompiledTransform {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                height: Some(100.0),
                rotation_degrees: 0.0,
            },
            100.0,
            100.0,
            FitMode::Contain,
        );
        assert_eq!(rect.width, 100.0);
        assert_eq!(rect.height, 100.0);
        assert_eq!(rect.x, 50.0);
    }

    #[test]
    fn cover_fit_expands_aspect_ratio() {
        let rect = layout_rect(
            CompiledTransform {
                x: 0.0,
                y: 0.0,
                width: Some(200.0),
                height: Some(100.0),
                rotation_degrees: 0.0,
            },
            100.0,
            100.0,
            FitMode::Cover,
        );
        assert_eq!(rect.width, 200.0);
        assert_eq!(rect.height, 200.0);
        assert_eq!(rect.y, -50.0);
    }

    #[test]
    fn renderer_creates_successfully() {
        let renderer = SkiaRenderer::new(320, 240);
        assert!(renderer.is_ok());
    }
}
