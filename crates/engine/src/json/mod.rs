//! JSON deserialization for Lumen compositions.
//!
//! Parses a JSON object into a [`Composition`] containing a validated [`Graph`]
//! of renderer-agnostic nodes and connections. The `*Json` structs are the shape of the document;
//! [`CompositionBuilder::build`] checks what their values mean.
//!
//! # Modules
//!
//! - `evaluate` — one param of a node, evaluated at a frame
//! - `property` — the JSON forms params share: a leading `=`, whole numbers and colors

mod evaluate;
mod property;

use std::{collections::BTreeMap, fmt, marker::PhantomData, ops::Deref};

use serde::{
    Deserialize, Deserializer,
    de::{Error as _, MapAccess, Visitor, value::MapAccessDeserializer},
};
#[cfg(feature = "metadata")]
use serde_json::json;
use serde_json::{Number, Value};

use crate::{
    audio::{AudioClip, AudioGain, AudioTimeline, AudioTrack},
    composition::{
        Composition, CompositionBuilder, CompositionTemplate, PropDef, PropKind, RenderSettings,
        Scene, TimelineSettings,
    },
    diagnostic::{Diagnostic, MAX_DIAGNOSTICS},
    error::{At, DocumentError, GraphValidationError, LumenError, Warning, WarningKind},
    expr::FunctionScope,
    gpu::frame_graph::RecordNode,
    graph::{Connection, Graph},
    media::{AssetKind, AssetResolver, Found, MediaLocation, asset_id, resolve_asset_of},
    node::{NodeId, NodeKind, scene::Instance3d},
};

pub use evaluate::{EvaluateRequest, evaluate_param};
pub(crate) use property::{
    StringParam, classify_string, expression_unsupported, integer_mismatch, integral_i64,
    reject_expression,
};
pub use property::{parse_color, try_parse_color};

const DEFAULT_FPS: f32 = 30.0;

/// The `major.minor` of the Lumen release line this engine reads. A composition's
/// `lumenSchemaVersion` must name it, as `major.minor` or `major.minor.patch`.
pub const SCHEMA_VERSION: &str = "0.6";

/// A struct read only from a JSON object. A derived `Deserialize` alone also reads a positional
/// array, which the schema rejects.
struct Object<T>(T);

impl<T> Deref for Object<T> {
    type Target = T;

    fn deref(&self) -> &T {
        &self.0
    }
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Object<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for ObjectVisitor<T> {
            type Value = Object<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object")
            }

            fn visit_map<A: MapAccess<'de>>(self, map: A) -> Result<Object<T>, A::Error> {
                T::deserialize(MapAccessDeserializer::new(map)).map(Object)
            }
        }

        deserializer.deserialize_map(ObjectVisitor(PhantomData))
    }
}

/// The document root. Keys it doesn't name are ignored.
#[derive(Deserialize)]
struct DocumentJson {
    timeline: Object<TimelineJson>,
    render_settings: Object<RenderSettingsJson>,
    #[serde(default)]
    functions: BTreeMap<String, String>,
    #[serde(default)]
    compositions: BTreeMap<String, Object<TemplateJson>>,
    #[serde(default, deserialize_with = "present")]
    audio: Option<Object<AudioJson>>,
    /// Nothing reads `$schema` or `metadata`, but they must hold what the schema says they hold.
    #[serde(rename = "$schema", default, deserialize_with = "present")]
    _schema: Option<String>,
    #[serde(rename = "metadata", default, deserialize_with = "present")]
    _metadata: Option<Object<MetadataJson>>,
    nodes: Vec<Object<NodeJson>>,
    connections: Vec<Object<ConnectionJson>>,
}

