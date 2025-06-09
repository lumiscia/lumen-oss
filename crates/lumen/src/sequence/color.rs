use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub struct ColorRGBA(u8, u8, u8, u8);

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
}
