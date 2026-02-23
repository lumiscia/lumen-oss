use skia_safe::{
    Color, FontMgr, FontStyle,
    font_style::{Weight, Width},
    textlayout::{
        FontCollection, ParagraphBuilder, ParagraphStyle, TextAlign as ParagraphTextAlign,
        TextDecoration as ParagraphTextDecoration,
        TextDecorationStyle as ParagraphTextDecorationStyle, TextStyle as ParagraphTextStyle,
    },
};

use crate::clip::{
    Clip, ClipGeometry, ClipMeta,
    style::{StyleContext, TextAlign, TextDecoration, TextOverflow, TextStyle, VerticalAlign},
};
use crate::render::backend::RenderError;
use crate::render::context::{FrameContext, RendererContext};

#[derive(Debug, Clone)]
pub struct TextClip {
    pub meta: ClipMeta,
    pub geometry: ClipGeometry,
    pub content: String,
    pub style: TextStyle,
}

impl TextClip {
    pub fn new(
        meta: ClipMeta,
        geometry: ClipGeometry,
        content: impl Into<String>,
        style: TextStyle,
    ) -> Self {
        Self {
            meta,
            geometry,
            content: content.into(),
            style,
        }
    }

    pub fn with_default_geometry(
        meta: ClipMeta,
        content: impl Into<String>,
        style: TextStyle,
    ) -> Self {
        Self::new(meta, ClipGeometry::default(), content, style)
    }

    pub fn with_geometry(mut self, geometry: ClipGeometry) -> Self {
        self.geometry = geometry;
        self
    }
}

impl TextClip {
    fn build_paragraph(
        &self,
        style_ctx: &StyleContext,
        font_collection: FontCollection,
        layout_width: f32,
    ) -> skia_safe::textlayout::Paragraph {
        let mut para_style = ParagraphStyle::new();
        para_style.set_text_align(match self.style.text_align {
            TextAlign::Left => ParagraphTextAlign::Left,
            TextAlign::Center => ParagraphTextAlign::Center,
            TextAlign::Right => ParagraphTextAlign::Right,
            TextAlign::Justify => ParagraphTextAlign::Justify,
        });

        para_style.set_max_lines(self.style.max_lines.map(|lines| lines.max(1) as usize));
        if matches!(self.style.overflow, TextOverflow::Ellipsis) {
            para_style.set_ellipsis("…");
        }

        let mut text_style = ParagraphTextStyle::new();
        let font_size = self.style.font_size.resolve_or(style_ctx, 16.0).max(1.0);
        let font_weight = self
            .style
            .font_weight
            .resolve_or(style_ctx, 400)
            .clamp(100, 900);
        let line_height = self.style.line_height.resolve_or(style_ctx, 1.2).max(0.5);
        let letter_spacing = self.style.letter_spacing.resolve_or(style_ctx, 0.0);

        text_style.set_font_size(font_size);
        text_style.set_color(Color::from_argb(
            self.style.color[3].resolve_or(style_ctx, 255),
            self.style.color[0].resolve_or(style_ctx, 0),
            self.style.color[1].resolve_or(style_ctx, 0),
            self.style.color[2].resolve_or(style_ctx, 0),
        ));

        if !self.style.font_family.trim().is_empty() {
            // Unknown families gracefully fall back via FontCollection's default FontMgr.
            text_style.set_font_families(&[self.style.font_family.as_str()]);
        }
        text_style.set_font_style(FontStyle::new(
            Weight::from(font_weight as i32),
            Width::NORMAL,
            self.style.font_style,
        ));
        text_style.set_height(line_height);
        text_style.set_height_override(true);
        text_style.set_letter_spacing(letter_spacing);

        let mut decorations = ParagraphTextDecoration::NO_DECORATION;
        if matches!(
            self.style.decoration,
            TextDecoration::Underline | TextDecoration::UnderlineStrikethrough
        ) {
            decorations |= ParagraphTextDecoration::UNDERLINE;
        }
        if matches!(
            self.style.decoration,
            TextDecoration::Strikethrough | TextDecoration::UnderlineStrikethrough
        ) {
            decorations |= ParagraphTextDecoration::LINE_THROUGH;
        }
        text_style.set_decoration_type(decorations);
        text_style.set_decoration_style(ParagraphTextDecorationStyle::Solid);

        para_style.set_text_style(&text_style);

        let mut builder = ParagraphBuilder::new(&para_style, font_collection);
        builder.push_style(&text_style);
        builder.add_text(&self.content);
        let mut paragraph = builder.build();
        paragraph.layout(layout_width);
        paragraph
    }

