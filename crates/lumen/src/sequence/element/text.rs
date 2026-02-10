use serde::{Deserialize, Serialize};

use crate::sequence::{ColorRGBA, element::ElementProperties};

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum Font {
    Arial,
    TimesNewRoman,
    Helvetica,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TextElement {
    pub font: Font,
    pub color: ColorRGBA,
    pub text: String,
    pub properties: ElementProperties,
}
