use std::collections::HashMap;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonComposition {
    pub schema_revision: String,
    pub graph: JsonGraph,
    pub timeline: JsonTimelineSettings,
    pub render_settings: JsonRenderSettings,
    #[serde(default)]
    pub tracks: Vec<JsonKeyframeTrack>,
    #[serde(default)]
    pub expressions: Vec<JsonExpression>,
    #[serde(default)]
    pub metadata: Option<JsonCompositionMetadata>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonCompositionMetadata {
    #[serde(default)]
    pub name: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonGraph {
    pub nodes: Vec<JsonNode>,
    pub connections: Vec<JsonConnection>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonTimelineSettings {
    pub fps: f32,
    pub duration_frames: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRenderSettings {
    pub width: u32,
    pub height: u32,
    pub background_color: [u8; 4],
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonNode {
    pub id: u64,
    pub kind: JsonNodeKind,
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JsonNodeKind {
    Shape {
        geometry: JsonShapeGeometry,
    },
    ShapeRenderer {
        #[serde(default = "default_color")]
        fill_color: [u8; 4],
        #[serde(default = "default_stroke_color")]
        stroke_color: [u8; 4],
        #[serde(default = "default_stroke_width")]
        stroke_width: f32,
        #[serde(default = "default_true")]
        fill_enabled: bool,
        #[serde(default)]
        stroke_enabled: bool,
    },
    MediaIn {
        kind: JsonMediaInKind,
    },
    SolidColor {
        color: [u8; 4],
        width: Option<u32>,
        height: Option<u32>,
    },
    Text {
        content: String,
        #[serde(default = "default_font_family")]
        font_family: String,
        #[serde(default = "default_font_size")]
        font_size: f32,
        #[serde(default = "default_font_weight")]
        font_weight: u16,
        #[serde(default = "default_font_style")]
        font_style: JsonTextFontStyle,
        max_width: Option<f32>,
        #[serde(default = "default_color")]
        color: [u8; 4],
        #[serde(default)]
        alignment: JsonTextAlignment,
    },
    Transform {
        #[serde(default = "default_one")]
        scale_x: f32,
        #[serde(default = "default_one")]
        scale_y: f32,
        #[serde(default)]
        translate_x: f32,
        #[serde(default)]
        translate_y: f32,
        #[serde(default)]
        rotate: f32,
        #[serde(default)]
        pivot_x: f32,
        #[serde(default)]
        pivot_y: f32,
        #[serde(default = "default_transform_sampling")]
        sampling: JsonTransformSampling,
    },
    Crop {
        x: u32,
        y: u32,
        width: u32,
        height: u32,
    },
    Resize {
        width: u32,
        height: u32,
        mode: JsonResizeMode,
        sampling: JsonResizeSampling,
    },
    Blur {
        radius: f32,
    },
    Shadow {
        color: [u8; 4],
        blur_radius: f32,
        offset_x: f32,
        offset_y: f32,
    },
    Boolean {
        mask_kind: JsonMaskKind,
        #[serde(default)]
        invert: bool,
    },
    Merge {
        #[serde(default = "default_blend_mode")]
        blend_mode: JsonBlendMode,
        #[serde(default = "default_one")]
        opacity: f32,
    },
    Switch {
        map: HashMap<String, JsonRange>,
    },
    FrameHold {
        hold_frame: u32,
    },
    MediaOutput {},
    Memo {
        cache_id: String,
        #[serde(default)]
        allow_expressions: bool,
    },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JsonShapeGeometry {
    Rectangle { width: u32, height: u32 },
    Ellipse { width: u32, height: u32 },
    Polygon { points: Vec<[f32; 2]> },
}

#[derive(Debug, Deserialize)]
#[serde(tag = "media_type", rename_all = "snake_case")]
pub enum JsonMediaInKind {
    Image {
        source: String,
    },
    Video {
        source: String,
        range: Option<JsonRange>,
        #[serde(default = "default_one")]
        speed: f32,
        #[serde(default)]
        loop_mode: JsonLoopMode,
    },
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonLoopMode {
    None,
    Repeat,
    PingPong,
}

impl Default for JsonLoopMode {
    fn default() -> Self {
        Self::None
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonTextAlignment {
    #[serde(default = "default_text_horizontal")]
    pub horizontal: JsonTextAlignmentHorizontal,
    #[serde(default = "default_text_vertical")]
    pub vertical: JsonTextAlignmentVertical,
}

impl Default for JsonTextAlignment {
    fn default() -> Self {
        Self {
            horizontal: JsonTextAlignmentHorizontal::Left,
            vertical: JsonTextAlignmentVertical::Top,
        }
    }
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonTextFontStyle {
    Normal,
    Italic,
    Oblique,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonTextAlignmentHorizontal {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonTextAlignmentVertical {
    Top,
    Middle,
    Bottom,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonResizeMode {
    Stretch,
    Fit,
    Fill,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonResizeSampling {
    Nearest,
    Bilinear,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonTransformSampling {
    Nearest,
    Bilinear,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonMaskKind {
    Alpha,
    Luma,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonBlendMode {
    Normal,
    Multiply,
    Screen,
    Overlay,
    Darken,
    Lighten,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonRange {
    pub start: u32,
    pub end: u32,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonConnection {
    pub from_node: u64,
    pub from_port: JsonPort,
    pub to_node: u64,
    pub to_port: JsonPort,
}

#[derive(Debug, Deserialize)]
#[serde(untagged)]
pub enum JsonPort {
    Named(String),
    Indexed(u16),
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonKeyframeTrack {
    pub id: u64,
    pub node_id: u64,
    pub property_path: String,
    pub value_type: JsonAnimatableType,
    pub keys: Vec<JsonKeyframe>,
    #[serde(default = "default_extrapolation")]
    pub before_extrapolation: JsonExtrapolation,
    #[serde(default = "default_extrapolation")]
    pub after_extrapolation: JsonExtrapolation,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonKeyframe {
    pub time_frame: u32,
    pub value: serde_json::Value,
    pub interpolation: JsonInterpolationMode,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonInterpolationMode {
    Step,
    Linear,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonExtrapolation {
    Hold,
    DefaultValue,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonAnimatableType {
    Float,
    Int,
    Boolean,
    Color,
    Vector2,
    String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JsonExpression {
    pub node_id: u64,
    pub property_path: String,
    pub source: String,
}

fn default_color() -> [u8; 4] {
    [255, 255, 255, 255]
}

fn default_stroke_color() -> [u8; 4] {
    [0, 0, 0, 255]
}

fn default_stroke_width() -> f32 {
    1.0
}

fn default_true() -> bool {
    true
}

fn default_one() -> f32 {
    1.0
}

fn default_font_family() -> String {
    "sans-serif".to_string()
}

fn default_font_size() -> f32 {
    16.0
}

fn default_font_weight() -> u16 {
    400
}

fn default_font_style() -> JsonTextFontStyle {
    JsonTextFontStyle::Normal
}

fn default_text_horizontal() -> JsonTextAlignmentHorizontal {
    JsonTextAlignmentHorizontal::Left
}

fn default_text_vertical() -> JsonTextAlignmentVertical {
    JsonTextAlignmentVertical::Top
}

fn default_blend_mode() -> JsonBlendMode {
    JsonBlendMode::Normal
}

fn default_transform_sampling() -> JsonTransformSampling {
    JsonTransformSampling::Bilinear
}

fn default_extrapolation() -> JsonExtrapolation {
    JsonExtrapolation::Hold
}
