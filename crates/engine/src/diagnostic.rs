//! Structured problems with a composition: what [`crate::json::check`] reports and what a failed
//! [`crate::json::parse`] carries.
//!
//! A diagnostic's `code` comes from the error or warning variant it was made from, and its `path`
//! from the ids and fields that variant holds, so both stay in step with the errors themselves.

use std::fmt::Write as _;

use serde::Serialize;
use serde_json::Value;

use crate::{
    error::{
        At, DocumentError, ExpressionError, GraphValidationError, LumenError, MediaError,
        MediaReason, PropertyError, RenderError, Warning, WarningKind,
    },
    media::MediaLocation,
    node::NodeId,
};

/// The most diagnostics one check or parse reports.
pub const MAX_DIAGNOSTICS: usize = 50;

/// Whether a diagnostic stops a composition rendering.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The composition doesn't render.
    Error,
    /// The composition renders, but not as it asks.
    Warning,
}

/// A problem the engine found with a composition. Its `code` is stable and public, and new codes
/// can appear in any release, so treat one you don't know by its severity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable and public, such as `node.unknown_param` or `font.fallback`. New codes can appear in
    /// any release.
    pub code: &'static str,
    /// A JSON pointer into the composition, such as `/nodes/3/params/opacity`. Empty for the
    /// whole document.
    pub path: String,
    /// The node the diagnostic is about, in the graph `path` points into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeId>,
    /// Free-form text for people to read, which can change in any release.
    pub message: String,
}

impl Diagnostic {
    /// `error` as a diagnostic, with its path into `document`, the JSON it was read from.
    pub fn from_error(error: &LumenError, document: &Value) -> Self {
        let (path, node) = locate(error, document);
        Self {
            severity: Severity::Error,
            code: error.code(),
            path,
            node,
            message: error.to_string(),
        }
    }

