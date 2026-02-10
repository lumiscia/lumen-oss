use skia_safe::{FontMgr, Typeface};

pub const FONT_ARIAL: &str = "Arial";

pub trait FontManager {
    fn skia(&self) -> &FontMgr;

    fn named(&self, name: &str) -> Option<Typeface>;

    fn arial(&self) -> Option<Typeface> {
        self.named(FONT_ARIAL)
    }
}