#[cfg(feature = "metadata")]
impl DocumentJson {
    fn schema() -> Value {
        let lumen_version = SCHEMA_VERSION;
        let lumen_version_pattern = format!("^{}(\\.[0-9]+)?$", lumen_version.replace('.', "\\."));
        json!({
            "type": "object",
            "required": ["timeline", "render_settings", "nodes", "connections"],
            "additionalProperties": true,
            "properties": {
                "$schema": {
                    "description": "URL of the composition schema, for editors.",
                    "type": "string"
                },
                "lumenSchemaVersion": {
                    "description": format!("The Lumen major.minor the composition was written for, such as `{lumen_version}`; an engine reads only its own line. Absent means the engine's own version."),
                    "type": "string",
                    "pattern": lumen_version_pattern
                },
                "metadata": MetadataJson::schema(),
                "timeline": { "$ref": "#/$defs/timeline" },
                "audio": { "$ref": "#/$defs/audio" },
                "render_settings": { "$ref": "#/$defs/renderSettings" },
                "functions": {
                    "description": "Expression functions keyed by signature, such as `rise(start)`. Calls expand where they're used.",
                    "type": "object",
                    "x-lumen-keyType": "`${string}(${string})`",
                    "propertyNames": {
                        "pattern": "^\\s*[A-Za-z_][A-Za-z0-9_]*\\s*\\(\\s*([A-Za-z_][A-Za-z0-9_]*\\s*(,\\s*[A-Za-z_][A-Za-z0-9_]*\\s*)*)?\\)\\s*$"
                    },
                    "additionalProperties": { "type": "string" }
                },
                "compositions": {
                    "description": "Reusable compositions by name, which `clip` nodes play. Node ids are scoped to each template.",
                    "type": "object",
                    "additionalProperties": { "$ref": "#/$defs/compositionTemplate" }
                },
                "nodes": {
                    "type": "array",
                    "items": {}
                },
                "connections": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/connection" }
                }
            }
        })
    }
}

/// Keys other than `name` are ignored.
#[derive(Deserialize)]
struct MetadataJson {
    #[serde(rename = "name", default, deserialize_with = "present")]
    _name: Option<String>,
}

#[cfg(feature = "metadata")]
impl MetadataJson {
    fn schema() -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "description": "Display name of the composition.", "type": "string" }
            },
            "additionalProperties": true
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TimelineJson {
    #[serde(default, deserialize_with = "present")]
    fps: Option<f32>,
    #[serde(deserialize_with = "whole_u32")]
    duration_frames: u32,
}

impl TimelineJson {
    fn settings(&self, default_fps: f32) -> TimelineSettings {
        TimelineSettings {
            fps: self.fps.unwrap_or(default_fps),
            duration_frames: self.duration_frames,
        }
    }
}

