use serde::{Deserialize, Serialize};

use crate::sequence::element::ElementProperties;

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone, Copy)]
pub enum Font {
    Arial,
    TimesNewRoman,
    Helvetica,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
pub struct TextElement {
    pub font: Font,
    pub text: String,
    pub properties: ElementProperties,
}
