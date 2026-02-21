use crate::expr::Scalar;
use serde::{Deserialize, Serialize};

use crate::time::Rational;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub canvas: Canvas,
    pub timeline: Timeline,
    #[serde(default)]
    pub sources: Vec<Source>,
    #[serde(default)]
    pub layers: Vec<Layer>,
    #[serde(default)]
    pub audio: AudioMix,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_background")]
    pub background: ColorRgba,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Timeline {
    pub fps: Rational,
    pub total_frames: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub id: String,
    #[serde(flatten)]
    pub kind: SourceKind,
}

impl Source {
    pub fn media_type(&self) -> SourceMediaType {
        self.kind.media_type()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SourceKind {
    File {
        media: SourceMediaType,
        path: String,
    },
    Generator {
        media: SourceMediaType,
        filter: String,
    },
}

impl SourceKind {
    pub fn media_type(&self) -> SourceMediaType {
        match self {
            Self::File { media, .. } => *media,
            Self::Generator { media, .. } => *media,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            Self::File { path, .. } => Some(path.as_str()),
            Self::Generator { .. } => None,
        }
    }

    pub fn generator_filter(&self) -> Option<&str> {
        match self {
            Self::Generator { filter, .. } => Some(filter.as_str()),
            Self::File { .. } => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourceMediaType {
    Video,
    Image,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Layer {
    pub id: String,
    #[serde(default)]
    pub z_index: i32,
    #[serde(default)]
    pub items: Vec<LayerItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayerItem {
    Clip(Clip),
    Group(ClipGroup),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Clip {
    pub id: String,
    pub start_frame: u64,
    pub duration_frames: u64,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default)]
    pub animation: ClipAnimation,
    #[serde(default)]
    pub mask: Option<Box<LayerItem>>,
    pub content: ClipContent,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipGroup {
    pub id: String,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub transform: GroupTransform,
    #[serde(default)]
    pub items: Vec<LayerItem>,
    #[serde(default)]
    pub mask: Option<Box<LayerItem>>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct GroupTransform {
    #[serde(default)]
    pub x: Scalar,
    #[serde(default)]
    pub y: Scalar,
    #[serde(default)]
    pub rotation_degrees: f32,
}

impl Default for GroupTransform {
    fn default() -> Self {
        Self {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            rotation_degrees: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct ClipAnimation {
    #[serde(default)]
    pub opacity: Vec<ScalarKeyframe>,
    #[serde(default)]
    pub x: Vec<ScalarKeyframe>,
    #[serde(default)]
    pub y: Vec<ScalarKeyframe>,
    #[serde(default)]
    pub width: Vec<ScalarKeyframe>,
    #[serde(default)]
    pub height: Vec<ScalarKeyframe>,
    #[serde(default)]
    pub rotation_degrees: Vec<ScalarKeyframe>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ScalarKeyframe {
    pub frame: u64,
    pub value: Scalar,
    #[serde(default)]
    pub duration_frames: u64,
    #[serde(default)]
    pub easing: Easing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Easing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    #[serde(default)]
    pub x: Scalar,
    #[serde(default)]
    pub y: Scalar,
    #[serde(default)]
    pub width: Option<Scalar>,
    #[serde(default)]
    pub height: Option<Scalar>,
    #[serde(default)]
    pub rotation_degrees: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            x: Scalar::Literal(0.0),
            y: Scalar::Literal(0.0),
            width: None,
            height: None,
            rotation_degrees: 0.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipContent {
    Solid { color: ColorRgba },
    Shape(ShapeClip),
    Text(TextClip),
    Image(ImageClip),
    Video(VideoClip),
    Layout(LayoutClip),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ImageClip {
    pub source: String,
    #[serde(default)]
    pub fit: FitMode,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoClip {
    pub source: String,
    #[serde(default)]
    pub pipeline: SourcePipeline,
    #[serde(default)]
    pub fit: FitMode,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextClip {
    pub text: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_text_color")]
    pub color: ColorRgba,
    #[serde(default)]
    pub align: TextAlign,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutClip {
    pub root: LayoutNode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutNode {
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub style: LayoutNodeStyle,
    #[serde(flatten)]
    pub kind: LayoutNodeKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum LayoutNodeKind {
    Container {
        #[serde(default)]
        children: Vec<LayoutNode>,
    },
    Text(LayoutTextNode),
    Image(LayoutImageNode),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutTextNode {
    pub text: String,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_text_color")]
    pub color: ColorRgba,
    #[serde(default = "default_layout_text_align")]
    pub align: TextAlign,
    #[serde(default)]
    pub line_height: Option<f32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutImageNode {
    pub source: String,
    #[serde(default)]
    pub fit: FitMode,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize, Default)]
pub struct LayoutEdges {
    #[serde(default)]
    pub top: f32,
    #[serde(default)]
    pub right: f32,
    #[serde(default)]
    pub bottom: f32,
    #[serde(default)]
    pub left: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutOverflow {
    #[default]
    Visible,
    Hidden,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LayoutNodeStyle {
    #[serde(default)]
    pub display: LayoutDisplay,
    #[serde(default)]
    pub flex_direction: LayoutFlexDirection,
    #[serde(default)]
    pub justify_content: LayoutJustifyContent,
    #[serde(default)]
    pub align_items: LayoutAlignItems,
    #[serde(default)]
    pub align_self: LayoutAlignSelf,
    #[serde(default)]
    pub overflow: LayoutOverflow,
    #[serde(default)]
    pub flex_grow: f32,
    #[serde(default = "default_flex_shrink")]
    pub flex_shrink: f32,
    #[serde(default)]
    pub width: Option<Scalar>,
    #[serde(default)]
    pub height: Option<Scalar>,
    #[serde(default)]
    pub min_width: Option<Scalar>,
    #[serde(default)]
    pub min_height: Option<Scalar>,
    #[serde(default)]
    pub max_width: Option<Scalar>,
    #[serde(default)]
    pub max_height: Option<Scalar>,
    #[serde(default)]
    pub padding: LayoutEdges,
    #[serde(default)]
    pub margin: LayoutEdges,
    #[serde(default)]
    pub gap: f32,
    #[serde(default)]
    pub background: Option<ColorRgba>,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: f32,
}

impl Default for LayoutNodeStyle {
    fn default() -> Self {
        Self {
            display: LayoutDisplay::Flex,
            flex_direction: LayoutFlexDirection::Column,
            justify_content: LayoutJustifyContent::FlexStart,
            align_items: LayoutAlignItems::Stretch,
            align_self: LayoutAlignSelf::Auto,
            overflow: LayoutOverflow::Visible,
            flex_grow: 0.0,
            flex_shrink: default_flex_shrink(),
            width: None,
            height: None,
            min_width: None,
            min_height: None,
            max_width: None,
            max_height: None,
            padding: LayoutEdges::default(),
            margin: LayoutEdges::default(),
            gap: 0.0,
            background: None,
            corner_radius: default_corner_radius(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutDisplay {
    #[default]
    Flex,
    None,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutFlexDirection {
    Row,
    #[default]
    Column,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutJustifyContent {
    #[default]
    FlexStart,
    Center,
    FlexEnd,
    SpaceBetween,
    SpaceAround,
    SpaceEvenly,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutAlignItems {
    #[default]
    Stretch,
    FlexStart,
    Center,
    FlexEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutAlignSelf {
    #[default]
    Auto,
    Stretch,
    FlexStart,
    Center,
    FlexEnd,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ShapeClip {
    #[serde(flatten)]
    pub shape: Shape,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "shape", rename_all = "snake_case")]
pub enum Shape {
    Rectangle {
        fill: ColorRgba,
        #[serde(default)]
        radius: f32,
    },
    Ellipse {
        fill: ColorRgba,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FitMode {
    #[default]
    Contain,
    Cover,
    Fill,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SourcePipeline {
    #[serde(default)]
    pub trim: Option<TrimRange>,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub looping: LoopMode,
}

impl Default for SourcePipeline {
    fn default() -> Self {
        Self {
            trim: None,
            speed: 1.0,
            reverse: false,
            looping: LoopMode::None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TrimRange {
    pub start_frame: u64,
    #[serde(default)]
    pub end_frame: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(tag = "mode", rename_all = "snake_case")]
pub enum LoopMode {
    #[default]
    None,
    Finite {
        count: u32,
    },
    Infinite,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColorRgba(pub u8, pub u8, pub u8, pub u8);

impl ColorRgba {
    pub fn r(self) -> u8 {
        self.0
    }

    pub fn g(self) -> u8 {
        self.1
    }

    pub fn b(self) -> u8 {
        self.2
    }

    pub fn a(self) -> u8 {
        self.3
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct AudioMix {
    #[serde(default)]
    pub tracks: Vec<AudioTrack>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioTrack {
    pub id: String,
    #[serde(default)]
    pub clips: Vec<AudioClip>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AudioClip {
    pub source: String,
    pub start_frame: u64,
    pub duration_frames: u64,
    #[serde(default)]
    pub source_in_frame: u64,
    #[serde(default = "default_volume")]
    pub volume: f32,
}

fn default_background() -> ColorRgba {
    ColorRgba(0, 0, 0, 255)
}

fn default_opacity() -> f32 {
    1.0
}

fn default_speed() -> f32 {
    1.0
}

fn default_corner_radius() -> f32 {
    0.0
}

fn default_volume() -> f32 {
    1.0
}

fn default_font_size() -> f32 {
    48.0
}

fn default_text_color() -> ColorRgba {
    ColorRgba(255, 255, 255, 255)
}

fn default_layout_text_align() -> TextAlign {
    TextAlign::Left
}

fn default_flex_shrink() -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::Project;

    #[test]
    fn rejects_legacy_layer_clips_field() {
        let payload = json!({
            "canvas": {
                "width": 320,
                "height": 180,
                "background": [0, 0, 0, 255]
            },
            "timeline": {
                "fps": { "num": 30, "den": 1 },
                "total_frames": 10
            },
            "sources": [],
            "layers": [
                {
                    "id": "layer_1",
                    "z_index": 0,
                    "clips": []
                }
            ],
            "audio": { "tracks": [] }
        });

        let err = serde_json::from_value::<Project>(payload).expect_err("must fail");
        assert!(err.to_string().contains("unknown field `clips`"));
    }
}
