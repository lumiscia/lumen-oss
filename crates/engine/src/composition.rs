use std::collections::BTreeMap;

use crate::{audio::AudioTimeline, error::LumenError, expr::ExpressionValue, graph::Graph};

#[derive(Debug, Clone)]
pub struct TimelineSettings {
    pub fps: f32,
    pub duration_frames: u32,
}

impl TimelineSettings {
    pub fn time_seconds(&self, frame: u32) -> f64 {
        if self.fps <= 0.0 {
            return 0.0;
        }
        frame as f64 / self.fps as f64
    }
}

#[derive(Debug, Clone)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub background_color: [u8; 4],
}

#[derive(Debug, Clone, Default)]
pub struct CompositionMetadata {
    pub name: Option<String>,
}

#[derive(Debug)]
pub struct Composition {
    pub graph: Graph,
    pub timeline: TimelineSettings,
    pub render_settings: RenderSettings,
    pub metadata: Option<CompositionMetadata>,
    pub audio: Option<AudioTimeline>,
    /// Templates that `clip` nodes instantiate, by name.
    pub compositions: BTreeMap<String, CompositionTemplate>,
}

/// A reusable composition with its own canvas, timeline and props. Its node ids are its own, so
/// they never collide with the root's or another template's.
#[derive(Debug)]
pub struct CompositionTemplate {
    pub graph: Graph,
    pub timeline: TimelineSettings,
    pub render_settings: RenderSettings,
    pub props: BTreeMap<String, PropDef>,
}

/// A prop a template declares, which `prop('name')` reads inside it.
#[derive(Debug, Clone)]
pub struct PropDef {
    pub kind: PropKind,
    /// The value an instance that doesn't set the prop gets.
    pub default: ExpressionValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PropKind {
    String,
    Number,
    Color,
    Vec2,
}

impl PropKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "color" => Some(Self::Color),
            "vec2" => Some(Self::Vec2),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Color => "color",
            Self::Vec2 => "vec2",
        }
    }

    /// The value of this kind a prop without a default holds.
    pub fn zero(self) -> ExpressionValue {
        match self {
            Self::String => ExpressionValue::String(String::new()),
            Self::Number => ExpressionValue::Number(0.0),
            Self::Color => ExpressionValue::Color([0.0; 4]),
            Self::Vec2 => ExpressionValue::Vec2((0.0, 0.0)),
        }
    }

    /// `value` when it is of this kind; numbers and vec2 values never convert to strings.
    pub fn check(self, value: ExpressionValue) -> Result<ExpressionValue, String> {
        let matches = matches!(
            (self, &value),
            (Self::String, ExpressionValue::String(_))
                | (Self::Number, ExpressionValue::Number(_))
                | (Self::Color, ExpressionValue::Color(_))
                | (Self::Vec2, ExpressionValue::Vec2(_))
        );
        if matches {
            Ok(value)
        } else {
            Err(format!(
                "expected a {}, got {}",
                self.name(),
                value.type_name()
            ))
        }
    }

    /// Reads a JSON literal of this kind: a string, a number, a `[r, g, b(, a)]` array or
    /// `#rrggbb(aa)` string, or a `[x, y]` array.
    pub fn literal(self, value: &serde_json::Value) -> Result<ExpressionValue, String> {
        let invalid = || format!("expected a {}, got {value}", self.name());
        match self {
            Self::String => value
                .as_str()
                .map(|text| ExpressionValue::String(text.to_string()))
                .ok_or_else(invalid),
            Self::Number => value
                .as_f64()
                .map(ExpressionValue::Number)
                .ok_or_else(invalid),
            Self::Color => color_literal(value)
                .map(ExpressionValue::color_from_u8)
                .ok_or_else(invalid),
            Self::Vec2 => match value.as_array().map(Vec::as_slice) {
                Some([x, y]) => x
                    .as_f64()
                    .zip(y.as_f64())
                    .map(ExpressionValue::Vec2)
                    .ok_or_else(invalid),
                _ => Err(invalid()),
            },
        }
    }
}

fn color_literal(value: &serde_json::Value) -> Option<[u8; 4]> {
    if let Some(hex) = value.as_str().and_then(|text| text.strip_prefix('#')) {
        if !matches!(hex.len(), 6 | 8) || !hex.is_ascii() {
            return None;
        }
        let mut color = [255; 4];
        for (index, channel) in color.iter_mut().take(hex.len() / 2).enumerate() {
            *channel = u8::from_str_radix(&hex[index * 2..index * 2 + 2], 16).ok()?;
        }
        return Some(color);
    }
    let channels = value
        .as_array()
        .filter(|items| matches!(items.len(), 3 | 4))?;
    let mut color = [255; 4];
    for (channel, item) in color.iter_mut().zip(channels) {
        *channel = u8::try_from(item.as_u64()?).ok()?;
    }
    Some(color)
}

unsafe impl Sync for Composition {}
unsafe impl Send for Composition {}

impl Composition {
    pub fn new(graph: Graph, timeline: TimelineSettings, render_settings: RenderSettings) -> Self {
        Self {
            graph,
            timeline,
            render_settings,
            metadata: None,
            audio: None,
            compositions: BTreeMap::new(),
        }
    }

    pub fn validate_structure(&self) -> Result<(), Vec<LumenError>> {
        let mut errors = Vec::new();

        match self.graph.validate() {
            Ok(_) => {}
            Err(graph_errors) => errors.extend(graph_errors),
        }
        for template in self.compositions.values() {
            if let Err(graph_errors) = template.graph.validate() {
                errors.extend(graph_errors);
            }
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}
