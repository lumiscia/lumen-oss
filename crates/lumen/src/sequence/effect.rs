use serde::{Deserialize, Serialize};

use crate::sequence::ColorRGBA;

#[derive(Debug, Serialize, Deserialize, PartialEq, Clone, Copy)]
pub enum Effect {
    DropShadow {
        color: ColorRGBA,
        distance: i32,
        direction: i32,
    },
}
