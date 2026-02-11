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
pub struct Layer {
    pub id: String,
    #[serde(default)]
    pub z_index: i32,
    #[serde(default)]
    pub clips: Vec<Clip>,
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
    #[serde(default)]
    pub rotation_degrees: f32,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
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
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageClip {
    pub source: String,
    #[serde(default)]
    pub fit: FitMode,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct VideoClip {
    pub source: String,
    #[serde(default)]
    pub pipeline: SourcePipeline,
    #[serde(default)]
    pub fit: FitMode,
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
pub struct ShapeClip {
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

fn default_volume() -> f32 {
    1.0
}

fn default_font_size() -> f32 {
    48.0
}

fn default_text_color() -> ColorRgba {
    ColorRgba(255, 255, 255, 255)
}
