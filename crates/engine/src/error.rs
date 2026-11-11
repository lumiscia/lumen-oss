//! Error types and diagnostics for the Lumen compositing engine.

use std::fmt;

use serde::{Serialize, ser::SerializeMap};
use thiserror::Error;

use crate::{
    media::{AssetKind, MediaLocation},
    node::{NodeId, PortKind},
};

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

impl From<MediaFailure> for LumenError {
    fn from(failure: MediaFailure) -> Self {
        Self::Media(failure.into())
    }
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

    /// The media failure this is, if it is one.
    pub fn media_failure(&self) -> Option<&MediaFailure> {
        match self {
            Self::Media(error) => error.failure(),
            _ => None,
        }
    }

    /// Says what kinds of media and where a media failure's source is used, unless it already
    /// does. Other errors are returned as they are.
    pub fn media_used_as(self, wanted: &[AssetKind], at: impl FnOnce() -> MediaLocation) -> Self {
        match self {
            Self::Media(error) => Self::Media(error.used_as(wanted, at)),
            error => error,
        }
    }

    /// Whether trying again may succeed, as the failure was a shortage that can pass.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Media(error) if error.is_transient())
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
    /// A 3D node, in a build of the engine without the `3d` feature.
    #[code("node.unsupported")]
    #[error("node {node_id} (`{kind}`): {NO_3D_SUPPORT}")]
    No3dSupport { node_id: NodeId, kind: &'static str },
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

pub(crate) const NO_3D_SUPPORT: &str =
    "this build of the engine has no 3D support, as it was built without the `3d` feature";

/// What to do about wiring a node of the wrong kind into an input that takes `expected`.
fn wrong_kind_hint(expected: PortKind) -> &'static str {
    match expected {
        PortKind::Raster => "draw the scene into a raster through a render_3d, and wire that in",
        PortKind::Scene => "show the raster in the scene through an image_plane_3d",
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

/// A media source that can't be used, the same way for every kind of media and in every host.
///
/// `source` is the string the composition wrote (a path, a URL, an alias or `asset:<id>`),
/// `wanted` the kinds the place using it accepts, and `found` the kind it turned out to be when
/// the host or the data says. `at` says where the composition uses it, once the engine knows.
#[derive(Debug, Clone)]
pub struct MediaFailure {
    pub source: String,
    /// The kinds of media the use reads, which are empty when the host that failed doesn't know.
    pub wanted: Vec<AssetKind>,
    /// The kind of media the source is, when it is known to be another than `wanted`.
    pub found: Option<AssetKind>,
    pub at: Option<MediaLocation>,
    pub reason: MediaReason,
    /// Whether the failure is a shortage that can pass, such as running out of file descriptors,
    /// so that trying again may succeed.
    pub transient: bool,
}

/// Why a media source can't be used, with its public diagnostic code.
///
/// `media.decode` is for data that is meant to be what the use wants but can't be read: it is
/// corrupt, truncated, contradicts itself, or is in no format the engine can identify.
/// `media.unsupported` is for data the engine identifies and doesn't read: a container or codec
/// it lacks, a glTF extension it doesn't implement, a font format it doesn't load.
#[derive(Debug, Clone, lumen_macros::DiagnosticCode)]
pub enum MediaReason {
    /// The host has nothing under the source.
    #[code("media.not_found")]
    NotFound { details: String },
    /// The source exists, but is another kind of media than the use reads.
    #[code("media.wrong_kind")]
    WrongKind { details: String },
    /// The source is the right kind, in a format or with a feature the engine doesn't read.
    #[code("media.unsupported")]
    Unsupported { details: String },
    /// The source is over a limit, in bytes, pixels or whatever `details` counts.
    #[code("media.too_large")]
    TooLarge {
        details: String,
        limit: u64,
        /// The value over the limit, when it is known; reading stops at the limit otherwise.
        actual: Option<u64>,
    },
    /// The source can't be read as what it should be.
    #[code("media.decode")]
    Decode { details: String },
}

impl MediaReason {
    /// The same reason with other details.
    pub fn with_details(self, details: String) -> Self {
        match self {
            Self::NotFound { .. } => Self::NotFound { details },
            Self::WrongKind { .. } => Self::WrongKind { details },
            Self::Unsupported { .. } => Self::Unsupported { details },
            Self::TooLarge { limit, actual, .. } => Self::TooLarge {
                details,
                limit,
                actual,
            },
            Self::Decode { .. } => Self::Decode { details },
        }
    }

    pub fn details(&self) -> &str {
        match self {
            Self::NotFound { details }
            | Self::WrongKind { details }
            | Self::Unsupported { details }
            | Self::TooLarge { details, .. }
            | Self::Decode { details } => details,
        }
    }
}

impl MediaFailure {
    fn new(source: &str, reason: MediaReason) -> Self {
        Self {
            source: source.to_string(),
            wanted: Vec::new(),
            found: None,
            at: None,
            reason,
            transient: false,
        }
    }

    pub fn not_found(source: &str, details: impl Into<String>) -> Self {
        Self::new(
            source,
            MediaReason::NotFound {
                details: details.into(),
            },
        )
    }

    /// `source` is `found`, which is not one of `wanted`.
    pub fn wrong_kind(
        source: &str,
        wanted: &[AssetKind],
        found: Option<AssetKind>,
        details: impl Into<String>,
    ) -> Self {
        Self {
            wanted: wanted.to_vec(),
            found,
            ..Self::new(
                source,
                MediaReason::WrongKind {
                    details: details.into(),
                },
            )
        }
    }

    pub fn unsupported(source: &str, details: impl Into<String>) -> Self {
        Self::new(
            source,
            MediaReason::Unsupported {
                details: details.into(),
            },
        )
    }

    /// `source` is over a limit of `limit`, by `actual` if that is known.
    pub fn too_large(
        source: &str,
        details: impl Into<String>,
        limit: u64,
        actual: Option<u64>,
    ) -> Self {
        Self::new(
            source,
            MediaReason::TooLarge {
                details: details.into(),
                limit,
                actual,
            },
        )
    }

    pub fn decode(source: &str, details: impl Into<String>) -> Self {
        Self::new(
            source,
            MediaReason::Decode {
                details: details.into(),
            },
        )
    }

    /// The kinds the use accepts, when the failure doesn't have them yet.
    pub fn wanting(mut self, kinds: &[AssetKind]) -> Self {
        if self.wanted.is_empty() {
            self.wanted = kinds.to_vec();
        }
        self
    }

    /// Marks the failure as one that can pass.
    pub fn transient(mut self) -> Self {
        self.transient = true;
        self
    }

    /// Where the composition uses the source, if the failure doesn't say yet.
    pub fn located(mut self, at: impl FnOnce() -> MediaLocation) -> Self {
        if self.at.is_none() {
            self.at = Some(at());
        }
        self
    }

    pub fn code(&self) -> &'static str {
        self.reason.code()
    }

    /// The failure from reading `source`, which `error` is about: a file that can't be opened is
    /// `media.not_found`, a shortage of memory, file descriptors or a device that can pass is a
    /// transient `media.decode`, and any other failure to read is `media.decode` of `what`.
    pub fn from_io(source: &str, what: &str, error: &std::io::Error) -> Self {
        use std::io::ErrorKind;
        if matches!(
            error.kind(),
            ErrorKind::NotFound | ErrorKind::PermissionDenied | ErrorKind::NotADirectory
        ) {
            return Self::not_found(source, error.to_string());
        }
        let failure = Self::decode(source, format!("failed reading {what}: {error}"));
        if is_shortage(error) {
            failure.transient()
        } else {
            failure
        }
    }
}

/// Whether `error` is the system running short of memory, file descriptors or a device, which can
/// pass.
fn is_shortage(error: &std::io::Error) -> bool {
    use std::io::ErrorKind;
    if matches!(
        error.kind(),
        ErrorKind::OutOfMemory
            | ErrorKind::WouldBlock
            | ErrorKind::Interrupted
            | ErrorKind::ResourceBusy
    ) {
        return true;
    }
    #[cfg(unix)]
    if let Some(errno) = error.raw_os_error() {
        return [
            libc::ENOMEM,
            libc::EMFILE,
            libc::ENFILE,
            libc::EAGAIN,
            libc::EBUSY,
        ]
        .contains(&errno);
    }
    false
}

impl fmt::Display for MediaFailure {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let noun = match self.wanted.as_slice() {
            [kind] => kind_noun(*kind),
            _ => "media source",
        };
        let at = self
            .at
            .as_ref()
            .map(|at| format!(" ({at})"))
            .unwrap_or_default();
        let source = &self.source;
        match &self.reason {
            MediaReason::NotFound { .. } => write!(f, "{noun} `{source}`{at} not found")?,
            MediaReason::WrongKind { .. } => {
                let wanted = self
                    .wanted
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(" or ");
                match self.found {
                    Some(found) => write!(f, "`{source}`{at} is {found}, not {wanted}")?,
                    None => write!(f, "`{source}`{at} is not {wanted}")?,
                }
            }
            MediaReason::Unsupported { .. } => write!(f, "{noun} `{source}`{at} is not supported")?,
            MediaReason::TooLarge { .. } => write!(f, "{noun} `{source}`{at} is too large")?,
            MediaReason::Decode { .. } => write!(f, "{noun} `{source}`{at} failed to decode")?,
        }
        match self.reason.details() {
            "" => Ok(()),
            details => write!(f, ": {details}"),
        }
    }
}

impl std::error::Error for MediaFailure {}

impl Serialize for MediaFailure {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(None)?;
        map.serialize_entry("code", self.code())?;
        map.serialize_entry("source", &self.source)?;
        map.serialize_entry("wanted", &self.wanted)?;
        if let Some(found) = &self.found {
            map.serialize_entry("found", found)?;
        }
        if let Some(at) = &self.at {
            map.serialize_entry("at", at)?;
        }
        map.serialize_entry("details", self.reason.details())?;
        if let MediaReason::TooLarge { limit, actual, .. } = &self.reason {
            map.serialize_entry("limit", limit)?;
            if let Some(actual) = actual {
                map.serialize_entry("actual", actual)?;
            }
        }
        map.end()
    }
}

