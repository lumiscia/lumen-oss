//! Error types and diagnostics for the Lumen compositing engine.

use std::ops::Range;

use thiserror::Error;

use crate::node::{NodeId, PortKind};

#[derive(Debug, Error, Clone)]
pub enum LumenError {
    #[error("graph validation error: {0}")]
    GraphValidation(#[from] GraphValidationError),
    #[error("property resolution error: {0}")]
    Property(#[from] PropertyError),
    #[error("expression error: {0}")]
    Expression(#[from] ExpressionError),
    #[error("media error: {0}")]
    Media(#[from] MediaError),
    #[error("render error: {0}")]
    Render(#[from] RenderError),
    #[error("threading error: {0}")]
    Threading(#[from] ThreadingError),
    #[error("sink error: {0}")]
    Sink(#[from] SinkError),
}

#[derive(Debug, Error, Clone)]
pub enum GraphValidationError {
    #[error("graph contains a cycle")]
    Cycle { path: Vec<NodeId> },
    #[error("connection target node is missing")]
    MissingTargetNode { node_id: NodeId },
    #[error("connection source node is missing")]
    MissingSourceNode { node_id: NodeId },
    #[error("missing required input `{port}`")]
    MissingRequiredInput { node_id: NodeId, port: String },
    #[error("port kind mismatch for `{to_port}`")]
    PortKindMismatch {
        from_node: NodeId,
        from_port: String,
        from_kind: PortKind,
        to_node: NodeId,
        to_port: String,
        expected_kind: PortKind,
    },
    #[error("switch ranges overlap")]
    SwitchRangeOverlap {
        node_id: NodeId,
        first: Range<u32>,
        second: Range<u32>,
    },
    #[error("exactly one media output node is required")]
    MissingMediaOutput,
    #[error("exactly one media output node is required")]
    MultipleMediaOutputs { count: usize },
    #[error("target node is not reachable or missing")]
    InvalidEvaluationTarget { node_id: NodeId },
}

#[derive(Debug, Error, Clone)]
pub enum PropertyError {
    #[error("property `{property_path}` is missing")]
    MissingProperty {
        node_id: NodeId,
        property_path: String,
    },
    #[error("property `{property_path}` has invalid type")]
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
    Parse {
        path: Option<String>,
        details: String,
    },
    #[error("expression evaluation failed: {details}")]
    Evaluate {
        path: Option<String>,
        details: String,
    },
    #[error("undefined variable in expression")]
    UndefinedVariable { path: Option<String>, name: String },
}

#[derive(Debug, Error, Clone)]
pub enum MediaError {
    #[error("media source not found")]
    SourceNotFound { media_source: String },
    #[error("media decoding failed")]
    Decode {
        media_source: String,
        details: String,
    },
    #[error("media frame is out of range")]
    FrameOutOfRange {
        media_source: String,
        frame: u32,
        frame_count: u32,
    },
}

#[derive(Debug, Error, Clone)]
pub enum RenderError {
    #[error("frame is out of range")]
    FrameOutOfRange { frame: u32, duration_frames: u32 },
    #[error("node evaluation failed")]
    NodeEvaluation {
        frame: u32,
        node_id: NodeId,
        node_kind: &'static str,
        details: String,
    },
    #[error("referenced node is missing from the graph")]
    MissingNode { frame: u32, node_id: NodeId },
    #[error("surface allocation failed")]
    SurfaceAllocation { width: u32, height: u32 },
    #[error("node output is missing")]
    MissingNodeOutput { frame: u32, node_id: NodeId },
    #[error("node output has invalid type")]
    InvalidNodeOutputType {
        frame: u32,
        node_id: NodeId,
        expected: &'static str,
        actual: &'static str,
    },
    #[error("media output node did not produce raster output")]
    InvalidMediaOutputType { frame: u32, node_id: NodeId },
    #[error("surface lease has already been taken")]
    SurfaceLeaseReleased,
    #[error("surface lease is still shared")]
    SharedSurfaceLease,
    #[error("surface readback requires owned access")]
    SurfaceReadbackUnsupported,
    #[error("render cancelled")]
    Cancelled { frame: u32 },
}

#[derive(Debug, Error, Clone)]
pub enum ThreadingError {
    #[error("worker initialization failed")]
    WorkerInit { details: String },
    #[error("worker failed")]
    WorkerFailure { frame: Option<u32>, details: String },
    #[error("render cancelled")]
    Cancelled,
}

#[derive(Debug, Error, Clone)]
pub enum SinkError {
    #[error("sink write failed")]
    WriteFrame { frame: u32, details: String },
    #[error("sink finalize failed")]
    Finalize { details: String },
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