#[cfg(feature = "metadata")]
impl TimelineJson {
    fn schema() -> Value {
        json!({
            "x-lumen-type": "Timeline",
            "description": "Length and frame rate. `fps` defaults to 30 at the root, and a template's timeline takes the root's `fps`.",
            "type": "object",
            "required": ["duration_frames"],
            "properties": {
                "fps": { "type": "number", "exclusiveMinimum": 0 },
                "duration_frames": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 4294967295u32
                }
            },
            "additionalProperties": false
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenderSettingsJson {
    #[serde(deserialize_with = "whole_u32")]
    width: u32,
    #[serde(deserialize_with = "whole_u32")]
    height: u32,
}

impl RenderSettingsJson {
    fn settings(&self) -> RenderSettings {
        RenderSettings {
            width: self.width,
            height: self.height,
        }
    }
}

#[cfg(feature = "metadata")]
impl RenderSettingsJson {
    fn schema() -> Value {
        json!({
            "x-lumen-type": "RenderSettings",
            "description": "The canvas size in pixels.",
            "type": "object",
            "required": ["width", "height"],
            "properties": {
                "width": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 },
                "height": { "type": "integer", "minimum": 1, "maximum": 4294967295u32 }
            },
            "additionalProperties": false
        })
    }
}

/// A composition a `clip` plays. A template without `render_settings` takes the root's size, and
/// without `timeline` the root's timeline; a `timeline` without `fps` takes the root's fps.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TemplateJson {
    #[serde(default, deserialize_with = "present")]
    render_settings: Option<Object<RenderSettingsJson>>,
    #[serde(default, deserialize_with = "present")]
    timeline: Option<Object<TimelineJson>>,
    #[serde(default)]
    props: BTreeMap<String, Object<PropDefJson>>,
    #[serde(default, deserialize_with = "present")]
    audio: Option<Object<AudioJson>>,
    nodes: Vec<Object<NodeJson>>,
    connections: Vec<Object<ConnectionJson>>,
}

#[cfg(feature = "metadata")]
impl TemplateJson {
    fn schema() -> Value {
        json!({
            "x-lumen-type": "CompositionTemplate",
            "description": "A composition a `clip` plays. Without `render_settings` it takes the root's size, and without `timeline` the root's timeline; a `timeline` without `fps` takes the root's fps.",
            "type": "object",
            "required": ["nodes", "connections"],
            "properties": {
                "render_settings": { "$ref": "#/$defs/renderSettings" },
                "timeline": { "$ref": "#/$defs/timeline" },
                "audio": { "$ref": "#/$defs/audio" },
                "props": {
                    "description": "Props that `prop('name')` reads inside the template, set per `clip`.",
                    "type": "object",
                    "additionalProperties": { "$ref": "#/$defs/compositionProp" }
                },
                "nodes": { "type": "array", "items": { "$ref": "#/properties/nodes/items" } },
                "connections": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/connection" }
                }
            },
            "additionalProperties": false
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PropDefJson {
    #[serde(rename = "type")]
    kind: PropKind,
    #[serde(default, deserialize_with = "present")]
    default: Option<Value>,
}

#[cfg(feature = "metadata")]
impl PropDefJson {
    fn schema() -> Value {
        let declaration = |kind: &str, default: Value| {
            json!({
                "type": "object",
                "required": ["type"],
                "properties": { "type": { "const": kind }, "default": default },
                "additionalProperties": false
            })
        };
        json!({
            "x-lumen-type": "CompositionPropDeclaration",
            "description": "A template prop's type and the value clips that don't set it get. Without a default it is empty, 0, transparent, [0, 0] or [0, 0, 0].",
            "oneOf": [
                declaration("string", json!({ "type": "string" })),
                declaration("number", json!({ "type": "number" })),
                declaration("color", json!({ "$ref": "#/$defs/color" })),
                declaration("vec2", json!({ "$ref": "#/$defs/vec2" })),
                declaration("vec3", json!({ "$ref": "#/$defs/vec3" }))
            ]
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NodeJson {
    id: u64,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default, deserialize_with = "present")]
    params: Option<Value>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConnectionJson {
    from_node: u64,
    /// Every node has the one output, so `from_port` only has to name it.
    #[serde(default, deserialize_with = "present")]
    from_port: Option<String>,
    to_node: u64,
    to_port: String,
}

#[cfg(feature = "metadata")]
impl ConnectionJson {
    fn schema() -> Value {
        json!({
            "x-lumen-type": "Connection",
            "type": "object",
            "required": ["from_node", "to_node", "to_port"],
            "properties": {
                "from_node": { "type": "integer", "minimum": 0 },
                "from_port": {
                    "description": "The output of `from_node` to read. Optional: it defaults to `output`, the only output any node has.",
                    "type": "string"
                },
                "to_node": { "type": "integer", "minimum": 0 },
                "to_port": { "type": "string" }
            },
            "additionalProperties": false
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioJson {
    tracks: Vec<Object<AudioTrackJson>>,
    clips: Vec<Object<AudioClipJson>>,
}

#[cfg(feature = "metadata")]
impl AudioJson {
    fn schema() -> Value {
        json!({
            "x-lumen-type": "AudioTimeline",
            "description": "Audio tracks and clips. A template's audio plays wherever a `clip` plays the template, following the clip's start, duration, offset, speed and end.",
            "type": "object",
            "required": ["tracks", "clips"],
            "properties": {
                "tracks": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/audioTrack" }
                },
                "clips": {
                    "type": "array",
                    "items": { "$ref": "#/$defs/audioClip" }
                }
            },
            "additionalProperties": false
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioTrackJson {
    id: String,
    /// A label for editors; playback doesn't read it.
    #[serde(rename = "name", default)]
    _name: Option<String>,
    #[serde(default)]
    muted: bool,
    #[serde(default)]
    solo: bool,
    #[serde(default)]
    gain: GainJson,
}

#[cfg(feature = "metadata")]
impl AudioTrackJson {
    fn schema() -> Value {
        json!({
            "x-lumen-type": "AudioTrack",
            "type": "object",
            "required": ["id"],
            "properties": {
                "id": { "type": "string" },
                "name": { "type": "string" },
                "muted": { "type": "boolean" },
                "solo": { "type": "boolean" },
                "gain": GainJson::schema()
            },
            "additionalProperties": false
        })
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioClipJson {
    id: String,
    /// A label for editors; playback doesn't read it.
    #[serde(rename = "name", default)]
    _name: Option<String>,
    source_id: String,
    track_id: String,
    start_ms: u64,
    duration_ms: u64,
    #[serde(default)]
    source_start_ms: u64,
    #[serde(default)]
    gain: GainJson,
    #[serde(default)]
    fade_in_ms: u64,
    #[serde(default)]
    fade_out_ms: u64,
}

#[cfg(feature = "metadata")]
impl AudioClipJson {
    fn schema() -> Value {
        json!({
            "x-lumen-type": "AudioClip",
            "type": "object",
            "required": ["id", "source_id", "track_id", "start_ms", "duration_ms"],
            "properties": {
                "id": { "type": "string" },
                "source_id": { "type": "string" },
                "track_id": { "type": "string" },
                "name": { "type": "string" },
                "start_ms": { "type": "integer", "minimum": 0 },
                "duration_ms": { "type": "integer", "minimum": 1 },
                "source_start_ms": { "type": "integer", "minimum": 0 },
                "gain": GainJson::schema(),
                "fade_in_ms": {
                    "description": "Milliseconds the clip rises from silence over at its start, on an equal-power curve.",
                    "type": "integer",
                    "minimum": 0
                },
                "fade_out_ms": {
                    "description": "Milliseconds the clip falls to silence over at its end, on an equal-power curve.",
                    "type": "integer",
                    "minimum": 0
                }
            },
            "additionalProperties": false
        })
    }
}

/// A gain as the document holds it: a number of at least 0, or an `=expression` parsed once
/// the composition's functions are known.
enum GainJson {
    Value(f32),
    Expr(String),
}

impl Default for GainJson {
    fn default() -> Self {
        Self::Value(1.0)
    }
}

impl<'de> Deserialize<'de> for GainJson {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let expected = "must be a number of at least 0 or an `=expression`";
        match Value::deserialize(deserializer)? {
            // Negative gains are rejected before narrowing could round them to zero.
            Value::Number(number) => match number.as_f64() {
                Some(gain) if gain < 0.0 => {
                    Err(D::Error::custom(format!("must be at least 0, got {gain}")))
                }
                Some(gain) => Ok(Self::Value(gain as f32)),
                None => Err(D::Error::custom(expected)),
            },
            Value::String(text) => match classify_string(&text) {
                Some(StringParam::Expression(source)) => Ok(Self::Expr(source.to_string())),
                _ => Err(D::Error::custom(format!("{expected}, got \"{text}\""))),
            },
            other => Err(D::Error::custom(format!("{expected}, got {other}"))),
        }
    }
}

#[cfg(feature = "metadata")]
impl GainJson {
    /// An audio track's or clip's gain: a number, or an expression of the composition's time.
    fn schema() -> Value {
        json!({
            "description": "Gain, 1 by default. An expression is evaluated in the time of the composition the audio is in, and reads `frame`, `time`, `fps`, `width`, `height` and props.",
            "anyOf": [
                { "type": "number", "minimum": 0, "maximum": f32::MAX as f64 },
                { "$ref": "#/$defs/expression" }
            ]
        })
    }
}

impl GainJson {
    /// The gain, or the error for the expression at `pointer` that doesn't parse.
    fn gain(&self, pointer: String) -> Result<AudioGain, LumenError> {
        match self {
            Self::Value(gain) => Ok(AudioGain::Value(*gain)),
            Self::Expr(source) => crate::expr::Expression::parse(source)
                .map(AudioGain::Expr)
                .map_err(|error| {
                    DocumentError::Invalid {
                        at: At::default(),
                        details: format!("`{}`: `={source}`: {error}", dotted(&pointer)),
                        pointer,
                    }
                    .into()
                }),
        }
    }
}

/// `/audio/clips/0/gain` as `audio.clips[0].gain`, as document errors name values.
fn dotted(pointer: &str) -> String {
    let mut path = String::new();
    for segment in pointer.split('/').skip(1) {
        if segment.bytes().all(|byte| byte.is_ascii_digit()) {
            path.push_str(&format!("[{segment}]"));
        } else {
            if !path.is_empty() {
                path.push('.');
            }
            path.push_str(segment);
        }
    }
    path
}

/// The audio timeline, adding the expressions that don't parse to `errors`.
fn audio_timeline(audio: &AudioJson, errors: &mut Vec<LumenError>) -> AudioTimeline {
    let mut gain = |gain: &GainJson, pointer: String| {
        gain.gain(pointer).unwrap_or_else(|error| {
            errors.push(error);
            AudioGain::Value(0.0)
        })
    };
    let tracks = audio
        .tracks
        .iter()
        .enumerate()
        .map(|(index, track)| AudioTrack {
            id: track.id.clone(),
            muted: track.muted,
            solo: track.solo,
            gain: gain(&track.gain, format!("/audio/tracks/{index}/gain")),
        })
        .collect();
    let clips = audio
        .clips
        .iter()
        .enumerate()
        .map(|(index, clip)| AudioClip {
            id: clip.id.clone(),
            source_id: clip.source_id.clone(),
            track_id: clip.track_id.clone(),
            start_ms: clip.start_ms,
            duration_ms: clip.duration_ms,
            source_start_ms: clip.source_start_ms,
            gain: gain(&clip.gain, format!("/audio/clips/{index}/gain")),
            fade_in_ms: clip.fade_in_ms,
            fade_out_ms: clip.fade_out_ms,
        })
        .collect();
    AudioTimeline { tracks, clips }
}

/// An optional key that must hold a value when it is there, so `null` fails as the schema says.
fn present<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    deserializer: D,
) -> Result<Option<T>, D::Error> {
    T::deserialize(deserializer).map(Some)
}

/// Reads a `u32`, accepting whole floats such as `1.0` like JSON Schema does.
fn whole_u32<'de, D: Deserializer<'de>>(deserializer: D) -> Result<u32, D::Error> {
    let number = Number::deserialize(deserializer)?;
    let whole = match number.as_u64() {
        Some(integer) => u32::try_from(integer).ok(),
        None => number.as_f64().and_then(|float| {
            (float.fract() == 0.0 && (0.0..=f64::from(u32::MAX)).contains(&float))
                .then_some(float as u32)
        }),
    };
    whole.ok_or_else(|| {
        D::Error::custom(format!(
            "must be a whole number from 0 to {}, got {number}",
            u32::MAX
        ))
    })
}

/// The schema of the document root without its nodes: `properties.nodes.items` is left for the
/// caller, which knows the node kinds.
#[cfg(feature = "metadata")]
pub fn root_schema() -> Value {
    DocumentJson::schema()
}

/// The `$defs` the root schema refers to, in the order the schema lists them.
#[cfg(feature = "metadata")]
pub fn schema_defs() -> Vec<(&'static str, Value)> {
    vec![
        ("timeline", TimelineJson::schema()),
        ("renderSettings", RenderSettingsJson::schema()),
        ("compositionTemplate", TemplateJson::schema()),
        ("compositionProp", PropDefJson::schema()),
        ("connection", ConnectionJson::schema()),
        ("audio", AudioJson::schema()),
        ("audioTrack", AudioTrackJson::schema()),
        ("audioClip", AudioClipJson::schema()),
    ]
}

/// Why a document isn't a composition: its errors, at most [`MAX_DIAGNOSTICS`] of them, each
/// with its path in the document.
#[derive(Debug, Clone)]
pub struct ParseError {
    diagnostics: Vec<Diagnostic>,
}

impl ParseError {
    fn new(document: &Value, mut errors: Vec<LumenError>) -> Self {
        errors.truncate(MAX_DIAGNOSTICS);
        Self {
            diagnostics: errors
                .iter()
                .map(|error| Diagnostic::from_error(error, document))
                .collect(),
        }
    }

    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.diagnostics
    }

    pub fn into_diagnostics(self) -> Vec<Diagnostic> {
        self.diagnostics
    }
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (index, diagnostic) in self.diagnostics.iter().enumerate() {
            if index > 0 {
                f.write_str("; ")?;
            }
            f.write_str(&diagnostic.message)?;
        }
        Ok(())
    }
}

impl std::error::Error for ParseError {}

/// Parse a JSON string into a [`Composition`].
pub fn parse(json: &str) -> Result<Composition, ParseError> {
    parse_value(&read_json(json).map_err(|error| ParseError::new(&Value::Null, vec![error]))?)
}

/// Parse a [`serde_json::Value`] into a [`Composition`].
pub fn parse_value(root: &Value) -> Result<Composition, ParseError> {
    load(root).map_err(|errors| ParseError::new(root, errors))
}

/// Everything wrong with a composition that can be told without rendering it: its errors, or the
/// warnings of one that parses. `known_font` says whether the host can resolve a font family, and
/// `assets` is the host's asset store, which `asset:` references are checked against; without
/// them, fonts and asset references aren't checked. Values set by expressions, and anything that
/// only rendering finds, aren't covered.
pub fn check(
    json: &str,
    known_font: Option<&dyn Fn(&str) -> bool>,
    assets: Option<&dyn AssetResolver>,
) -> Vec<Diagnostic> {
    match read_json(json) {
        Ok(root) => check_value(&root, known_font, assets),
        Err(error) => vec![Diagnostic::from_error(&error, &Value::Null)],
    }
}

/// [`check`] for a [`serde_json::Value`].
pub fn check_value(
    root: &Value,
    known_font: Option<&dyn Fn(&str) -> bool>,
    assets: Option<&dyn AssetResolver>,
) -> Vec<Diagnostic> {
    match load(root) {
        Ok(composition) => check_composition(&composition, root, known_font, assets),
        Err(errors) => ParseError::new(root, errors).into_diagnostics(),
    }
}

/// [`check_value`] for a composition already parsed from `root`.
pub fn check_composition(
    composition: &Composition,
    root: &Value,
    known_font: Option<&dyn Fn(&str) -> bool>,
    assets: Option<&dyn AssetResolver>,
) -> Vec<Diagnostic> {
    let mut diagnostics =
        assets.map_or_else(Vec::new, |assets| asset_errors(composition, root, assets));
    if let Some(known_font) = known_font {
        diagnostics.extend(font_fallbacks(composition, root, known_font));
    }
    diagnostics.truncate(MAX_DIAGNOSTICS);
    diagnostics
}

fn read_json(json: &str) -> Result<Value, LumenError> {
    serde_json::from_str(json).map_err(|error| {
        DocumentError::Syntax {
            details: error.to_string(),
        }
        .into()
    })
}

/// Reads `root` in phases, each reporting every error it finds: the document's shape, the
/// functions, every graph's nodes, every graph's connections, then [`CompositionBuilder::build`].
/// A phase with errors ends the load, since later phases would only repeat them.
fn load(root: &Value) -> Result<Composition, Vec<LumenError>> {
    check_schema_version(root).map_err(|error| vec![error])?;
    let Object(document): Object<DocumentJson> =
        serde_path_to_error::deserialize(root).map_err(|error| {
            let pointer = error
                .path()
                .iter()
                .filter_map(|segment| match segment {
                    serde_path_to_error::Segment::Seq { index } => Some(format!("/{index}")),
                    serde_path_to_error::Segment::Map { key } => {
                        Some(format!("/{}", crate::diagnostic::escape(key)))
                    }
                    serde_path_to_error::Segment::Enum { .. }
                    | serde_path_to_error::Segment::Unknown => None,
                })
                .collect();
            vec![
                DocumentError::Invalid {
                    at: At::default(),
                    pointer,
                    details: error.to_string(),
                }
                .into(),
            ]
        })?;
    let timeline = document.timeline.settings(DEFAULT_FPS);
    let render_settings = document.render_settings.settings();

    let function_scope = FunctionScope::enter(&document.functions)
        .map_err(|error| vec![DocumentError::Function { error }.into()])?;
    let mut errors = Vec::new();
    let mut graph = build_nodes(&document.nodes, &mut errors);
    let mut templates = BTreeMap::new();
    for (name, template) in &document.compositions {
        if name.len() > MAX_TEMPLATE_NAME_BYTES {
            errors.push(
                DocumentError::Invalid {
                    at: At::default(),
                    pointer: "/compositions".to_string(),
                    details: format!(
                        "a name in `compositions` is {} bytes long; the most is \
                         {MAX_TEMPLATE_NAME_BYTES}",
                        name.len()
                    ),
                }
                .into(),
            );
        }
        let mut template_errors = Vec::new();
        let built = build_template(template, &timeline, &render_settings, &mut template_errors);
        errors.extend(
            template_errors
                .into_iter()
                .map(|error| error.in_template(name)),
        );
        templates.insert(name.clone(), built);
    }
    let audio = document
        .audio
        .as_ref()
        .map(|audio| audio_timeline(audio, &mut errors));
    drop(function_scope);
    end_phase(&mut errors)?;

    connect(&mut graph, &document.connections, &mut errors);
    for (name, template) in &document.compositions {
        let mut template_errors = Vec::new();
        let graph = &mut templates.get_mut(name).expect("built above").scene.graph;
        connect(graph, &template.connections, &mut template_errors);
        errors.extend(
            template_errors
                .into_iter()
                .map(|error| error.in_template(name)),
        );
    }
    end_phase(&mut errors)?;

    CompositionBuilder {
        scene: Scene {
            graph,
            timeline,
            render_settings,
        },
        audio,
        compositions: templates,
    }
    .build()
}

fn end_phase(errors: &mut Vec<LumenError>) -> Result<(), Vec<LumenError>> {
    if errors.is_empty() {
        Ok(())
    } else {
        Err(std::mem::take(errors))
    }
}

/// The longest name a template in `compositions` may have. Whatever lists a composition's parts
/// repeats the name for each, so it is kept short.
const MAX_TEMPLATE_NAME_BYTES: usize = 256;

fn check_schema_version(root: &Value) -> Result<(), LumenError> {
    let details = match root.get("lumenSchemaVersion") {
        None => return Ok(()),
        Some(Value::String(version)) if reads_schema_version(version) => return Ok(()),
        Some(Value::String(version)) => format!(
            "this composition declares schema version {version}; this engine reads \
             {SCHEMA_VERSION}, so it was written for a different Lumen version"
        ),
        Some(_) => format!(
            "`lumenSchemaVersion` must be a string such as \"{SCHEMA_VERSION}\", the Lumen \
             version the composition was written for"
        ),
    };
    Err(DocumentError::SchemaVersion { details }.into())
}

/// Whether `version` is [`SCHEMA_VERSION`], or that `major.minor` with a numeric patch.
fn reads_schema_version(version: &str) -> bool {
    version == SCHEMA_VERSION
        || version
            .strip_prefix(SCHEMA_VERSION)
            .and_then(|rest| rest.strip_prefix('.'))
            .is_some_and(|patch| !patch.is_empty() && patch.bytes().all(|b| b.is_ascii_digit()))
}

fn json_type(value: &Value) -> &'static str {
    match value {
        Value::Null => "null",
        Value::Bool(_) => "a boolean",
        Value::Number(_) => "a number",
        Value::String(_) => "a string",
        Value::Array(_) => "an array",
        Value::Object(_) => "an object",
    }
}

/// Builds the nodes of the root or of a template, adding what's wrong with each to `errors`.
fn build_nodes(nodes: &[Object<NodeJson>], errors: &mut Vec<LumenError>) -> Graph {
    let mut graph = Graph::new();
    let mut ids = std::collections::HashSet::new();
    for (index, node) in nodes.iter().enumerate() {
        let id = NodeId::new(node.id);
        if !ids.insert(id) {
            errors.push(
                DocumentError::DuplicateNodeId {
                    at: At::node(id),
                    index,
                }
                .into(),
            );
            continue;
        }
        let params = match &node.params {
            None => None,
            Some(Value::Object(params)) => Some(params),
            Some(other) => {
                errors.push(
                    DocumentError::ParamsNotObject {
                        at: At::param(id, ""),
                        kind: node.kind.clone(),
                        got: json_type(other),
                    }
                    .into(),
                );
                continue;
            }
        };
        let node_kind = match NodeKind::from_json(&node.kind, id, params) {
            Ok(node_kind) => node_kind,
            Err(node_errors) => {
                errors.extend(node_errors.into_iter().map(LumenError::from));
                continue;
            }
        };
        if let (NodeKind::Instance3d(_), Some(params)) = (&node_kind, params)
            && let Some(details) = Instance3d::copies_conflict(params)
        {
            errors.push(
                DocumentError::InvalidParam {
                    at: At::param(id, "copies"),
                    details,
                }
                .into(),
            );
            continue;
        }
        let invalid = node_kind.literal_param_errors();
        if !invalid.is_empty() {
            errors.extend(invalid.into_iter().map(|(param, details)| {
                DocumentError::InvalidParam {
                    at: At::param(id, param),
                    details,
                }
                .into()
            }));
            continue;
        }
        graph.nodes.insert(id, node_kind);
        graph.positions.insert(id, index);
    }
    graph
}

/// Writes each connection into its target node, adding the ones that can't be made to `errors`.
fn connect(
    graph: &mut Graph,
    connections: &[Object<ConnectionJson>],
    errors: &mut Vec<LumenError>,
) {
    for (index, connection) in connections.iter().enumerate() {
        if let Some(from_port) = &connection.from_port
            && from_port != "output"
        {
            errors.push(
                DocumentError::InvalidFromPort {
                    at: At::default(),
                    index,
                    port: from_port.clone(),
                }
                .into(),
            );
            continue;
        }
        let from_node = NodeId::new(connection.from_node);
        let to_node = NodeId::new(connection.to_node);
        let to_port = &connection.to_port;
        let connected = graph.connect(Connection {
            from_node,
            to_node,
            to_port: to_port.clone(),
        });
        if let Err(LumenError::GraphValidation(error)) = connected {
            errors.push(
                DocumentError::Connection {
                    at: At::default(),
                    index,
                    from_node,
                    to_node,
                    to_port: to_port.clone(),
                    error: Box::new(error),
                }
                .into(),
            );
        }
    }
}

fn build_template(
    template: &TemplateJson,
    root_timeline: &TimelineSettings,
    root_settings: &RenderSettings,
    errors: &mut Vec<LumenError>,
) -> CompositionTemplate {
    let timeline = template.timeline.as_ref().map_or_else(
        || root_timeline.clone(),
        |timeline| timeline.settings(root_timeline.fps),
    );
    let render_settings = template
        .render_settings
        .as_ref()
        .map_or_else(|| root_settings.clone(), |settings| settings.settings());
    let props = template
        .props
        .iter()
        .map(|(name, def)| {
            let default = match &def.default {
                Some(value) => def.kind.literal(value).unwrap_or_else(|error| {
                    errors.push(
                        GraphValidationError::InvalidSettings {
                            at: At::default(),
                            field: format!("/props/{}/default", crate::diagnostic::escape(name)),
                            details: format!("prop `{name}` default: {error}"),
                        }
                        .into(),
                    );
                    def.kind.zero()
                }),
                None => def.kind.zero(),
            };
            let kind = def.kind;
            (name.clone(), PropDef { kind, default })
        })
        .collect();
    CompositionTemplate {
        scene: Scene {
            graph: build_nodes(&template.nodes, errors),
            timeline,
            render_settings,
        },
        props,
        audio: template
            .audio
            .as_ref()
            .map(|audio| audio_timeline(audio, errors)),
    }
}

/// An error for each asset a source names as a literal that `assets` doesn't have, or has as
/// another kind than the source reads.
fn asset_errors(
    composition: &Composition,
    root: &Value,
    assets: &dyn AssetResolver,
) -> Vec<Diagnostic> {
    let mut errors = Vec::new();
    crate::media::walk_media(composition, &mut |found| {
        let (source, kinds, at) = match found {
            Found::Media { source, kinds, at } => (source, kinds, at),
            Found::Font { family, at } => (family, vec![AssetKind::Font], MediaLocation::Node(at)),
            Found::Dynamic { .. } => return,
        };
        let Some(id) = asset_id(source) else {
            return;
        };
        if let Err(error) = resolve_asset_of(assets, id, &kinds) {
            let error = LumenError::from(error).media_used_as(&kinds, || at);
            errors.push(Diagnostic::from_error(&error, root));
        }
    });
    errors
}

/// A warning for each family a text node names as a literal that `known_font` can't resolve. The
/// bundled family always resolves, and asset references are checked against the asset store
/// instead.
fn font_fallbacks(
    composition: &Composition,
    root: &Value,
    known_font: &dyn Fn(&str) -> bool,
) -> Vec<Diagnostic> {
    let mut warnings = Vec::new();
    crate::media::walk_media(composition, &mut |found| {
        let Found::Font { family, at } = found else {
            return;
        };
        if family == lumen_text::DEFAULT_FONT_FAMILY
            || asset_id(family).is_some()
            || known_font(family)
        {
            return;
        }
        let warning = Warning {
            at: at.at(),
            kind: WarningKind::FontFallback {
                family: family.to_string(),
            },
        };
        let path = crate::diagnostic::document_pointer(root, &warning.at, "");
        warnings.push(Diagnostic::from_warning(&warning, path));
    });
    warnings
}

#[cfg(test)]
mod tests;
