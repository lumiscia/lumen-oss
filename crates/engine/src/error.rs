//! Error types and diagnostics for the Lumen compositing engine.

use std::fmt;

use serde::{Serialize, ser::SerializeMap};
use thiserror::Error;

use crate::{
    media::{AssetKind, MediaLocation},
    node::{DEFAULT_OUTPUT, JsonNode, NodeId, NodeKind, OutputPortDef, PortKind},
};

/// Where in a composition something is: a scene, the root or a template, and in it, maybe a node
/// and a param of that node.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash)]
pub struct At {
    /// The template, or `None` for the root.
    pub template: Option<String>,
    pub node: Option<NodeId>,
    /// A param path in the node's `params`, such as `content[1].font_size`, or empty for the
    /// whole of `params`.
    pub param: Option<String>,
}

impl At {
    /// Node `node` of the root.
    pub fn node(node: NodeId) -> Self {
        Self {
            node: Some(node),
            ..Self::default()
        }
    }

    /// The param at `path` of node `node` of the root.
    pub fn param(node: NodeId, path: impl Into<String>) -> Self {
        Self {
            param: Some(path.into()),
            ..Self::node(node)
        }
    }

    /// The same place in template `template`, or in the root for `None`.
    pub fn in_template(self, template: Option<&str>) -> Self {
        Self {
            template: template.map(str::to_string),
            ..self
        }
    }

    /// The node this is in, without its param.
    pub(crate) fn node_only(&self) -> Self {
        Self {
            param: None,
            ..self.clone()
        }
    }

    /// The scene this is in, without its node.
    pub(crate) fn scene(&self) -> Self {
        Self {
            template: self.template.clone(),
            ..Self::default()
        }
    }

    /// The place followed by `: `, or nothing for the whole root.
    pub(crate) fn prefix(&self) -> String {
        if self.template.is_none() && self.node.is_none() {
            String::new()
        } else {
            format!("{self}: ")
        }
    }

    /// The place, naming its node as a `kind` node, such as `text node 3`.
    pub(crate) fn of_kind(&self, kind: &'static str) -> impl fmt::Display + '_ {
        OfKind(self, Some(kind))
    }
}

impl fmt::Display for At {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        OfKind(self, None).fmt(f)
    }
}

struct OfKind<'a>(&'a At, Option<&'static str>);

