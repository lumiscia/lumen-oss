//! Structured problems with a composition: what [`crate::json::check`] reports and what a failed
//! [`crate::json::parse`] carries.
//!
//! A diagnostic's `code` comes from the error or warning variant it was made from, and its `path`
//! from the ids and fields that variant holds, so both stay in step with the errors themselves.

use serde::Serialize;
use serde_json::Value;

use crate::{
    error::{
        DocumentError, ExpressionError, GraphValidationError, LumenError, MediaError,
        PropertyError, RenderError, Warning,
    },
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
            Warning::FontFallback { node_id, .. } => Some(*node_id),
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

/// The pointer of a param path such as `content[1].font_size`, as `/content/1/font_size`.
fn param_pointer(path: &str) -> String {
    let mut pointer = String::new();
    for segment in path.split(['.', '[']).filter(|segment| !segment.is_empty()) {
        pointer.push('/');
        pointer.push_str(&escape(segment.strip_suffix(']').unwrap_or(segment)));
    }
    pointer
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

/// Where in `scene` the problem `error` describes is, and which node it's about.
fn locate(error: &LumenError, scene: &Value) -> (String, Option<NodeId>) {
    match error {
        LumenError::Document(error) => locate_document(error, scene),
        LumenError::GraphValidation(error) => locate_graph(error, scene),
        LumenError::Property(PropertyError::InvalidType {
            node_id,
            property_path,
            ..
        }) => at_node(
            scene,
            *node_id,
            &format!("/params{}", param_pointer(property_path)),
        ),
        LumenError::Expression(error) => locate_expression(error, scene),
        LumenError::Media(_) => (String::new(), None),
        LumenError::Render(error) => match error {
            RenderError::NodeEvaluation { node_id, .. }
            | RenderError::MissingNode { node_id }
            | RenderError::InvalidNodeOutputType { node_id, .. } => at_node(scene, *node_id, ""),
            RenderError::FrameOutOfRange { .. } | RenderError::Gpu { .. } => (String::new(), None),
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

fn locate_expression(error: &ExpressionError, scene: &Value) -> (String, Option<NodeId>) {
    match error {
        ExpressionError::Located { node_id, path, .. } => {
            at_node(scene, *node_id, &format!("/params{}", param_pointer(path)))
        }
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
        assert_eq!(param_pointer(""), "");
        assert_eq!(param_pointer("opacity"), "/opacity");
        assert_eq!(
            param_pointer("content[1].font_size"),
            "/content/1/font_size"
        );
        assert_eq!(param_pointer("uniforms.a/b"), "/uniforms/a~1b");
    }

    #[test]
    fn every_code_is_unique_and_documented() {
        let readme = include_str!("../README.md");
        let mut seen = std::collections::BTreeSet::new();
        for code in codes() {
            assert!(seen.insert(code), "`{code}` is used twice");
            assert!(
                readme.contains(&format!("| `{code}` |")),
                "README.md doesn't document `{code}`"
            );
        }
    }
}
