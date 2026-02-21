use serde::{Deserialize, Serialize};

use super::{BaseStyle, LayoutNode, StyleValue};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct Layer {
    pub id: String,
    #[serde(default)]
    pub items: Vec<LayerItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum LayerItem {
    Clip(ClipItem),
    Group(GroupItem),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ClipItem {
    pub id: String,
    pub start_frame: u64,
    pub duration_frames: u64,
    pub content: ClipContent,
    #[serde(default)]
    pub style: ClipStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<LayerItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct GroupItem {
    pub id: String,
    #[serde(default)]
    pub items: Vec<LayerItem>,
    #[serde(default)]
    pub style: ClipStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mask: Option<Box<LayerItem>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum ClipContent {
    Solid,
    Shape { geometry: ShapeGeometry },
    Text { content: String },
    Image { source: String },
    Video {
        source: String,
        #[serde(default)]
        pipeline: VideoPipeline,
    },
    Layout { root: LayoutNode },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct ClipStyle {
    #[serde(flatten)]
    pub base: BaseStyle,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fill: Option<[u8; 4]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fit: Option<FitMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub font_size: Option<StyleValue>,
}

impl Default for ClipStyle {
    fn default() -> Self {
        Self {
            base: BaseStyle::default(),
            fill: None,
            fit: None,
            font_size: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    Cover,
    Contain,
    Fill,
    None,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct VideoPipeline {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trim: Option<TrimRange>,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default)]
    pub r#loop: LoopMode,
}

impl Default for VideoPipeline {
    fn default() -> Self {
        Self {
            trim: None,
            speed: default_speed(),
            r#loop: LoopMode::None,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct TrimRange {
    pub start_frame: u64,
    pub end_frame: u64,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum LoopMode {
    Label(LoopModeLabel),
    Finite { finite: u32 },
}

impl Default for LoopMode {
    fn default() -> Self {
        Self::Label(LoopModeLabel::None)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum LoopModeLabel {
    None,
    Infinite,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum ShapeGeometry {
    Rect,
    Ellipse,
    Polygon {
        vertices: Vec<PolygonVertex>,
        #[serde(default = "default_polygon_closed")]
        closed: bool,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case", deny_unknown_fields)]
pub struct PolygonVertex {
    pub x: f32,
    pub y: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cp_in: Option<[f32; 2]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cp_out: Option<[f32; 2]>,
}

fn default_speed() -> f32 {
    1.0
}

fn default_polygon_closed() -> bool {
    true
}
