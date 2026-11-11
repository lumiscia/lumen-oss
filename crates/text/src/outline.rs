use cosmic_text::SubpixelBin;
use swash::zeno::Command;

use crate::{GlyphKey, TextSystem};

/// One step of a glyph outline, in pixels with Y pointing up from the glyph's pen position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum OutlineCommand {
    MoveTo([f32; 2]),
    LineTo([f32; 2]),
    QuadTo([f32; 2], [f32; 2]),
    CurveTo([f32; 2], [f32; 2], [f32; 2]),
    Close,
}

/// What a glyph's font has to outline it with. A font may mix colour glyphs with ordinary ones,
/// which are outlined.
#[derive(Debug, Clone, PartialEq)]
pub enum GlyphOutline {
    /// The unhinted outline, at the size the glyph was shaped at and in its font's weight and
    /// variation. Empty for a glyph with nothing to draw, such as whitespace.
    Commands(Vec<OutlineCommand>),
    /// The glyph is drawn from colour or bitmap tables, such as an emoji's, has no outline, or its
    /// font is gone.
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
            .is_some_and(|font| !is_colour_glyph(&font, key.glyph_id));
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

/// Whether `font` draws `glyph` from its `COLR` layers or a colour bitmap strike, rather than an
/// outline.
fn is_colour_glyph(font: &cosmic_text::Font, glyph: u16) -> bool {
    let swash = font.as_swash();
    swash
        .table(u32::from_be_bytes(*b"COLR"))
        .is_some_and(|colr| colr_has_glyph(colr, glyph))
        || swash.color_strikes().any(|strike| strike.contains(glyph))
}

/// Whether the `COLR` table `colr` has layers for `glyph`, as a version 0 base glyph or a version
/// 1 paint.
fn colr_has_glyph(colr: &[u8], glyph: u16) -> bool {
    let u16_at = |at: usize| {
        Some(u16::from_be_bytes(
            colr.get(at..at.checked_add(2)?)?.try_into().ok()?,
        ))
    };
    let u32_at = |at: usize| {
        Some(u32::from_be_bytes(
            colr.get(at..at.checked_add(4)?)?.try_into().ok()?,
        ))
    };
    // Both lists are sorted by glyph, in records of 6 bytes that start with it.
    let listed = |start: usize, count: usize| {
        let (mut low, mut high) = (0, count.min(colr.len().saturating_sub(start) / 6));
        while low < high {
            let middle = low + (high - low) / 2;
            match start.checked_add(middle * 6).and_then(u16_at) {
                Some(found) if found < glyph => low = middle + 1,
                Some(found) if found > glyph => high = middle,
                Some(_) => return true,
                None => return false,
            }
        }
        false
    };
    let base = u16_at(2)
        .zip(u32_at(4))
        .is_some_and(|(count, start)| listed(start as usize, count as usize));
    let paints = match (u16_at(0), u32_at(14)) {
        (Some(1), Some(list)) if list != 0 => u32_at(list as usize)
            .is_some_and(|count| listed((list as usize).saturating_add(4), count as usize)),
        _ => false,
    };
    base || paints
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::TextLayoutRequest;

    const COLR_FONT: &[u8] = include_bytes!("../assets/roboto/RobotoColrTest.ttf");

    /// `COLR_FONT` with the base glyph count of its `COLR` table set to zero, so that it still has
    /// the table but no colour glyphs.
    fn font_without_colour_glyphs() -> Vec<u8> {
        let mut font = COLR_FONT.to_vec();
        let tables = u16::from_be_bytes([font[4], font[5]]) as usize;
        let record = (0..tables)
            .map(|index| 12 + index * 16)
            .find(|&record| &font[record..record + 4] == b"COLR")
            .expect("the font has a COLR table");
        let table = u32::from_be_bytes(font[record + 8..record + 12].try_into().unwrap()) as usize;
        font[table + 2..table + 4].fill(0);
        font
    }

    fn outline_of(font: Vec<u8>, glyph: &str) -> GlyphOutline {
        let mut system = TextSystem::new();
        system.load_font_data(font);
        let mut request = TextLayoutRequest::new(glyph);
        request.spans[0].font_size = 100.0;
        request.spans[0].font_family = "Roboto COLR Test".to_string();
        let layout = system.shape(&request).place([0.0, 0.0]);
        system.glyph_outline(layout.glyphs[0].key)
    }

    #[test]
    fn a_font_with_colour_tables_outlines_the_glyphs_that_are_not_colour() {
        assert_eq!(
            outline_of(COLR_FONT.to_vec(), "A"),
            GlyphOutline::Unavailable,
            "a colour glyph has no outline to extrude"
        );
        let GlyphOutline::Commands(commands) = outline_of(font_without_colour_glyphs(), "A") else {
            panic!("an `A` that is no colour glyph has an outline");
        };
        let contours = commands
            .iter()
            .filter(|command| matches!(command, OutlineCommand::MoveTo(_)))
            .count();
        assert_eq!(contours, 2);
    }

    #[test]
    fn colr_tables_list_their_glyphs_as_base_glyphs_or_paints() {
        let mut v0 = vec![0, 0, 0, 2, 0, 0, 0, 14, 0, 0, 0, 0, 0, 0];
        v0.extend([0, 3, 0, 0, 0, 1, 0, 9, 0, 1, 0, 1]);
        assert!(colr_has_glyph(&v0, 3) && colr_has_glyph(&v0, 9));
        assert!(!colr_has_glyph(&v0, 4) && !colr_has_glyph(&v0, 0));
        // Version 1 adds a list of paints at the offset in its header.
        let mut v1 = vec![0, 1, 0, 0, 0, 0, 0, 14, 0, 0, 0, 0, 0, 0, 0, 0, 0, 18];
        v1.extend([0, 0, 0, 1, 0, 7, 0, 0, 0, 0]);
        assert!(colr_has_glyph(&v1, 7) && !colr_has_glyph(&v1, 3));
        assert!(!colr_has_glyph(&[], 1) && !colr_has_glyph(&[0, 0, 0, 9], 1));
        // A count of more records than the table holds finds only those it has.
        v1[18..22].fill(0xFF);
        assert!(colr_has_glyph(&v1, 7) && !colr_has_glyph(&v1, 9));
    }

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