/// What a kind is called in a sentence about one source.
fn kind_noun(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Environment => "environment map",
        kind => kind.name(),
    }
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum MediaError {
    /// A source that can't be used: missing, of another kind, unsupported, too large or corrupt.
    #[code(transparent)]
    #[error(transparent)]
    Failed(Box<MediaFailure>),
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
    #[code("audio.limit")]
    #[error("audio: {details}")]
    AudioLimit { details: String },
}

impl From<MediaFailure> for MediaError {
    fn from(failure: MediaFailure) -> Self {
        Self::Failed(Box::new(failure))
    }
}

impl MediaError {
    pub fn source_not_found(source: &str, details: impl Into<String>) -> Self {
        MediaFailure::not_found(source, details).into()
    }

    pub fn decode(source: &str, details: impl Into<String>) -> Self {
        MediaFailure::decode(source, details).into()
    }

    /// The failure, if this is one.
    pub fn failure(&self) -> Option<&MediaFailure> {
        match self {
            Self::Failed(failure) => Some(failure.as_ref()),
            _ => None,
        }
    }

    /// Says what kinds of media and where the source is used, unless the error already does.
    pub fn used_as(self, wanted: &[AssetKind], at: impl FnOnce() -> MediaLocation) -> Self {
        match self {
            Self::Failed(failure) => Self::from((*failure).wanting(wanted).located(at)),
            error => error,
        }
    }