impl fmt::Display for OfKind<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let Self(at, kind) = self;
        let mut separator = "";
        if let Some(template) = &at.template {
            write!(f, "composition `{template}`")?;
            separator = ": ";
        }
        if let Some(node) = at.node {
            f.write_str(separator)?;
            if let Some(kind) = kind {
                write!(f, "{kind} ")?;
            }
            write!(f, "node {node}")?;
            if let Some(param) = at.param.as_deref().filter(|param| !param.is_empty()) {
                write!(f, " `{param}`")?;
            }
        }
        Ok(())
    }
}

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
                at: At::param(node_id, path),
                failure: Box::new(failure),
            }),
            error => error,
        }
    }

    /// The error found in template `name`, rather than the root, unless it already names the
    /// template it is in.
    pub(crate) fn in_template(mut self, name: &str) -> Self {
        if let Some(at) = self.at_mut()
            && at.template.is_none()
        {
            at.template = Some(name.to_string());
        }
        self
    }

    fn at_mut(&mut self) -> Option<&mut At> {
        match self {
            Self::Document(error) => error.at_mut(),
            Self::GraphValidation(error) => error.at_mut(),
            Self::Property(PropertyError::InvalidType { at, .. })
            | Self::Expression(ExpressionError::Located { at, .. }) => Some(at),
            Self::Render(error) => error.at_mut(),
            Self::Expression(_) | Self::Media(_) => None,
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

    fn with_located_path(mut self, prefixed: impl FnOnce(&str) -> String) -> Self {
        if let Self::Expression(ExpressionError::Located { at, .. }) = &mut self {
            at.param = Some(prefixed(at.param.as_deref().unwrap_or_default()));
        }
        self
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
    #[error("{}{details}", .at.prefix())]
    Invalid {
        /// The scene the value is in.
        at: At,
        /// A JSON pointer to the value, from the scene.
        pointer: String,
        details: String,
    },
    #[code("expression.function")]
    #[error("functions: {error}")]
    Function { error: ExpressionError },
    #[code("node.unknown_type")]
    #[error("{at}: unknown or unsupported node type `{kind}`")]
    UnknownNodeType { at: At, kind: String },
    /// `at` is the node's whole `params`.
    #[code("node.invalid_params")]
    #[error("{at} (`{kind}`): `params` must be an object, got {got}")]
    ParamsNotObject {
        at: At,
        kind: String,
        got: &'static str,
    },
    /// `at` is the key in `params` the node doesn't have.
    #[code("node.unknown_param")]
    #[error("{}: unknown param `{}`", .at.node_only(), .at.param.as_deref().unwrap_or_default())]
    UnknownParam { at: At },
    /// `at` is the key in `params` whose value is wrong, which `details` names.
    #[code("node.invalid_param")]
    #[error("{}: {details}", .at.node_only())]
    InvalidParam { at: At, details: String },
    #[code("graph.duplicate_node_id")]
    #[error("{at}: duplicate node id")]
    DuplicateNodeId {
        at: At,
        /// The node's index in `nodes`.
        index: usize,
    },
    #[code("connection.invalid_from_port")]
    #[error(
        "{}connection {index}: node {from_node} has no output {port:?}; {}",
        .at.prefix(),
        outputs_are(outputs)
    )]
    InvalidFromPort {
        at: At,
        index: usize,
        from_node: NodeId,
        port: Box<str>,
        /// The outputs the node names beside `output`.
        outputs: &'static [OutputPortDef],
    },
    /// A connection that could not be made, at its index in `connections`.
    #[code(transparent = error)]
    #[error("{}connecting {from_node} -> {to_node}:{to_port}: {error}", .at.prefix())]
    Connection {
        /// The scene the connection is in.
        at: At,
        index: usize,
        from_node: NodeId,
        to_node: NodeId,
        to_port: String,
        error: Box<GraphValidationError>,
    },
}

