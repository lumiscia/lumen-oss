use serde::{Deserialize, Serialize};

use crate::{sequence::ColorRGBA, time::{Rational, Time}};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Sequence {
    pub canvas: Canvas,
    pub timeline: Timeline,
    #[serde(default)]
    pub assets: Vec<Asset>,
    #[serde(default)]
    pub tracks: Vec<Track>,
    #[serde(default)]
    pub audio: AudioGraph,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Canvas {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_background")]
    pub background: ColorRGBA,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Timeline {
    pub fps: Rational,
    pub duration: Time,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Asset {
    pub id: String,
    pub kind: AssetKind,
    pub source: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AssetKind {
    Image,
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub id: String,
    pub kind: TrackKind,
    #[serde(default)]
    pub clips: Vec<TrackClip>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TrackKind {
    Text,
    Shape,
    Image,
    Video,
    Audio,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TrackClip {
    pub id: String,
    pub start: Time,
    pub duration: Time,
    #[serde(default)]
    pub source_in: Option<Time>,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default)]
    pub reverse: bool,
    #[serde(default)]
    pub transform: Transform,
    #[serde(default = "default_opacity")]
    pub opacity: f32,
    #[serde(default)]
    pub blend_mode: BlendMode,
    pub content: ClipContent,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Transform {
    #[serde(default)]
    pub x: f32,
    #[serde(default)]
    pub y: f32,
    #[serde(default)]
    pub width: Option<f32>,
    #[serde(default)]
    pub height: Option<f32>,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            width: None,
            height: None,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Multiply,
    Screen,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClipContent {
    Text(TextContent),
    Shape(ShapeContent),
    AssetRef { asset_id: String },
    Solid { color: ColorRGBA },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ShapeContent {
    Rectangle {
        fill: ColorRGBA,
        #[serde(default)]
        radius: f32,
    },
    Ellipse {
        fill: ColorRGBA,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    pub text: String,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default = "default_font_size")]
    pub font_size: f32,
    #[serde(default = "default_text_color")]
    pub color: ColorRGBA,
    #[serde(default)]
    pub align: TextAlign,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextAlign {
    Left,
    #[default]
    Center,
    Right,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct AudioGraph {
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
    pub asset_id: String,
    pub start: Time,
    pub duration: Time,
    #[serde(default)]
    pub source_in: Option<Time>,
    #[serde(default = "default_volume")]
    pub volume: f32,
}

fn default_background() -> ColorRGBA {
    ColorRGBA(0, 0, 0, 255)
}

fn default_text_color() -> ColorRGBA {
    ColorRGBA(255, 255, 255, 255)
}

fn default_font_size() -> f32 {
    42.0
}

fn default_opacity() -> f32 {
    1.0
}

fn default_volume() -> f32 {
    1.0
}

fn default_speed() -> f32 {
    1.0
}
