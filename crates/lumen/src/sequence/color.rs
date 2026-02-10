use serde::{Deserialize, Serialize};
use skia_safe::{Color3f, Color4f};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub struct ColorRGBA(pub u8, pub u8, pub u8, pub u8);

impl ColorRGBA {
    #[inline]
    pub fn r(&self) -> u8 {
        self.0
    }

    #[inline]
    pub fn g(&self) -> u8 {
        self.1
    }

    #[inline]
    pub fn b(&self) -> u8 {
        self.2
    }

    #[inline]
    pub fn a(&self) -> u8 {
        self.3
    }

    #[inline]
    pub fn as_color4f(&self) -> Color4f {
        self.into()
    }

    #[inline]
    pub fn as_color3f(&self) -> Color3f {
        self.into()
    }
}

impl Into<Color4f> for &ColorRGBA {
    fn into(self) -> Color4f {
        Color4f {
            r: self.r() as f32 / 256.0,
            g: self.g() as f32 / 256.0,
            b: self.b() as f32 / 256.0,
            a: self.a() as f32 / 256.0,
        }
    }
}

impl Into<Color3f> for &ColorRGBA {
    fn into(self) -> Color3f {
        Color3f {
            x: self.r() as f32 / 256.0,
            y: self.g() as f32 / 256.0,
            z: self.b() as f32 / 256.0,
        }
    }
}