impl DocumentError {
    fn at_mut(&mut self) -> Option<&mut At> {
        match self {
            Self::Invalid { at, .. }
            | Self::UnknownNodeType { at, .. }
            | Self::ParamsNotObject { at, .. }
            | Self::UnknownParam { at }
            | Self::InvalidParam { at, .. }
            | Self::DuplicateNodeId { at, .. }
            | Self::InvalidFromPort { at, .. }
            | Self::Connection { at, .. } => Some(at),
            Self::Syntax { .. } | Self::SchemaVersion { .. } | Self::Function { .. } => None,
        }
    }
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum GraphValidationError {
    #[code("graph.cycle")]
    #[error("{}graph contains a cycle: {}", .at.prefix(), .path.iter().map(ToString::to_string).collect::<Vec<_>>().join(" -> "))]
    Cycle {
        /// The scene the graph is.
        at: At,
        /// The nodes along the cycle, ending with the one it returns to.
        path: Vec<NodeId>,
    },
    /// `at` is the id the graph stores the node under.
    #[code("graph.node_id_mismatch")]
    #[error("{at} holds node {node_id}")]
    NodeIdMismatch { at: At, node_id: NodeId },
    #[code("connection.missing_target_node")]
    #[error("{at} is missing as a connection target")]
    MissingTargetNode { at: At },
    #[code("connection.missing_source_node")]
    #[error("{at} is missing as a connection source")]
    MissingSourceNode { at: At },
    #[code("connection.unknown_port")]
    #[error("connection target port `{port}` is missing on {at}")]
    MissingTargetPort { at: At, port: String },
    #[code("connection.input_already_connected")]
    #[error("input `{port}` of {at} is already connected")]
    InputAlreadyConnected { at: At, port: String },
    /// A connection whose source outputs another kind than the input takes.
    #[code("connection.wrong_kind")]
    #[error(
        "{at}: input `{port}` takes {expected}, but {} {actual}: {}",
        wrong_kind_source(*.source_node, *.source_output),
        wrong_kind_hint(*.expected, *.actual)
    )]
    WrongInputKind {
        at: At,
        port: &'static str,
        source_node: NodeId,
        /// The output of `source_node` read, unless it is the default `output`.
        source_output: Option<&'static OutputPortDef>,
        expected: PortKind,
        actual: PortKind,
    },
    /// A 3D node, in a build of the engine without the `3d` feature.
    #[code("node.unsupported")]
    #[error("{at} (`{kind}`): {NO_3D_SUPPORT}")]
    No3dSupport { at: At, kind: &'static str },
    #[code("graph.missing_required_input")]
    #[error("{at} is missing required input `{port}`")]
    MissingRequiredInput { at: At, port: String },
    #[code("graph.missing_media_output")]
    #[error("{}exactly one media output node is required, found none", .at.prefix())]
    MissingMediaOutput { at: At },
    #[code("graph.multiple_media_outputs")]
    #[error("{}exactly one media output node is required, found {count}", .at.prefix())]
    MultipleMediaOutputs { at: At, count: usize },
    /// `at` is the clip's whole `params`.
    #[code("clip.invalid")]
    #[error("{at}: {details}")]
    InvalidClip { at: At, details: String },
    /// `at` is the node's whole `params`.
    #[code("expression.invalid_prop_read")]
    #[error("{at}: {details}")]
    InvalidPropRead { at: At, details: String },
    #[code("settings.invalid")]
    #[error("{}{details}", .at.prefix())]
    InvalidSettings {
        /// The scene the settings are in.
        at: At,
        /// A JSON pointer to the setting, from the scene.
        field: String,
        details: String,
    },
    #[code("audio.invalid")]
    #[error("{}audio: {details}", .at.prefix())]
    InvalidAudio {
        /// The scene the audio is in.
        at: At,
        details: String,
    },
    #[code("composition.cycle")]
    #[error("compositions instantiate themselves: {}", .templates.iter().map(|name| format!("`{name}`")).collect::<Vec<_>>().join(" -> "))]
    TemplateCycle {
        /// The templates along the cycle, ending with the one it returns to.
        templates: Vec<String>,
    },
}

impl GraphValidationError {
    fn at_mut(&mut self) -> Option<&mut At> {
        match self {
            Self::Cycle { at, .. }
            | Self::NodeIdMismatch { at, .. }
            | Self::MissingTargetNode { at }
            | Self::MissingSourceNode { at }
            | Self::MissingTargetPort { at, .. }
            | Self::InputAlreadyConnected { at, .. }
            | Self::WrongInputKind { at, .. }
            | Self::No3dSupport { at, .. }
            | Self::MissingRequiredInput { at, .. }
            | Self::MissingMediaOutput { at }
            | Self::MultipleMediaOutputs { at, .. }
            | Self::InvalidClip { at, .. }
            | Self::InvalidPropRead { at, .. }
            | Self::InvalidSettings { at, .. }
            | Self::InvalidAudio { at, .. } => Some(at),
            Self::TemplateCycle { .. } => None,
        }
    }
}

pub(crate) const NO_3D_SUPPORT: &str =
    "this build of the engine has no 3D support, as it was built without the `3d` feature";

/// The outputs a node has, `output` and those it names, as a clause.
fn outputs_are(named: &[OutputPortDef]) -> String {
    let quoted: Vec<_> = std::iter::once(DEFAULT_OUTPUT)
        .chain(named.iter().map(|def| def.name))
        .map(|name| format!("`{name}`"))
        .collect();
    match quoted.split_last() {
        Some((last, rest)) if !rest.is_empty() => {
            format!("its outputs are {} and {last}", rest.join(", "))
        }
        _ => "its only output is `output`".to_string(),
    }
}

/// What a wrong-kind connection reads, followed by the kind it is.
fn wrong_kind_source(node: NodeId, output: Option<&OutputPortDef>) -> String {
    match output {
        None => format!("node {node} outputs"),
        Some(output) => format!("output `{}` of node {node} is", output.name),
    }
}