    /// The error with its source named `written`, as the composition wrote it, and `resolved`, the
    /// file the host opened for it, replaced by that name in its details. For hosts that resolve
    /// a source to a path before opening it, so that a failure after open does not name the path.
    pub fn written_as(self, resolved: &str, written: &str) -> Self {
        match self {
            Self::Failed(failure) => {
                let failure = *failure;
                let details = failure.reason.details().replace(resolved, written);
                Self::from(MediaFailure {
                    source: written.to_string(),
                    reason: failure.reason.with_details(details),
                    ..failure
                })
            }
            Self::FrameOutOfRange {
                media_source,
                frame,
                frame_count,
            } => Self::FrameOutOfRange {
                media_source: media_source.replace(resolved, written),
                frame,
                frame_count,
            },
            error => error,
        }
    }

    /// Whether trying again may succeed, as the failure was a shortage that can pass.
    pub fn is_transient(&self) -> bool {
        self.failure().is_some_and(|failure| failure.transient)
    }

    /// The error as a shortage that can pass if its `cause` was one.
    pub(crate) fn caused_by(self, cause: &MediaError) -> Self {
        match self {
            Self::Failed(failure) if cause.is_transient() => Self::from(failure.transient()),
            error => error,
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
    #[code("render.too_deeply_nested")]
    #[error("{}node {node_id} is more than {limit} nodes deep, counting from the output through clips and the compositions they play, so the composition is nested too deeply to render. Flatten it: put many layers in one `raster_multimerge` instead of chaining `merge`s, and combine consecutive transforms or adjustments into one node", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default())]
    TooDeeplyNested {
        /// The template the node is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        limit: u32,
    },
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
    /// Glyphs of a `text_3d` node have no outlines to extrude, as a font that draws them from
    /// colour or bitmap tables, such as emoji, has none, so they are left out.
    #[code("font.no_outlines")]
    #[error("{}text_3d node {node_id}: {glyphs} glyphs have no outlines to extrude, such as colour emoji, so they are left out", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default())]
    NoOutlines {
        /// The template the node is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        glyphs: usize,
    },
    /// A `render_3d` scene has more lights casting shadows than it draws shadows for.
    #[code("scene.shadow_limit")]
    #[error("{}render_3d node {node_id}: light nodes {} cast no shadows, past the first 8 lights casting them", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default(), .lights.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))]
    ShadowLimit {
        /// The template the node is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        /// The lights left without shadows, in scene order.
        lights: Vec<NodeId>,
    },
    /// A `model_3d`'s model has primitives that aren't triangle lists, which aren't drawn.
    #[code("scene.model_primitives_skipped")]
    #[error("{}model_3d node {node_id}: model `{model}` has primitives of points, lines, or triangle strips or fans, which aren't drawn ({count} left out)", .template.as_ref().map(|name| format!("composition `{name}`: ")).unwrap_or_default())]
    ModelPrimitivesSkipped {
        /// The template the node is in, or `None` at the root.
        template: Option<String>,
        node_id: NodeId,
        /// The model's source.
        model: String,
        count: usize,
    },
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;
    use crate::media::NodeParamAt;

    #[test]
    fn a_media_failure_serializes_what_a_host_needs_to_say_what_went_wrong() {
        let failure =
            MediaFailure::too_large("big.png", "wider than the device allows", 8192, Some(9000))
                .wanting(&[AssetKind::Image])
                .located(|| {
                    MediaLocation::Node(NodeParamAt {
                        template: Some("card".to_string()),
                        node: crate::node::NodeId::new(3),
                        param: "source".to_string(),
                    })
                });
        assert_eq!(
            failure.to_string(),
            "image `big.png` (composition `card`, node 3 `source`) is too large: wider than the device allows"
        );
        assert_eq!(
            serde_json::to_value(&failure).unwrap(),
            json!({
                "code": "media.too_large",
                "source": "big.png",
                "wanted": ["image"],
                "at": { "type": "node", "template": "card", "node": 3, "param": "source" },
                "details": "wider than the device allows",
                "limit": 8192,
                "actual": 9000,
            })
        );
    }

    #[test]
    fn a_failure_wrapping_a_shortage_stays_transient() {
        let wrapped = || MediaError::decode("a.mp4", "hardware decode failed");
        let shortage = MediaError::from(MediaFailure::decode("a.mp4", "no memory").transient());
        assert!(wrapped().caused_by(&shortage).is_transient());
        assert!(!wrapped().caused_by(&wrapped()).is_transient());
    }

    #[cfg(unix)]
    #[test]
    fn a_shortage_of_descriptors_or_memory_while_reading_is_transient() {
        for errno in [libc::EMFILE, libc::ENFILE, libc::ENOMEM, libc::EAGAIN] {
            let io = std::io::Error::from_raw_os_error(errno);
            let failure = MediaFailure::from_io("a.png", "image", &io);
            assert_eq!(failure.code(), "media.decode", "{io}");
            assert!(failure.transient, "{io}");
        }
        let corrupt = std::io::Error::from_raw_os_error(libc::EIO);
        assert!(!MediaFailure::from_io("a.png", "image", &corrupt).transient);
        let missing = std::io::Error::from_raw_os_error(libc::ENOTDIR);
        let failure = MediaFailure::from_io("a/b.png", "image", &missing);
        assert_eq!(failure.code(), "media.not_found");
    }
}
