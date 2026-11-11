//! Structured problems with a composition: what [`crate::json::check`] reports and what a failed
//! [`crate::json::parse`] carries.
//!
//! A diagnostic's `code` comes from the error or warning variant it was made from, and its `path`
//! from the ids and fields that variant holds, so both stay in step with the errors themselves.

use serde::Serialize;
use serde_json::Value;

use crate::{
    error::{
        DocumentError, ExpressionError, GraphValidationError, LumenError, MediaError, MediaReason,
        PropertyError, RenderError, Warning,
    },
    media::MediaLocation,
    node::NodeId,
};

/// The most diagnostics one check or parse reports.
pub const MAX_DIAGNOSTICS: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    /// The composition doesn't render.
    Error,
    /// The composition renders, but not as it asks.
    Warning,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Diagnostic {
    pub severity: Severity,
    /// Stable and public; see the crate README for the list.
    pub code: &'static str,
    /// A JSON pointer into the composition, such as `/nodes/3/params/opacity`. Empty for the
    /// whole document.
    pub path: String,
    /// The node the diagnostic is about, in the graph `path` points into.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub node: Option<NodeId>,
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
        let node = match warning {
            Warning::FontFallback { node_id, .. }
            | Warning::NoOutlines { node_id, .. }
            | Warning::ShadowLimit { node_id, .. }
            | Warning::UnsupportedVideoColor { node_id, .. }
            | Warning::ModelPrimitivesSkipped { node_id, .. }
            | Warning::ImageReduced { node_id, .. } => Some(*node_id),
        };
        Self {
            severity: Severity::Warning,
            code: warning.code(),
            path,
            node,
            message: warning.to_string(),
        }
    }

    pub fn is_error(&self) -> bool {
        self.severity == Severity::Error
    }
}

/// Every code a diagnostic can have.
pub fn codes() -> impl Iterator<Item = &'static str> {
    [
        DocumentError::CODES,
        GraphValidationError::CODES,
        PropertyError::CODES,
        ExpressionError::CODES,
        MediaError::CODES,
        MediaReason::CODES,
        RenderError::CODES,
        Warning::CODES,
    ]
    .into_iter()
    .flatten()
    .copied()
}

/// Escapes a JSON pointer reference token.
pub(crate) fn escape(token: &str) -> String {
    token.replace('~', "~0").replace('/', "~1")
}