/// What to do about wiring a node that outputs `actual` into an input that takes `expected`.
fn wrong_kind_hint(expected: PortKind, actual: PortKind) -> &'static str {
    use PortKind::{Drawing, Geometry, Material, Raster, Scene};
    match (expected, actual) {
        (Material, _) | (_, Material) => "connect a material_3d to the object's `material` input",
        (Raster, Drawing) => "draw the drawing into a raster through a render_2d, and wire that in",
        (Raster, Geometry) => {
            "style the geometry with a fill_2d or stroke_2d, and draw that into a raster through a render_2d"
        }
        (Drawing, Geometry) => "style the geometry with a fill_2d or stroke_2d, and wire that in",
        (Raster, _) => "draw the scene into a raster through a render_3d, and wire that in",
        (Scene, Geometry | Drawing) => {
            "draw it into a raster through a render_2d, and show that in the scene through an image_plane_3d"
        }
        (Scene, _) => "show the raster in the scene through an image_plane_3d",
        (Geometry, Drawing) => "wire in the geometry the drawing's fill_2d or stroke_2d styles",
        (Geometry, _) => "build the outline from path_2d, shape_2d, text_2d and offset_2d nodes",
        (Drawing, _) => "build the drawing from fill_2d, stroke_2d, shadow_2d and merge_2d nodes",
    }
}

#[derive(Debug, Error, Clone, lumen_macros::DiagnosticCode)]
pub enum PropertyError {
    #[code("property.invalid_type")]
    /// `at` is the param with the value.
    #[error("{at}: invalid type: expected {expected}, got {actual}")]
    InvalidType {
        at: At,
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
    /// `at` is the param the expression is in.
    #[error("{at}: {failure}")]
    Located {
        at: At,
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

/// The schema of what [`Serialize`] writes: `limit` and `actual` only for `media.too_large`.
#[cfg(feature = "metadata")]
impl schemars::JsonSchema for MediaFailure {
    fn schema_name() -> std::borrow::Cow<'static, str> {
        "MediaFailure".into()
    }

    fn json_schema(generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        let kind = generator.subschema_for::<AssetKind>();
        let location = generator.subschema_for::<MediaLocation>();
        schemars::json_schema!({
            "description": "A media source that can't be used, the same way for every kind and every host.",
            "type": "object",
            "properties": {
                "code": {
                    "x-lumen-type": "MediaFailureCode",
                    "description": "Why the source can't be used, as its public diagnostic code. `media.decode` is data that should be\nwhat the use reads but is corrupt, truncated or in no format the engine identifies;\n`media.unsupported` is data it identifies and doesn't read, such as a codec it lacks.",
                    "type": "string",
                    "enum": MediaReason::CODES
                },
                "source": {
                    "description": "The source as the composition wrote it: a path, URL, alias or `asset:<id>`.",
                    "type": "string"
                },
                "wanted": {
                    "description": "The kinds the place using the source reads; empty when the host that failed doesn't know.",
                    "type": "array",
                    "items": kind
                },
                "found": {
                    "description": "The kind the source is, when that is established. It is absent when it can't be told,\nsuch as an MP4 that is not video-only or audio-only until opened.",
                    "$ref": kind.get("$ref")
                },
                "at": {
                    "description": "Where the composition uses the source, when known.",
                    "$ref": location.get("$ref")
                },
                "details": { "type": "string" },
                "limit": {
                    "description": "For `media.too_large`, the limit, in the unit `details` names.",
                    "type": "integer",
                    "format": "uint64",
                    "minimum": 0
                },
                "actual": {
                    "description": "For `media.too_large`, the value over the limit, when it is known.",
                    "type": "integer",
                    "format": "uint64",
                    "minimum": 0
                }
            },
            "required": ["code", "source", "wanted", "details"],
            "additionalProperties": false
        })
    }
}

/// What a kind is called in a sentence about one source.
fn kind_noun(kind: AssetKind) -> &'static str {
    match kind {
        AssetKind::Environment => "environment map",
        AssetKind::Lut => "LUT",
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
    #[cfg(feature = "ffmpeg")]
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
    #[error("{} evaluation failed: {details}", .at.of_kind(.node_kind))]
    NodeEvaluation {
        at: At,
        /// The node's `type`, as the document writes it.
        node_kind: &'static str,
        details: String,
    },
    #[code("render.missing_node")]
    #[error("{at} is referenced but missing from the graph")]
    MissingNode { at: At },
    /// A node of a template was asked for by itself, which only root nodes can be.
    #[code("render.template_node")]
    #[error("{at} is in a template, and only nodes of the root can be evaluated by themselves")]
    TemplateNode { at: At },
    /// A node's param was asked for at a composition frame the node isn't evaluated at, because a
    /// clip or switch that leads to it isn't showing it then.
    #[code("render.node_not_evaluated")]
    #[error(
        "{at} is not evaluated at frame {frame}: no clip or switch leading to it shows it then"
    )]
    NodeNotEvaluated { at: At, frame: f64 },
    /// A node's param was asked for at a composition frame at which it is evaluated at several
    /// frames of its own.
    #[code("render.ambiguous_node_frame")]
    #[error(
        "{at} is evaluated at frame {frame} of the composition at several frames of its own: {local}"
    )]
    AmbiguousNodeFrame { at: At, frame: f64, local: String },
    #[code("render.invalid_output_type")]
    #[error("output of {at} has invalid type: expected {expected}, got {actual}")]
    InvalidNodeOutputType {
        at: At,
        expected: &'static str,
        actual: &'static str,
    },
    /// The device, its queue or a driver failed, which is no fault of the composition.
    #[code("render.gpu")]
    #[error("GPU render failed: {details}")]
    Gpu { details: String },
    /// The composition needs more than the device or the engine allows, such as a frame whose data
    /// doesn't fit the device's buffers.
    #[code("render.limit")]
    #[error("{details}")]
    Limit { details: String },
    /// The engine, or a host's resolver or target, broke a contract the render relies on.
    #[code("render.internal")]
    #[error("internal render error: {details}")]
    Internal { details: String },
    #[code("render.too_deeply_nested")]
    #[error(
        "{at} is more than {limit} nodes deep, counting from the output through clips and the compositions they play, so the composition is nested too deeply to render. Flatten it: put many layers in one `raster_multimerge` instead of chaining `merge`s, and combine consecutive transforms or adjustments into one node"
    )]
    TooDeeplyNested { at: At, limit: u32 },
}

