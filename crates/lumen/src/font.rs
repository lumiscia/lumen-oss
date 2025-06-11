use skia_safe::Font;

pub const FONT_ARIAL: &str = "Arial";

pub trait FontManager {
    fn named(&self, name: &str) -> Option<Font>;

    fn arial(&self) -> Option<Font> {
        self.named(FONT_ARIAL)
    }
}