/// `/compositions/<name>` for a template, empty at the root.
pub(crate) fn scene_pointer(template: Option<&str>) -> String {
    template.map_or_else(String::new, |name| {
        format!("/compositions/{}", escape(name))
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

/// `path`, a param path, under node `id` of `template` in `root`, or of the root.
pub(crate) fn at_param(
    root: &Value,
    template: Option<&str>,
    id: NodeId,
    path: &str,
) -> (String, Option<NodeId>) {
    let (pointer, node) = at_scene_param(template_scene(root, template), id, path);
    (format!("{}{pointer}", scene_pointer(template)), node)
}

/// `path`, a param path, under node `id` of `scene`.
fn at_scene_param(scene: &Value, id: NodeId, path: &str) -> (String, Option<NodeId>) {
    let params =
        node_index(scene, id).and_then(|index| scene.get("nodes")?.get(index)?.get("params"));
    at_node(
        scene,
        id,
        &format!("/params{}", param_pointer(params, path)),
    )
}

/// The index of node `id` in `scene`'s `nodes`.
pub(crate) fn node_index(scene: &Value, id: NodeId) -> Option<usize> {
    scene
        .get("nodes")?
        .as_array()?
        .iter()
        .position(|node| node.get("id").and_then(Value::as_u64) == Some(id.0))
}

/// `rest` under node `id` of `scene`, or `scene`'s `nodes` when it has no such node.
fn at_node(scene: &Value, id: NodeId, rest: &str) -> (String, Option<NodeId>) {
    let path = match node_index(scene, id) {
        Some(index) => format!("/nodes/{index}{rest}"),
        None => "/nodes".to_string(),
    };
    (path, Some(id))
}

/// The connection in `scene` from `from` into `port` of `to`, or node `to` when no connection
/// names it.
fn at_connection(scene: &Value, from: NodeId, to: NodeId, port: &str) -> (String, Option<NodeId>) {
    let is_it = |connection: &Value| {
        connection.get("from_node").and_then(Value::as_u64) == Some(from.0)
            && connection.get("to_node").and_then(Value::as_u64) == Some(to.0)
            && connection.get("to_port").and_then(Value::as_str) == Some(port)
    };
    let index = scene
        .get("connections")
        .and_then(Value::as_array)
        .and_then(|connections| connections.iter().position(is_it));
    match index {
        Some(index) => (format!("/connections/{index}"), Some(to)),
        None => at_node(scene, to, ""),
    }
}

/// Where in `scene` the problem `error` describes is, and which node it's about.
fn locate(error: &LumenError, scene: &Value) -> (String, Option<NodeId>) {
    match error {
        LumenError::Document(error) => locate_document(error, scene),
        LumenError::GraphValidation(error) => locate_graph(error, scene),
        LumenError::Property(PropertyError::InvalidType {
            node_id,
            property_path,
            ..
        }) => at_scene_param(scene, *node_id, property_path),
        LumenError::Expression(error) => locate_expression(error, scene),
        LumenError::Media(error) => locate_media(error, scene),
        LumenError::Render(error) => match error {
            RenderError::NodeEvaluation { node_id, .. }
            | RenderError::MissingNode { node_id }
            | RenderError::InvalidNodeOutputType { node_id, .. } => at_node(scene, *node_id, ""),
            RenderError::TooDeeplyNested {
                template, node_id, ..
            } => {
                let template = template.as_deref();
                let (path, node) = at_node(template_scene(scene, template), *node_id, "");
                (format!("{}{path}", scene_pointer(template)), node)
            }
            RenderError::FrameOutOfRange { .. }
            | RenderError::Gpu { .. }
            | RenderError::Limit { .. }
            | RenderError::Internal { .. } => (String::new(), None),
        },
    }
}

fn locate_document(error: &DocumentError, scene: &Value) -> (String, Option<NodeId>) {
    match error {
        DocumentError::Syntax { .. } => (String::new(), None),
        DocumentError::SchemaVersion { .. } => ("/lumenSchemaVersion".to_string(), None),
        DocumentError::Invalid { pointer, .. } => (pointer.clone(), None),
        DocumentError::Function { .. } => ("/functions".to_string(), None),
        DocumentError::UnknownNodeType { node_id, .. } => at_node(scene, *node_id, "/type"),
        DocumentError::ParamsNotObject { node_id, .. } => at_node(scene, *node_id, "/params"),
        DocumentError::UnknownParam { node_id, param }
        | DocumentError::InvalidParam { node_id, param, .. } => {
            at_node(scene, *node_id, &format!("/params/{}", escape(param)))
        }
        DocumentError::DuplicateNodeId { node_id, index } => {
            (format!("/nodes/{index}"), Some(*node_id))
        }
        DocumentError::InvalidFromPort { index, .. } => {
            (format!("/connections/{index}/from_port"), None)
        }
        DocumentError::Connection {
            index,
            to_node,
            error,
            ..
        } => {
            let field = match error {
                GraphValidationError::MissingSourceNode { .. } => "/from_node",
                GraphValidationError::MissingTargetNode { .. } => "/to_node",
                _ => "/to_port",
            };
            (format!("/connections/{index}{field}"), Some(*to_node))
        }
    }
}

fn locate_graph(error: &GraphValidationError, scene: &Value) -> (String, Option<NodeId>) {
    let in_template = |template: &Option<String>, rest: &str, node: Option<NodeId>| {
        let scene = template_scene(scene, template.as_deref());
        match node {
            Some(id) => {
                let (path, node) = at_node(scene, id, rest);
                (
                    format!("{}{path}", scene_pointer(template.as_deref())),
                    node,
                )
            }
            None => (
                format!("{}{rest}", scene_pointer(template.as_deref())),
                None,
            ),
        }
    };
    match error {
        GraphValidationError::Cycle { path } => match path.first() {
            Some(node) => at_node(scene, *node, ""),
            None => ("/nodes".to_string(), None),
        },
        GraphValidationError::NodeIdMismatch { key: node_id, .. }
        | GraphValidationError::MissingRequiredInput { node_id, .. }
        | GraphValidationError::MissingTargetNode { node_id }
        | GraphValidationError::MissingSourceNode { node_id }
        | GraphValidationError::MissingTargetPort { node_id, .. }
        | GraphValidationError::InputAlreadyConnected { node_id, .. } => {
            at_node(scene, *node_id, "")
        }
        GraphValidationError::No3dSupport { node_id, .. } => at_node(scene, *node_id, "/type"),
        GraphValidationError::WrongInputKind {
            node_id,
            port,
            source_node,
            ..
        } => at_connection(scene, *source_node, *node_id, port),
        GraphValidationError::MissingMediaOutput
        | GraphValidationError::MultipleMediaOutputs { .. } => ("/nodes".to_string(), None),
        GraphValidationError::InvalidClip {
            template, node_id, ..
        }
        | GraphValidationError::InvalidPropRead {
            template, node_id, ..
        } => in_template(template, "/params", Some(*node_id)),
        GraphValidationError::InvalidSettings {
            template, field, ..
        } => in_template(template, field, None),
        GraphValidationError::InvalidAudio { .. } => ("/audio".to_string(), None),
        GraphValidationError::TemplateCycle { templates } => {
            (scene_pointer(templates.first().map(String::as_str)), None)
        }
        GraphValidationError::InTemplate { template, error } => {
            let (path, node) = locate(error, template_scene(scene, Some(template)));
            (format!("{}{path}", scene_pointer(Some(template))), node)
        }
    }
}

/// Where the composition names the source of `error`, when the error says.
fn locate_media(error: &MediaError, root: &Value) -> (String, Option<NodeId>) {
    match error.failure().and_then(|failure| failure.at.as_ref()) {
        Some(MediaLocation::Node(at)) => at_param(root, at.template.as_deref(), at.node, &at.param),
        Some(MediaLocation::AudioClip {
            template, index, ..
        }) => (
            format!(
                "{}/audio/clips/{index}/source_id",
                scene_pointer(template.as_deref())
            ),
            None,
        ),
        None => (String::new(), None),
    }
}

fn locate_expression(error: &ExpressionError, scene: &Value) -> (String, Option<NodeId>) {
    match error {
        ExpressionError::Located { node_id, path, .. } => at_scene_param(scene, *node_id, path),
        ExpressionError::Parse { .. } | ExpressionError::Evaluate { .. } => (String::new(), None),
    }
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
    fn a_wrong_kind_points_at_its_connection() {
        let document = serde_json::json!({ "connections": [
            { "from_node": 1, "to_node": 3, "to_port": "source" },
            { "from_node": 2, "to_node": 3, "to_port": "source" },
        ] });
        let error = LumenError::from(GraphValidationError::WrongInputKind {
            node_id: NodeId::new(3),
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