    fn resolved_max_width(&self, style_ctx: &StyleContext) -> Option<f32> {
        self.style
            .max_width
            .as_ref()
            .map(|max_width| max_width.resolve_or(style_ctx, f32::MAX))
            .filter(|width| width.is_finite() && *width > 0.0)
    }

    pub fn measure(&self, available_width: f32, ctx: &StyleContext) -> (f32, f32) {
        let width = if available_width.is_finite() && available_width > 0.0 {
            available_width
        } else {
            f32::MAX
        };
        let mut font_collection = FontCollection::new();
        font_collection.set_default_font_manager(FontMgr::default(), None);

        let paragraph = self.build_paragraph(ctx, font_collection, width);
        (paragraph.longest_line(), paragraph.height())
    }
}

impl Clip for TextClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(
        &self,
        frame: u32,
        frame_ctx: &FrameContext,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        if !self.contains_frame(frame) {
            return Ok(());
        }

        self.style
            .base
            .draw(frame, frame_ctx, renderer_ctx, |renderer_ctx, _resolved| {
                let expression_scope = renderer_ctx.expression_scope().clone();
                let style_ctx = StyleContext::with_scope(frame, &expression_scope);
                let resolved_placeholder =
                    self.style.resolve_placeholder(frame, self.content.as_str());
                let (default_width, default_height) = resolved_placeholder.bounds();
                let geometry = self.geometry.resolve_with_context(
                    &style_ctx,
                    frame_ctx.width as f32 * 0.15,
                    frame_ctx.height as f32 * 0.15,
                    default_width,
                    default_height,
                    0.0,
                    0.0,
                );

                let layout_width = self.resolved_max_width(&style_ctx).unwrap_or(f32::MAX);
                let paragraph =
                    self.build_paragraph(&style_ctx, renderer_ctx.font_collection(), layout_width);

                let offset_y = match self.style.vertical_align {
                    VerticalAlign::Top => 0.0,
                    VerticalAlign::Middle => (geometry.height - paragraph.height()) * 0.5,
                    VerticalAlign::Bottom => geometry.height - paragraph.height(),
                };

                paragraph.paint(
                    renderer_ctx.canvas(),
                    (geometry.left(), geometry.top() + offset_y.max(0.0)),
                );
                Ok(())
            })
    }
}

#[cfg(test)]
mod tests {
    use skia_safe::{BlendMode, font_style::Slant as FontSlant};

    use super::TextClip;
    use crate::clip::{
        Clip, ClipGeometry, ClipMeta,
        style::{
            BaseStyle, StyleContext, StyleProperty, StyleValue, TextAlign, TextDecoration,
            TextOverflow, TextStyle, TransformStyle, VerticalAlign,
        },
    };
    use crate::render::{
        backend::read_surface_rgba,
        context::{FrameContext, RendererContext},
    };
    use crate::time::Rational;

    fn literal<T>(value: T) -> StyleProperty<T> {
        StyleProperty::Value(StyleValue::Literal(value))
    }

    fn base_style() -> BaseStyle {
        BaseStyle {
            visible: literal(true),
            opacity: literal(1.0),
            blend_mode: BlendMode::SrcOver,
            blur: literal(0.0),
            shadows: Vec::new(),
            clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
            transform: TransformStyle {
                translate: [literal(0.0), literal(0.0)],
                scale: [literal(1.0), literal(1.0)],
                rotation: literal(0.0),
                skew: [literal(0.0), literal(0.0)],
                origin: [literal(0.0), literal(0.0)],
            },
            alignment: [literal(0.0), literal(0.0)],
            mask: None,
        }
    }

    fn text_style() -> TextStyle {
        TextStyle {
            base: base_style(),
            font_family: "sans-serif".to_owned(),
            font_size: literal(16.0),
            font_weight: literal(600),
            font_style: FontSlant::Upright,
            color: [literal(220), literal(30), literal(40), literal(255)],
            line_height: literal(1.25),
            letter_spacing: literal(0.0),
            text_align: TextAlign::Left,
            vertical_align: VerticalAlign::Top,
            max_width: None,
            max_lines: None,
            overflow: TextOverflow::Clip,
            decoration: TextDecoration::None,
        }
    }

