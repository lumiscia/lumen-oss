//! Error types and diagnostics for the Lumen compositing engine.

use thiserror::Error;

use crate::node::{NodeId, PortKind};

#[derive(Debug, Error, Clone)]
pub enum LumenError {
    #[error(transparent)]
    GraphValidation(#[from] GraphValidationError),
    #[error(transparent)]
    Property(#[from] PropertyError),
    #[error(transparent)]
    Expression(#[from] ExpressionError),
    #[error(transparent)]
    Media(#[from] MediaError),
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

#[derive(Debug, Error, Clone)]
pub enum GraphValidationError {
    #[error("graph contains a cycle: {}", .path.iter().map(ToString::to_string).collect::<Vec<_>>().join(" -> "))]
    Cycle {
        /// The nodes along the cycle, ending with the one it returns to.
        path: Vec<NodeId>,
    },
    #[error("node {node_id} is stored under id {key}")]
    NodeIdMismatch { key: NodeId, node_id: NodeId },
    #[error("connection target node {node_id} is missing")]
    MissingTargetNode { node_id: NodeId },
    #[error("connection source node {node_id} is missing")]
    MissingSourceNode { node_id: NodeId },
    #[error("connection target port `{port}` is missing on node {node_id}")]
    MissingTargetPort { node_id: NodeId, port: String },
    #[error("connection source port `{port}` is missing on node {node_id}")]
    MissingSourcePort { node_id: NodeId, port: String },
    #[error("node {node_id} is missing required input `{port}`")]
    MissingRequiredInput { node_id: NodeId, port: String },
    #[error(
        "port kind mismatch: input `{to_port}` of node {to_node} takes {expected_kind:?}, but output `{from_port}` of node {from_node} is {from_kind:?}"
    )]
    PortKindMismatch {
        from_node: NodeId,
        from_port: String,
        from_kind: PortKind,
        to_node: NodeId,
        to_port: String,
        expected_kind: PortKind,
    },
    #[error("exactly one media output node is required, found none")]
    MissingMediaOutput,
    #[error("exactly one media output node is required, found {count}")]
    MultipleMediaOutputs { count: usize },
    #[error("target node {node_id} is not reachable or missing")]
    InvalidEvaluationTarget { node_id: NodeId },
    #[error("{}node {node_id}: {details}", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default())]
    InvalidClip {
        /// The template the clip is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        details: String,
    },
    #[error("audio: {details}")]
    InvalidAudio { details: String },
    #[error("compositions instantiate themselves: {}", .templates.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(" -> "))]
    TemplateCycle {
        /// The templates along the cycle, ending with the one it returns to.
        templates: Vec<String>,
    },
    #[error("composition `{template}`: {error}")]
    InTemplate {
        template: String,
        error: Box<LumenError>,
    },
}

#[derive(Debug, Error, Clone)]
pub enum PropertyError {
    #[error("property `{property_path}` on node {node_id} is missing")]
    MissingProperty {
        node_id: NodeId,
        property_path: String,
    },
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

#[derive(Debug, Error, Clone)]
pub enum ExpressionError {
    #[error("expression parse failed: {details}")]
    Parse { details: String },
    #[error("{details}")]
    Evaluate { details: String },
    #[error("undefined variable `{name}`")]
    UndefinedVariable { name: String },
    #[error("node {node_id}{}: {failure}", if .path.is_empty() { String::new() } else { format!(" `{path}`") })]
    Located {
        node_id: NodeId,
        /// The param within the node, such as `content[1].font_size`.
        path: String,
        failure: Box<ExpressionError>,
    },
}

#[derive(Debug, Error, Clone)]
pub enum MediaError {
    #[error("media source `{media_source}` not found: {details}")]
    SourceNotFound {
        media_source: String,
        details: String,
    },
    #[error("media decoding failed for {media_source}: {details}")]
    Decode {
        media_source: String,
        details: String,
    },
    #[error("media frame {frame} is out of range for {media_source} ({frame_count} frames)")]
    FrameOutOfRange {
        media_source: String,
        frame: u32,
        frame_count: u32,
    },
    #[error("audio clip `{clip}` names unknown track `{track}`")]
    UnknownAudioTrack { clip: String, track: String },
}

impl MediaError {
    pub fn source_not_found(source: &str, details: impl Into<String>) -> Self {
        Self::SourceNotFound {
            media_source: source.to_string(),
            details: details.into(),
        }
    }
}

#[derive(Debug, Error, Clone)]
pub enum RenderError {
    #[error("frame {frame} is out of range for a composition of {duration_frames} frames")]
    FrameOutOfRange { frame: u32, duration_frames: u32 },
    #[error("{node_kind} node {node_id} evaluation failed: {details}")]
    NodeEvaluation {
        node_id: NodeId,
        node_kind: &'static str,
        details: String,
    },
    #[error("node {node_id} is referenced but missing from the graph")]
    MissingNode { node_id: NodeId },
    #[error("surface allocation failed for {width}x{height}")]
    SurfaceAllocation { width: u32, height: u32 },
    #[error("output of node {node_id} is missing")]
    MissingNodeOutput { node_id: NodeId },
    #[error("output of node {node_id} has invalid type: expected {expected}, got {actual}")]
    InvalidNodeOutputType {
        node_id: NodeId,
        expected: &'static str,
        actual: &'static str,
    },
    #[error("media output node {node_id} did not produce raster output")]
    InvalidMediaOutputType { node_id: NodeId },
    #[error("pixel readback failed for {width}x{height}")]
    PixelReadbackFailed { width: u32, height: u32 },
    #[error("all scratch surfaces are currently in use")]
    ScratchSurfaceUnavailable,
    #[error("{backend} backend is busy")]
    BackendBusy { backend: &'static str },
    #[error("render cancelled")]
    Cancelled,
    #[error("GPU render failed: {details}")]
    Gpu { details: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum Warning {
    FpsMismatch {
        node_id: NodeId,
        composition_fps: f32,
        source_fps: f32,
    },
    CapabilityMissing {
        node_id: NodeId,
        requirement: &'static str,
    },
}
