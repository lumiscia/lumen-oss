use cosmic_text::SubpixelBin;
use swash::zeno::Command;

use crate::{GlyphKey, TextSystem, msdf};

/// One step of a glyph outline, in pixels with Y pointing up from the glyph's pen position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutlineCommand {
    MoveTo([f32; 2]),
    LineTo([f32; 2]),
    QuadTo([f32; 2], [f32; 2]),
    CurveTo([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

/// What a glyph's font has to outline it with.
#[derive(Debug, Clone, PartialEq)]
pub enum GlyphOutline {
    /// The unhinted outline, at the size the glyph was shaped at and in its font's weight and
    /// variation. Empty for a glyph with nothing to draw, such as whitespace.
    Commands(Vec<OutlineCommand>),
    /// The glyph is drawn from colour or bitmap tables, such as an emoji's, or its font is gone.
    Unavailable,
}

impl GlyphKey {
    /// This key without its subpixel bins, which leave the outline as it is.
    pub fn outline_key(self) -> Self {
        Self(cosmic_text::CacheKey {
            x_bin: SubpixelBin::Zero,
            y_bin: SubpixelBin::Zero,
            ..self.0
        })
    }
}

impl TextSystem {
    /// The outline of the glyph `key` from the layout these fonts shaped.
    pub fn glyph_outline(&mut self, key: GlyphKey) -> GlyphOutline {
        let key = key.outline_key().0;
        let plain = self
            .font_system
            .get_font(key.font_id, key.font_weight)
            .is_some_and(|font| !msdf::has_colour_tables(&font));
        if !plain {
            return GlyphOutline::Unavailable;
        }
        let Some(commands) = self
            .swash_cache
            .get_outline_commands_uncached(&mut self.font_system, key)
        else {
            return GlyphOutline::Unavailable;
        };
        let point = |p: swash::zeno::Vector| [p.x, p.y];
        GlyphOutline::Commands(
            commands
                .iter()
                .map(|command| match *command {
                    Command::MoveTo(p) => OutlineCommand::MoveTo(point(p)),
                    Command::LineTo(p) => OutlineCommand::LineTo(point(p)),
                    Command::QuadTo(c, p) => OutlineCommand::QuadTo(point(c), point(p)),
                    Command::CurveTo(c0, c1, p) => {
                        OutlineCommand::CurveTo(point(c0), point(c1), point(p))
                    }
                    Command::Close => OutlineCommand::Close,
                })
                .collect(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextLayoutRequest;

    #[test]
    fn letters_have_outlines_and_spaces_have_empty_ones() {
        let mut system = TextSystem::new();
        let mut request = TextLayoutRequest::new("o o");
        request.spans[0].font_size = 100.0;
        let layout = system.shape(&request).place([0.3, 0.0]);
        let outlines: Vec<_> = layout
            .glyphs
            .iter()
            .map(|glyph| system.glyph_outline(glyph.key))
            .collect();
        let GlyphOutline::Commands(o) = &outlines[0] else {
            panic!("an `o` has an outline");
        };
        // An `o` is an outer contour and its counter.
        let contours = o
            .iter()
            .filter(|command| matches!(command, OutlineCommand::MoveTo(_)))
            .count();
        assert_eq!(contours, 2);
        assert_eq!(outlines[1], GlyphOutline::Commands(Vec::new()));
        assert_eq!(
            outlines[0], outlines[2],
            "the outline ignores subpixel bins"
        );
    }
}