    /// `warning` as a diagnostic about the value at `path`.
    pub fn from_warning(warning: &Warning, path: String) -> Self {
        Self {
            severity: Severity::Warning,
            code: warning.code(),
            path,
            node: warning.at.node,
            message: warning.to_string(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Every code a diagnostic can have.
pub fn codes() -> impl Iterator<Item = &'static str> {
    code_list().map(|(code, _)| code)
}

/// Every code a diagnostic can have, with the severity of the diagnostics that carry it.
pub fn code_list() -> impl Iterator<Item = (&'static str, Severity)> {
    let errors = [
        DocumentError::CODES,
        GraphValidationError::CODES,
        PropertyError::CODES,
        ExpressionError::CODES,
        MediaError::CODES,
        MediaReason::CODES,
        RenderError::CODES,
    ]
    .into_iter()
    .flatten()
    .map(|code| (*code, Severity::Error));
    let warnings = WarningKind::CODES
        .iter()
        .map(|code| (*code, Severity::Warning));
    errors.chain(warnings)
}

/// Where a diagnostic with a code comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FoundBy {
    /// [`crate::json::check`] finds it, without rendering.
    Check,
    /// Only rendering finds it.
    Render,
    /// Both find it.
    CheckAndRender,
}

impl FoundBy {
    /// How the codes' documentation spells it.
    pub const fn name(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Render => "render",
            Self::CheckAndRender => "check, render",
        }
    }
}

/// What a diagnostic code means, for documentation.
#[derive(Debug, Clone, Copy)]
pub struct CodeDoc {
    pub code: &'static str,
    /// Where a diagnostic with this code comes from.
    pub found_by: FoundBy,
    /// Markdown.
    pub meaning: &'static str,
}

/// Every code with what it means, in the order the documentation lists them. A test keeps it in
/// step with [`codes`].
pub const CODE_DOCS: &[CodeDoc] = &[
    CodeDoc {
        code: "document.syntax",
        found_by: FoundBy::Check,
        meaning: "The text is not JSON.",
    },
    CodeDoc {
        code: "document.schema_version",
        found_by: FoundBy::Check,
        meaning: "`lumenSchemaVersion` is not a string, or names a version outside this engine's Lumen line.",
    },
    CodeDoc {
        code: "document.invalid",
        found_by: FoundBy::Check,
        meaning: "A key or value outside node params has the wrong shape, is missing or is unknown.",
    },
    CodeDoc {
        code: "expression.function",
        found_by: FoundBy::Check,
        meaning: "A function in `functions` doesn't parse, or is defined twice or recursively.",
    },
    CodeDoc {
        code: "node.unknown_type",
        found_by: FoundBy::Check,
        meaning: "A node's `type` is not a node type.",
    },
    CodeDoc {
        code: "node.invalid_params",
        found_by: FoundBy::Check,
        meaning: "A node's `params` is not an object.",
    },
    CodeDoc {
        code: "node.unknown_param",
        found_by: FoundBy::Check,
        meaning: "A node has a param its type doesn't.",
    },
    CodeDoc {
        code: "node.invalid_param",
        found_by: FoundBy::Check,
        meaning: "A param's value is wrong for it, including expressions that don't parse.",
    },
    CodeDoc {
        code: "node.unsupported",
        found_by: FoundBy::Check,
        meaning: "A node needs what this build of the engine was built without: any 3D node, without the `3d` feature.",
    },
    CodeDoc {
        code: "graph.duplicate_node_id",
        found_by: FoundBy::Check,
        meaning: "Two nodes of one graph share an id.",
    },
    CodeDoc {
        code: "connection.invalid_from_port",
        found_by: FoundBy::Check,
        meaning: "A connection's `from_port` is not `output`.",
    },
    CodeDoc {
        code: "connection.missing_source_node",
        found_by: FoundBy::Check,
        meaning: "A connection or input reads a node that doesn't exist.",
    },
    CodeDoc {
        code: "connection.missing_target_node",
        found_by: FoundBy::Check,
        meaning: "A connection writes to a node that doesn't exist.",
    },
    CodeDoc {
        code: "connection.unknown_port",
        found_by: FoundBy::Check,
        meaning: "A connection writes to an input its node doesn't have.",
    },
    CodeDoc {
        code: "connection.input_already_connected",
        found_by: FoundBy::Check,
        meaning: "A single input is connected twice.",
    },
    CodeDoc {
        code: "connection.wrong_kind",
        found_by: FoundBy::Check,
        meaning: "A connection wires a node into an input of another kind: a scene into an input that takes a raster, a raster into one that takes a scene, or a material anywhere but a `material` input.",
    },
    CodeDoc {
        code: "graph.node_id_mismatch",
        found_by: FoundBy::Check,
        meaning: "A node is stored under another id. Only builders made in Rust can do this.",
    },
    CodeDoc {
        code: "graph.missing_required_input",
        found_by: FoundBy::Check,
        meaning: "A required input is not connected.",
    },
    CodeDoc {
        code: "graph.missing_media_output",
        found_by: FoundBy::Check,
        meaning: "A graph has no `media_output` node.",
    },
    CodeDoc {
        code: "graph.multiple_media_outputs",
        found_by: FoundBy::Check,
        meaning: "A graph has more than one `media_output` node.",
    },
    CodeDoc {
        code: "graph.cycle",
        found_by: FoundBy::Check,
        meaning: "Nodes read each other in a cycle.",
    },
    CodeDoc {
        code: "clip.invalid",
        found_by: FoundBy::Check,
        meaning: "A clip names a composition that doesn't exist, sets props it doesn't declare or of the wrong kind, plays a composition and a `source` together, or has a negative `duration` other than -1.",
    },
    CodeDoc {
        code: "expression.invalid_prop_read",
        found_by: FoundBy::Check,
        meaning: "`prop()` is used outside a template, names an undeclared prop, or doesn't name it with a string literal.",
    },
    CodeDoc {
        code: "settings.invalid",
        found_by: FoundBy::Check,
        meaning: "A timeline, canvas or prop default can't render, such as an `fps` of 0.",
    },
    CodeDoc {
        code: "audio.invalid",
        found_by: FoundBy::Check,
        meaning: "Audio tracks or clips are duplicated, unknown, too many in one composition, or have invalid timing or gain, or a gain expression reads a node, a glyph, text or an undeclared prop.",
    },
    CodeDoc {
        code: "composition.cycle",
        found_by: FoundBy::Check,
        meaning: "Templates instantiate themselves.",
    },
    CodeDoc {
        code: "font.fallback",
        found_by: FoundBy::CheckAndRender,
        meaning: "A `text` or `text_3d` node's font family resolved to no font, so it renders in a fallback.",
    },
    CodeDoc {
        code: "font.no_outlines",
        found_by: FoundBy::Render,
        meaning: "A `text_2d` or `text_3d` node's glyphs have no outlines, such as colour emoji, so they are left out.",
    },
    CodeDoc {
        code: "scene.shadow_limit",
        found_by: FoundBy::Render,
        meaning: "A `render_3d` scene has more than 8 lights casting shadows, so those past the first 8 cast none.",
    },
    CodeDoc {
        code: "scene.model_primitives_skipped",
        found_by: FoundBy::Render,
        meaning: "A `model_3d`'s model has points, lines, or triangle strips or fans, which aren't drawn.",
    },
    CodeDoc {
        code: "media.color_unsupported",
        found_by: FoundBy::Render,
        meaning: "A video's colour tags name a transfer or primaries the engine doesn't convert, so it renders as SDR or BT.709 in their place, or an image's ICC profile can't be honoured, so it renders as sRGB.",
    },
    CodeDoc {
        code: "media.image_reduced",
        found_by: FoundBy::Render,
        meaning: "An image read only by `resize`, `transform` and `image_plane_3d` nodes is larger on a side than the device's texture limit, or the host's `set_max_image_side`, so it is shown from a copy reduced to fit.",
    },
    CodeDoc {
        code: "property.invalid_type",
        found_by: FoundBy::Render,
        meaning: "An expression gave a param a value of the wrong type.",
    },
    CodeDoc {
        code: "expression.parse",
        found_by: FoundBy::Render,
        meaning: "An expression built at render time doesn't parse.",
    },
    CodeDoc {
        code: "expression.evaluate",
        found_by: FoundBy::Render,
        meaning: "An expression failed to evaluate.",
    },
    CodeDoc {
        code: "media.not_found",
        found_by: FoundBy::CheckAndRender,
        meaning: "A media source isn't there: a path or alias the host doesn't have, or an `asset:` id its asset store doesn't have.",
    },
    CodeDoc {
        code: "media.wrong_kind",
        found_by: FoundBy::CheckAndRender,
        meaning: "A media source is another kind than the place using it reads, such as an `asset:` image used as a model, or a video file given where an image is read.",
    },
    CodeDoc {
        code: "media.unsupported",
        found_by: FoundBy::Render,
        meaning: "A media source is the right kind but in a format or with a feature the engine doesn't read: an unknown container or codec, BT.2020 constant-luminance video, a glTF using a required extension, a `.gltf` JSON file, a WOFF font.",
    },
    CodeDoc {
        code: "media.too_large",
        found_by: FoundBy::Render,
        meaning: "A media source is over a limit, in bytes, pixels or counts, or is bigger than the device can hold. The limit and, when known, the actual value are in the failure.",
    },
    CodeDoc {
        code: "media.decode",
        found_by: FoundBy::Render,
        meaning: "A media source is meant to be what the place reads but can't be read: it is corrupt, truncated or in no format the engine can identify.",
    },
    CodeDoc {
        code: "media.frame_out_of_range",
        found_by: FoundBy::Render,
        meaning: "A video frame past the end of its source was asked for.",
    },
    CodeDoc {
        code: "audio.unknown_track",
        found_by: FoundBy::Render,
        meaning: "An audio clip names a track that doesn't exist.",
    },
    CodeDoc {
        code: "audio.limit",
        found_by: FoundBy::Render,
        meaning: "Audio is more than the plan can hold: too many clips playing sound at once, a loop restarting too often, too many segments in one plan, or a preview timeline too long.",
    },
    CodeDoc {
        code: "render.frame_out_of_range",
        found_by: FoundBy::Render,
        meaning: "A frame past the composition's end was asked for.",
    },
    CodeDoc {
        code: "render.node_evaluation",
        found_by: FoundBy::Render,
        meaning: "A node failed to render, such as text with too many glyphs or a `wgsl_shader` that doesn't compile.",
    },
    CodeDoc {
        code: "render.missing_node",
        found_by: FoundBy::Render,
        meaning: "A node read while rendering, or one whose param is evaluated by itself, doesn't exist.",
    },
    CodeDoc {
        code: "render.template_node",
        found_by: FoundBy::Render,
        meaning: "A node of a template was asked for by itself, such as to evaluate one of its params. Only nodes of the root can be.",
    },
    CodeDoc {
        code: "render.node_not_evaluated",
        found_by: FoundBy::Render,
        meaning: "A node's param was asked for at a frame at which no clip or switch leading to the node shows it.",
    },
    CodeDoc {
        code: "render.ambiguous_node_frame",
        found_by: FoundBy::Render,
        meaning: "A node's param was asked for at a frame at which the node is evaluated at several frames of its own, through clips with different timing, a time remap or a motion blur.",
    },
    CodeDoc {
        code: "render.invalid_output_type",
        found_by: FoundBy::Render,
        meaning: "A node's output had the wrong type for its reader.",
    },
    CodeDoc {
        code: "render.gpu",
        found_by: FoundBy::Render,
        meaning: "The GPU failed: the device, its queue or a driver. Nothing the composition says causes it, so trying again may succeed.",
    },
    CodeDoc {
        code: "render.limit",
        found_by: FoundBy::Render,
        meaning: "The composition needs more than the device or engine allows, such as a frame whose data is too large for the device's buffers or a video frame larger than its texture limit.",
    },
    CodeDoc {
        code: "render.internal",
        found_by: FoundBy::Render,
        meaning: "The engine, or a host's resolver or render target, broke a contract the render relies on, such as a poisoned lock or a stale recorded frame uploaded. Not the composition's fault. Trying again helps only for fonts that kept loading, which stay loaded for the next render.",
    },
    CodeDoc {
        code: "render.too_deeply_nested",
        found_by: FoundBy::Render,
        meaning: "A node is more than 48 nodes deep, counting through clips. Flatten the chain: put many layers in one `raster_multimerge` instead of chaining `merge`s, and combine consecutive transforms or adjustments.",
    },
];

/// Every code's [`CodeDoc`] with the severity of the diagnostics that carry it, in the order of
/// [`CODE_DOCS`].
pub fn code_docs() -> impl Iterator<Item = (&'static CodeDoc, Severity)> {
    CODE_DOCS.iter().map(|doc| {
        let (_, severity) = code_list()
            .find(|(code, _)| *code == doc.code)
            .expect("every documented code is a code");
        (doc, severity)
    })
}

/// Escapes a JSON pointer reference token.
pub(crate) fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// The JSON pointer of `at`, with `rest` after it. `find` gives a node's index in its scene's
/// `nodes`, and its `params` when they are at hand, or `None` when the scene has no such node, in
/// which case the pointer is the scene's `nodes`.
pub(crate) fn pointer<'v>(
    at: &At,
    rest: &str,
    find: impl FnOnce(NodeId) -> Option<(usize, Option<&'v Value>)>,
) -> String {
    let mut pointer = at.template.as_deref().map_or_else(String::new, |name| {
        format!("/compositions/{}", escape(name))
    });
    let Some(node) = at.node else {
        pointer.push_str(rest);
        return pointer;
    };
    let Some((index, params)) = find(node) else {
        pointer.push_str("/nodes");
        return pointer;
    };
    let _ = write!(pointer, "/nodes/{index}");
    if let Some(param) = &at.param {
        pointer.push_str("/params");
        pointer.push_str(&param_pointer(params, param));
    }
    pointer.push_str(rest);
    pointer
}

/// The [`pointer`] of `at` in `document`, the JSON of the whole composition.
pub(crate) fn document_pointer(document: &Value, at: &At, rest: &str) -> String {
    let scene = template_scene(document, at.template.as_deref());
    pointer(at, rest, |id| {
        let index = node_index(scene, id)?;
        Some((index, scene.get("nodes")?.get(index)?.get("params")))
    })
}

/// The pointer of a param path such as `content[1].font_size`, as `/content/1/font_size`. Keys may
/// hold `.` and `[`, so where `params` has an object, each field is matched against the longest of
/// its keys the path can continue from, and `[n]` is an index only where it can't.
fn param_pointer(params: Option<&Value>, path: &str) -> String {
    let mut pointer = String::new();
    let mut value = params;
    let mut rest = path;
    while !rest.is_empty() {
        let field = rest.strip_prefix('.').unwrap_or(rest);
        let continues = |key: &str| {
            field
                .strip_prefix(key)
                .is_some_and(|after| after.is_empty() || after.starts_with(['.', '[']))
        };
        let key = value.and_then(Value::as_object).and_then(|object| {
            object
                .keys()
                .filter(|key| !key.is_empty() && continues(key))
                .max_by_key(|key| key.len())
        });
        if let Some(key) = key {
            pointer.push('/');
            pointer.push_str(&escape(key));
            value = value.and_then(|value| value.get(key));
            rest = &field[key.len()..];
            continue;
        }
        if let Some((index, after)) = rest
            .strip_prefix('[')
            .and_then(|after| after.split_once(']'))
        {
            pointer.push('/');
            pointer.push_str(&escape(index));
            value = value.and_then(|value| value.get(index.parse::<usize>().ok()?));
            rest = after;
            continue;
        }
        let key = &field[..field.find(['.', '[']).unwrap_or(field.len())];
        pointer.push('/');
        if key.is_empty() {
            pointer.push_str(&escape(field));
            break;
        }
        pointer.push_str(&escape(key));
        value = value.and_then(|value| value.get(key));
        rest = &field[key.len()..];
    }
    pointer
}

/// The index of node `id` in `scene`'s `nodes`.
fn node_index(scene: &Value, id: NodeId) -> Option<usize> {
    scene
        .get("nodes")?
        .as_array()?
        .iter()
        .position(|node| node.get("id").and_then(Value::as_u64) == Some(id.0))
}

/// The JSON of `template` in `root`, or `root` itself for the root.
fn template_scene<'a>(root: &'a Value, template: Option<&str>) -> &'a Value {
    match template {
        Some(name) => root
            .get("compositions")
            .and_then(|compositions| compositions.get(name))
            .unwrap_or(&Value::Null),
        None => root,
    }
}

