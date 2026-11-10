//! Error types and diagnostics for the Lumen compositing engine.

use thiserror::Error;

use crate::node::{NodeId, PortKind};

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum LumenError {
    #[code(transparent)]
    #[error(transparent)]
    Document(#[from] DocumentError),
    #[code(transparent)]
    #[error(transparent)]
    GraphValidation(#[from] GraphValidationError),
    #[code(transparent)]
    #[error(transparent)]
    Property(#[from] PropertyError),
    #[code(transparent)]
    #[error(transparent)]
    Expression(#[from] ExpressionError),
    #[code(transparent)]
    #[error(transparent)]
    Media(#[from] MediaError),
    #[code(transparent)]
    #[error(transparent)]
    Render(#[from] RenderError),
}

impl LumenError {
    /// An expression evaluation failure.
    pub fn evaluate(details: impl Into<String>) -> Self {
        Self::Expression(ExpressionError::Evaluate {
            details: details.into(),
        })
    }

    /// Locates an expression failure at the param `path` of node `node_id`. Other errors are
    /// returned as they are.
    pub fn located(self, node_id: NodeId, path: &str) -> Self {
        match self {
            Self::Expression(failure) => Self::Expression(ExpressionError::Located {
                node_id,
                path: path.to_string(),
                failure: Box::new(failure),
            }),
            error => error,
        }
    }

    /// Puts `field` in front of the path of a located expression failure.
    pub fn in_field(self, field: &str) -> Self {
        self.with_located_path(|path| match path.chars().next() {
            None => field.to_string(),
            Some('[') => format!("{field}{path}"),
            Some(_) => format!("{field}.{path}"),
        })
    }

    /// Puts `[index]` in front of the path of a located expression failure.
    pub fn in_index(self, index: usize) -> Self {
        self.with_located_path(|path| match path.chars().next() {
            None | Some('[') => format!("[{index}]{path}"),
            Some(_) => format!("[{index}].{path}"),
        })
    }

    fn with_located_path(self, prefixed: impl FnOnce(&str) -> String) -> Self {
        match self {
            Self::Expression(ExpressionError::Located {
                node_id,
                path,
                failure,
            }) => Self::Expression(ExpressionError::Located {
                node_id,
                path: prefixed(&path),
                failure,
            }),
            error => error,
        }
    }
}

/// What stops a JSON document from being read as a composition, before its graphs are checked.
#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum DocumentError {
    #[code("document.syntax")]
    #[error("invalid JSON: {details}")]
    Syntax { details: String },
    #[code("document.schema_version")]
    #[error("{details}")]
    SchemaVersion { details: String },
    /// A key or value the document's shape doesn't allow, outside node params.
    #[code("document.invalid")]
    #[error("{details}")]
    Invalid {
        /// A JSON pointer to the value.
        pointer: String,
        details: String,
    },
    #[code("expression.function")]
    #[error("functions: {error}")]
    Function { error: ExpressionError },
    #[code("node.unknown_type")]
    #[error("node {node_id}: unknown or unsupported node type `{kind}`")]
    UnknownNodeType { node_id: NodeId, kind: String },
    #[code("node.invalid_params")]
    #[error("node {node_id} (`{kind}`): `params` must be an object, got {got}")]
    ParamsNotObject {
        node_id: NodeId,
        kind: String,
        got: &'static str,
    },
    #[code("node.unknown_param")]
    #[error("unknown param `{param}` on node {node_id}")]
    UnknownParam { node_id: NodeId, param: String },
    #[code("node.invalid_param")]
    #[error("node {node_id}: {details}")]
    InvalidParam {
        node_id: NodeId,
        /// The key in `params` whose value is wrong.
        param: String,
        details: String,
    },
    #[code("graph.duplicate_node_id")]
    #[error("duplicate node id {node_id}")]
    DuplicateNodeId {
        node_id: NodeId,
        /// The node's index in `nodes`.
        index: usize,
    },
    #[code("connection.invalid_from_port")]
    #[error("connection {index}: `from_port` must be `output`, got {port:?}")]
    InvalidFromPort { index: usize, port: String },
    /// A connection that could not be made, at its index in `connections`.
    #[code(transparent = error)]
    #[error("connecting {from_node} -> {to_node}:{to_port}: {error}")]
    Connection {
        index: usize,
        from_node: NodeId,
        to_node: NodeId,
        to_port: String,
        error: GraphValidationError,
    },
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum GraphValidationError {
    #[code("graph.cycle")]
    #[error("graph contains a cycle: {}", .path.iter().map(ToString::to_string).collect::<Vec<_>>().join(" -> "))]
    Cycle {
        /// The nodes along the cycle, ending with the one it returns to.
        path: Vec<NodeId>,
    },
    #[code("graph.node_id_mismatch")]
    #[error("node {node_id} is stored under id {key}")]
    NodeIdMismatch { key: NodeId, node_id: NodeId },
    #[code("connection.missing_target_node")]
    #[error("connection target node {node_id} is missing")]
    MissingTargetNode { node_id: NodeId },
    #[code("connection.missing_source_node")]
    #[error("connection source node {node_id} is missing")]
    MissingSourceNode { node_id: NodeId },
    #[code("connection.unknown_port")]
    #[error("connection target port `{port}` is missing on node {node_id}")]
    MissingTargetPort { node_id: NodeId, port: String },
    #[code("connection.input_already_connected")]
    #[error("input `{port}` of node {node_id} is already connected")]
    InputAlreadyConnected { node_id: NodeId, port: String },
    /// A connection whose source outputs another kind than the input takes.
    #[code("connection.wrong_kind")]
    #[error("node {source_node} outputs {actual}, but input `{port}` of node {node_id} takes {expected}: {}", wrong_kind_hint(*.expected))]
    WrongInputKind {
        node_id: NodeId,
        port: String,
        source_node: NodeId,
        expected: PortKind,
        actual: PortKind,
    },
    #[code("graph.missing_required_input")]
    #[error("node {node_id} is missing required input `{port}`")]
    MissingRequiredInput { node_id: NodeId, port: String },
    #[code("graph.missing_media_output")]
    #[error("exactly one media output node is required, found none")]
    MissingMediaOutput,
    #[code("graph.multiple_media_outputs")]
    #[error("exactly one media output node is required, found {count}")]
    MultipleMediaOutputs { count: usize },
    #[code("clip.invalid")]
    #[error("{}node {node_id}: {details}", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default())]
    InvalidClip {
        /// The template the clip is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        details: String,
    },
    #[code("expression.invalid_prop_read")]
    #[error("{}node {node_id}: {details}", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default())]
    InvalidPropRead {
        /// The template the node is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        details: String,
    },
    #[code("settings.invalid")]
    #[error("{}{details}", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default())]
    InvalidSettings {
        /// The template the settings are in, or `None` at the root.
        template: Option<String>,
        /// A JSON pointer to the setting, from the root or the template.
        field: String,
        details: String,
    },
    #[code("audio.invalid")]
    #[error("audio: {details}")]
    InvalidAudio { details: String },
    #[code("composition.cycle")]
    #[error("compositions instantiate themselves: {}", .templates.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(" -> "))]
    TemplateCycle {
        /// The templates along the cycle, ending with the one it returns to.
        templates: Vec<String>,
    },
    #[code(transparent = error)]
    #[error("composition `{template}`: {error}")]
    InTemplate {
        template: String,
        error: Box<LumenError>,
    },
}

/// What to do about wiring a node of the wrong kind into an input that takes `expected`.
fn wrong_kind_hint(expected: PortKind) -> &'static str {
    match expected {
        PortKind::Raster => "render the scene to a raster first, and wire that in",
        PortKind::Scene => {
            "show the raster in the scene through a scene node that takes a raster input"
        }
    }
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum PropertyError {
    #[code("property.invalid_type")]
    #[error(
        "property `{property_path}` on node {node_id} has invalid type: expected {expected}, got {actual}"
    )]
    InvalidType {
        node_id: NodeId,
        property_path: String,
        expected: &'static str,
        actual: &'static str,
    },
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum ExpressionError {
    #[code("expression.parse")]
    #[error("expression parse failed: {details}")]
    Parse { details: String },
    #[code("expression.evaluate")]
    #[error("{details}")]
    Evaluate { details: String },
    #[code(transparent = failure)]
    #[error("node {node_id}{}: {failure}", if .path.is_empty() { String::new() } else { format!(" `{path}`") })]
    Located {
        node_id: NodeId,
        /// The param within the node, such as `content[1].font_size`.
        path: String,
        failure: Box<ExpressionError>,
    },
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum MediaError {
    #[code("media.not_found")]
    #[error("media source `{media_source}` not found: {details}")]
    SourceNotFound {
        media_source: String,
        details: String,
    },
    #[code("media.decode")]
    #[error("media decoding failed for {media_source}: {details}")]
    Decode {
        media_source: String,
        details: String,
    },
    #[code("media.frame_out_of_range")]
    #[error("media frame {frame} is out of range for {media_source} ({frame_count} frames)")]
    FrameOutOfRange {
        media_source: String,
        frame: u32,
        frame_count: u32,
    },
    #[code("audio.unknown_track")]
    #[error("audio clip `{clip}` names unknown track `{track}`")]
    UnknownAudioTrack { clip: String, track: String },
    #[code("asset.not_found")]
    #[error("asset `{id}` is not one this host has")]
    AssetNotFound { id: String },
    #[code("asset.wrong_kind")]
    #[error("asset `{id}` is {actual}, not {expected}")]
    AssetWrongKind {
        id: String,
        expected: crate::media::AssetKind,
        actual: crate::media::AssetKind,
    },
}

impl MediaError {
    pub fn source_not_found(source: &str, details: impl Into<String>) -> Self {
        Self::SourceNotFound {
            media_source: source.to_string(),
            details: details.into(),
        }
    }
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum RenderError {
    #[code("render.frame_out_of_range")]
    #[error("frame {frame} is out of range for a composition of {duration_frames} frames")]
    FrameOutOfRange { frame: u32, duration_frames: u32 },
    #[code("render.node_evaluation")]
    #[error("{node_kind} node {node_id} evaluation failed: {details}")]
    NodeEvaluation {
        node_id: NodeId,
        node_kind: &'static str,
        details: String,
    },
    #[code("render.missing_node")]
    #[error("node {node_id} is referenced but missing from the graph")]
    MissingNode { node_id: NodeId },
    #[code("render.invalid_output_type")]
    #[error("output of node {node_id} has invalid type: expected {expected}, got {actual}")]
    InvalidNodeOutputType {
        node_id: NodeId,
        expected: &'static str,
        actual: &'static str,
    },
    #[code("render.gpu")]
    #[error("GPU render failed: {details}")]
    Gpu { details: String },
}

/// Something that renders, but not as the composition asks. Warnings never fail a render.
#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum Warning {
    /// A text node's family resolved to no font, so its text is shaped in the default family.
    #[code("font.fallback")]
    #[error("{}text node {node_id}: font family `{family}` is not available, so its text falls back to `{}`", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default(), lumen_text::DEFAULT_FONT_FAMILY)]
    FontFallback {
        /// The template the node is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        family: String,
    },
}