impl RenderError {
    /// Node `node_id`, an `N`, failing to evaluate.
    pub(crate) fn evaluating<N: JsonNode>(node_id: NodeId, details: impl Into<String>) -> Self {
        Self::evaluating_kind(node_id, N::KIND, details)
    }

    /// `node` failing to evaluate.
    pub(crate) fn evaluating_node(node: &NodeKind, details: impl Into<String>) -> Self {
        Self::evaluating_kind(crate::node::Node::id(node), node.kind_name(), details)
    }

    fn evaluating_kind(
        node_id: NodeId,
        node_kind: &'static str,
        details: impl Into<String>,
    ) -> Self {
        Self::NodeEvaluation {
            at: At::node(node_id),
            node_kind,
            details: details.into(),
        }
    }

    fn at_mut(&mut self) -> Option<&mut At> {
        match self {
            Self::NodeEvaluation { at, .. }
            | Self::MissingNode { at }
            | Self::TemplateNode { at }
            | Self::NodeNotEvaluated { at, .. }
            | Self::AmbiguousNodeFrame { at, .. }
            | Self::InvalidNodeOutputType { at, .. }
            | Self::TooDeeplyNested { at, .. } => Some(at),
            Self::FrameOutOfRange { .. }
            | Self::Gpu { .. }
            | Self::Limit { .. }
            | Self::Internal { .. } => None,
        }
    }
}

/// Something that renders, but not as the composition asks. Warnings never fail a render.
#[derive(Debug, Clone, PartialEq)]
pub struct Warning {
    /// The node the warning is about.
    pub at: At,
    pub kind: WarningKind,
}

impl Warning {
    pub fn code(&self) -> &'static str {
        self.kind.code()
    }
}

impl fmt::Display for Warning {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{}: {}",
            OfKind(&self.at, self.kind.node_kind()),
            self.kind
        )
    }
}

