use lumen::{
    font::{self, FontManager},
    skia::{FontMgr, FontStyle, Typeface},
};

pub mod clip;
pub mod decode;
pub mod encode;
pub mod render;

pub(crate) struct ServerFontManager(FontMgr);

impl ServerFontManager {
    pub fn new() -> Self {
        Self(FontMgr::new())
    }
}

impl FontManager for ServerFontManager {
    fn skia(&self) -> &FontMgr {
        &self.0
    }

    fn named(&self, name: &str) -> Option<Typeface> {
        match name {
            font::FONT_ARIAL => self.0.match_family_style(name, FontStyle::normal()),
            _ => None,
        }
    }
}