    fn clip_with_style(content: &str, style: TextStyle) -> TextClip {
        TextClip {
            meta: ClipMeta {
                id: Some("txt".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            geometry: ClipGeometry {
                x: literal(10.0),
                y: literal(10.0),
                width: literal(120.0),
                height: literal(64.0),
                anchor_x: literal(0.0),
                anchor_y: literal(0.0),
            },
            content: content.to_owned(),
            style,
        }
    }

    #[test]
    fn text_clip_new_sets_fields() {
        let style = text_style();
        let clip = TextClip::new(
            ClipMeta {
                id: Some("txt".to_owned()),
                start_frame: 3,
                end_frame: 9,
            },
            ClipGeometry::default(),
            "hello",
            style.clone(),
        );

        assert_eq!(clip.meta.id.as_deref(), Some("txt"));
        assert_eq!(clip.meta.start_frame, 3);
        assert_eq!(clip.meta.end_frame, 9);
        assert_eq!(clip.content, "hello");
        assert_eq!(clip.style.font_family, style.font_family);
    }

    #[test]
    fn text_clip_with_default_geometry_starts_from_default() {
        let clip = TextClip::with_default_geometry(
            ClipMeta {
                id: Some("txt".to_owned()),
                start_frame: 0,
                end_frame: 0,
            },
            "hello",
            text_style(),
        );

        assert_eq!(clip.geometry, ClipGeometry::default());
    }

    fn frame_context() -> FrameContext {
        FrameContext {
            frame: 0,
            time_seconds: 0.0,
            width: 160,
            height: 120,
            device_scale: 1.0,
        }
    }

    #[test]
    fn text_clip_draws_non_transparent_pixels() {
        let mut renderer_ctx =
            RendererContext::new(160, 120, Rational::new(30, 1)).expect("renderer context");
        renderer_ctx.clear();

        let clip = clip_with_style("Hello", text_style());
        clip.draw(0, &frame_context(), &mut renderer_ctx)
            .expect("text clip should draw");

        let pixels = read_surface_rgba(&mut renderer_ctx).expect("readback");
        let mut alpha_pixels = 0usize;
        for y in 10..74 {
            for x in 10..130 {
                let idx = (y * 160 + x) * 4 + 3;
                if pixels[idx] > 0 {
                    alpha_pixels += 1;
                }
            }
        }
        assert!(alpha_pixels > 0);
    }

    #[test]
    fn text_measure_returns_positive_dimensions() {
        let clip = clip_with_style("Hello", text_style());
        let style_ctx = StyleContext::new(0);

        let (width, height) = clip.measure(200.0, &style_ctx);
        assert!(width > 0.0);
        assert!(height > 0.0);
    }

    #[test]
    fn text_measure_wrapping_increases_height() {
        let mut style = text_style();
        style.max_width = Some(literal(48.0));
        let clip = clip_with_style("wrap me over many characters", style);
        let style_ctx = StyleContext::new(0);

        let (_, wrapped_height) = clip.measure(48.0, &style_ctx);
        let (_, single_line_height) = clip.measure(1_000.0, &style_ctx);
        assert!(wrapped_height > single_line_height);
    }

    #[test]
    fn text_measure_max_lines_one_with_ellipsis_limits_height() {
        let style_ctx = StyleContext::new(0);

        let mut ellipsized_style = text_style();
        ellipsized_style.max_width = Some(literal(48.0));
        ellipsized_style.max_lines = Some(1);
        ellipsized_style.overflow = TextOverflow::Ellipsis;
        let ellipsized = clip_with_style("this is a long line that should wrap", ellipsized_style);

        let mut unlimited_style = text_style();
        unlimited_style.max_width = Some(literal(48.0));
        let unlimited = clip_with_style("this is a long line that should wrap", unlimited_style);

        let (_, ellipsized_height) = ellipsized.measure(48.0, &style_ctx);
        let (_, unlimited_height) = unlimited.measure(48.0, &style_ctx);
        assert!(ellipsized_height < unlimited_height);
    }
}