/// What a [`Warning`] is about.
#[derive(Debug, Error, Clone, PartialEq, lumen_macros::DiagnosticCode)]
pub enum WarningKind {
    /// A text node's family resolved to no font, so its text is shaped in the default family.
    #[code("font.fallback")]
    #[error(
        "font family `{family}` is not available, so its text falls back to `{}`",
        lumen_text::DEFAULT_FONT_FAMILY
    )]
    FontFallback { family: String },
    /// Glyphs of a `text_2d` or `text_3d` node have no outlines, as a font that draws them from
    /// colour or bitmap tables, such as emoji, has none, so they are left out.
    #[code("font.no_outlines")]
    #[error("{glyphs} glyphs have no outlines, such as colour emoji, so they are left out")]
    NoOutlines { glyphs: usize },
    /// A `render_3d` scene has more lights casting shadows than it draws shadows for.
    #[code("scene.shadow_limit")]
    #[error("light nodes {} cast no shadows, past the first 8 lights casting them", .lights.iter().map(ToString::to_string).collect::<Vec<_>>().join(", "))]
    ShadowLimit {
        /// The lights left without shadows, in scene order.
        lights: Vec<NodeId>,
    },
    /// A `render_3d` lists a matte name in `mattes` that no object of its scene takes, so the
    /// channel for it in the `matte` output is 0.
    #[code("scene.matte_unused")]
    #[error("no object takes the matte `{name}`, so its channel of the matte output is 0")]
    MatteUnused { name: String },
    /// A video's colour tags name a transfer or primaries the renderer doesn't convert, so it reads
    /// as SDR or BT.709 in their place, or an image's embedded ICC profile can't be honoured, so it
    /// reads as sRGB.
    #[code("media.color_unsupported")]
    #[error("{} `{media}`: {details}", .kind.name())]
    UnsupportedColor {
        /// `Image` or `Video`.
        kind: crate::media::AssetKind,
        media: String,
        details: String,
    },
    /// A `model_3d`'s model has primitives that aren't triangle lists, which aren't drawn.
    #[code("scene.model_primitives_skipped")]
    #[error(
        "model `{model}` has primitives of points, lines, or triangle strips or fans, which aren't drawn ({count} left out)"
    )]
    ModelPrimitivesSkipped {
        /// The model's source.
        model: String,
        count: usize,
    },
    /// An image is larger on a side than the textures the device makes, or than the host lets
    /// images be decoded at, so it is shown from a copy reduced to fit.
    #[code("media.image_reduced")]
    #[error(
        "image `{image}` is {width}x{height}, past the {limit}-pixel limit on a side, so it is shown reduced to {reduced_width}x{reduced_height}"
    )]
    ImageReduced {
        /// The image's source.
        image: String,
        width: u32,
        height: u32,
        reduced_width: u32,
        reduced_height: u32,
        limit: u32,
    },
}

impl WarningKind {
    /// The kind of node that raises this warning, as its message names it, if only one kind does.
    fn node_kind(&self) -> Option<&'static str> {
        match self {
            Self::FontFallback { .. } => Some("text"),
            Self::NoOutlines { .. } => None,
            Self::ShadowLimit { .. } | Self::MatteUnused { .. } => Some("render_3d"),
            Self::UnsupportedColor { .. } | Self::ImageReduced { .. } => Some("media_in"),
            Self::ModelPrimitivesSkipped { .. } => Some("model_3d"),
        }
    }

    /// What tells two warnings of this kind about one node apart.
    pub(crate) fn identity(&self) -> Option<&str> {
        match self {
            Self::FontFallback { family } => Some(family),
            Self::UnsupportedColor { media, .. } => Some(media),
            Self::ImageReduced { image, .. } => Some(image),
            Self::MatteUnused { name } => Some(name),
            Self::NoOutlines { .. }
            | Self::ShadowLimit { .. }
            | Self::ModelPrimitivesSkipped { .. } => None,
        }
    }
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
            "image `big.png` (composition `card`: node 3 `source`) is too large: wider than the device allows"
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

    #[cfg(feature = "ffmpeg")]
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
