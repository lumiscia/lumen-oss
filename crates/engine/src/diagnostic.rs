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
    let warnings = WarningKind::CODES.iter().map(|code| (*code, Severity::Warning));
    errors.chain(warnings)
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
    fn every_code_is_unique_and_documented() {
        let mut seen = std::collections::BTreeSet::new();
        for code in codes() {
            assert!(seen.insert(code), "`{code}` is used twice");
        }
        let documented = include_str!("../README.md")
            .lines()
            .filter_map(|line| {
                let cell = line.strip_prefix('|')?.split('|').next()?.trim();
                cell.strip_prefix('`')?.strip_suffix('`')
            })
            .collect::<std::collections::BTreeSet<_>>();
        assert_eq!(
            documented, seen,
            "README.md's table of codes is out of date"
        );
    }
}