/// Where in `document` the problem `error` describes is, and which node it's about.
fn locate(error: &LumenError, document: &Value) -> (String, Option<NodeId>) {
    let at = |at: &At, rest: &str| (document_pointer(document, at, rest), at.node);
    let in_scene = |at: &At, rest: &str| (document_pointer(document, &at.scene(), rest), at.node);
    match error {
        LumenError::Document(error) => match error {
            DocumentError::Syntax { .. } => (String::new(), None),
            DocumentError::SchemaVersion { .. } => ("/lumenSchemaVersion".to_string(), None),
            DocumentError::Function { .. } => ("/functions".to_string(), None),
            DocumentError::Invalid {
                at: place, pointer, ..
            } => at(place, pointer),
            DocumentError::UnknownNodeType { at: place, .. } => at(place, "/type"),
            DocumentError::ParamsNotObject { at: place, .. }
            | DocumentError::UnknownParam { at: place }
            | DocumentError::InvalidParam { at: place, .. } => at(place, ""),
            DocumentError::DuplicateNodeId { at: place, index } => {
                in_scene(place, &format!("/nodes/{index}"))
            }
            DocumentError::InvalidFromPort {
                at: place, index, ..
            } => at(place, &format!("/connections/{index}/from_port")),
            DocumentError::Connection {
                at: place,
                index,
                to_node,
                error,
                ..
            } => {
                let field = match error.as_ref() {
                    GraphValidationError::MissingSourceNode { .. } => "from_node",
                    GraphValidationError::MissingTargetNode { .. } => "to_node",
                    _ => "to_port",
                };
                let path =
                    document_pointer(document, place, &format!("/connections/{index}/{field}"));
                (path, Some(*to_node))
            }
        },
        LumenError::GraphValidation(error) => match error {
            GraphValidationError::Cycle { at: place, path } => match path.first() {
                Some(node) => at(
                    &At {
                        node: Some(*node),
                        ..place.clone()
                    },
                    "",
                ),
                None => at(place, "/nodes"),
            },
            GraphValidationError::WrongInputKind {
                at: place,
                port,
                source_node,
                ..
            } => {
                let scene = template_scene(document, place.template.as_deref());
                match place
                    .node
                    .and_then(|to| connection_index(scene, *source_node, to, port))
                {
                    Some(index) => in_scene(place, &format!("/connections/{index}")),
                    None => at(place, ""),
                }
            }
            GraphValidationError::MissingMediaOutput { at: place }
            | GraphValidationError::MultipleMediaOutputs { at: place, .. } => at(place, "/nodes"),
            GraphValidationError::No3dSupport { at: place, .. } => at(place, "/type"),
            GraphValidationError::InvalidSettings {
                at: place, field, ..
            } => at(place, field),
            GraphValidationError::InvalidAudio { at: place, .. } => at(place, "/audio"),
            GraphValidationError::TemplateCycle { templates } => {
                let first = At::default().in_template(templates.first().map(String::as_str));
                at(&first, "")
            }
            GraphValidationError::NodeIdMismatch { at: place, .. }
            | GraphValidationError::MissingTargetNode { at: place }
            | GraphValidationError::MissingSourceNode { at: place }
            | GraphValidationError::MissingTargetPort { at: place, .. }
            | GraphValidationError::InputAlreadyConnected { at: place, .. }
            | GraphValidationError::MissingRequiredInput { at: place, .. }
            | GraphValidationError::InvalidClip { at: place, .. }
            | GraphValidationError::InvalidPropRead { at: place, .. } => at(place, ""),
        },
        LumenError::Property(PropertyError::InvalidType { at: place, .. })
        | LumenError::Expression(ExpressionError::Located { at: place, .. }) => at(place, ""),
        LumenError::Expression(
            ExpressionError::Parse { .. } | ExpressionError::Evaluate { .. },
        ) => (String::new(), None),
        LumenError::Media(error) => match error.failure().and_then(|failure| failure.at.as_ref()) {
            Some(MediaLocation::Node(node)) => at(&node.at(), ""),
            Some(MediaLocation::AudioClip {
                template, index, ..
            }) => at(
                &At::default().in_template(template.as_deref()),
                &format!("/audio/clips/{index}/source_id"),
            ),
            None => (String::new(), None),
        },
        LumenError::Render(error) => match error {
            RenderError::NodeEvaluation { at: place, .. }
            | RenderError::MissingNode { at: place }
            | RenderError::TemplateNode { at: place }
            | RenderError::NodeNotEvaluated { at: place, .. }
            | RenderError::AmbiguousNodeFrame { at: place, .. }
            | RenderError::InvalidNodeOutputType { at: place, .. }
            | RenderError::TooDeeplyNested { at: place, .. } => at(place, ""),
            RenderError::FrameOutOfRange { .. }
            | RenderError::Gpu { .. }
            | RenderError::Limit { .. }
            | RenderError::Internal { .. } => (String::new(), None),
        },
    }
}

/// The index in `scene`'s `connections` of the one from `from` into `port` of `to`.
fn connection_index(scene: &Value, from: NodeId, to: NodeId, port: &str) -> Option<usize> {
    scene
        .get("connections")?
        .as_array()?
        .iter()
        .position(|connection| {
            connection.get("from_node").and_then(Value::as_u64) == Some(from.0)
                && connection.get("to_node").and_then(Value::as_u64) == Some(to.0)
                && connection.get("to_port").and_then(Value::as_str) == Some(port)
        })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn param_paths_become_pointers() {
        assert_eq!(param_pointer(None, ""), "");
        assert_eq!(param_pointer(None, "opacity"), "/opacity");
        assert_eq!(
            param_pointer(None, "content[1].font_size"),
            "/content/1/font_size"
        );
        assert_eq!(param_pointer(None, "uniforms.a/b"), "/uniforms/a~1b");
        // Keys holding `.` or `[` are found in the params themselves.
        let params = serde_json::json!({ "props": { "a.b": 1, "a": { "b": 2 }, "c[0]": [3] } });
        assert_eq!(param_pointer(Some(&params), "props.a.b"), "/props/a.b");
        assert_eq!(
            param_pointer(Some(&params), "props.c[0][0]"),
            "/props/c[0]/0"
        );
        assert_eq!(param_pointer(Some(&params), "props.["), "/props/[");
        let params = serde_json::json!({ "content": [{ "font_features": { "[00]": "=1" } }] });
        assert_eq!(
            param_pointer(Some(&params), "content[0].font_features[00]"),
            "/content/0/font_features/[00]"
        );
    }

    #[test]
    fn a_place_reads_and_points_the_same_way_in_the_root_and_in_a_template() {
        let document = serde_json::json!({
            "nodes": [{ "id": 7 }, { "id": 3, "params": { "a.b": 1 } }],
            "compositions": { "card/x": { "nodes": [{ "id": 3 }] } },
        });
        let node = NodeId::new(3);
        let card = Some("card/x");
        for (at, shown, pointer) in [
            (At::default(), "", ""),
            (At::node(node), "node 3", "/nodes/1"),
            (
                At::param(node, "a.b"),
                "node 3 `a.b`",
                "/nodes/1/params/a.b",
            ),
            (
                At::default().in_template(card),
                "composition `card/x`",
                "/compositions/card~1x",
            ),
            (
                At::node(node).in_template(card),
                "composition `card/x`: node 3",
                "/compositions/card~1x/nodes/0",
            ),
            (
                At::param(node, "content[1].size").in_template(card),
                "composition `card/x`: node 3 `content[1].size`",
                "/compositions/card~1x/nodes/0/params/content/1/size",
            ),
        ] {
            assert_eq!(at.to_string(), shown);
            assert_eq!(document_pointer(&document, &at, ""), pointer, "{shown}");
        }
    }

    #[test]
    fn a_wrong_kind_points_at_its_connection() {
        let document = serde_json::json!({ "connections": [
            { "from_node": 1, "to_node": 3, "to_port": "source" },
            { "from_node": 2, "to_node": 3, "to_port": "source" },
        ] });
        let error = LumenError::from(GraphValidationError::WrongInputKind {
            at: At::node(NodeId::new(3)),
            port: "source".to_string(),
            source_node: NodeId::new(2),
            expected: crate::node::PortKind::Scene,
            actual: crate::node::PortKind::Raster,
        });
        let diagnostic = Diagnostic::from_error(&error, &document);
        assert_eq!(diagnostic.path, "/connections/1");
        assert_eq!(diagnostic.node, Some(NodeId::new(3)));
    }

    #[test]
    fn every_code_is_unique_and_documented_once() {
        let mut seen = BTreeSet::new();
        for code in codes() {
            assert!(seen.insert(code), "`{code}` is used twice");
        }
        let mut documented = BTreeSet::new();
        for doc in CODE_DOCS {
            assert!(
                documented.insert(doc.code),
                "`{}` is documented twice",
                doc.code
            );
        }
        assert_eq!(documented, seen, "`CODE_DOCS` and the codes differ");
    }
}
