//! Evaluating one param of a node at a frame, without rendering.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ParseError, load, read_json};
use crate::{
    composition::Composition,
    diagnostic::Diagnostic,
    error::{At, DocumentError, RenderError},
    expr::{ExpressionContext, InstanceScope},
    node::{NodeId, NodeKind, PropertyEval},
};

/// Which param of which node to evaluate, and when.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct EvaluateRequest {
    /// A node of the root composition.
    pub node: NodeId,
    /// The param's name in the node's `params`. A group of params the node flattens, such as
    /// `position`, is addressed by the names it flattens to.
    pub param: String,
    /// The composition frame, which may be fractional.
    pub frame: f64,
    /// The copy an `instance_3d`'s `position`, `rotation` and `scale` are evaluated for, which
    /// they read as `instance`. Other nodes and params ignore it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub instance: Option<u32>,
}

/// The param `request` names, evaluated as a render would at its frame, in the JSON that sets the
/// param. Without media, an expression that needs it fails, and text measures in the bundled font.
/// A composition with errors fails with the diagnostics [`check`](super::check) reports, and
/// anything wrong with the request fails with a diagnostic of its own.
pub fn evaluate_param(
    composition_json: &str,
    request: &EvaluateRequest,
) -> Result<Value, Vec<Diagnostic>> {
    let root = read_json(composition_json)
        .map_err(|error| vec![Diagnostic::from_error(&error, &Value::Null)])?;
    let composition =
        load(&root).map_err(|errors| ParseError::new(&root, errors).into_diagnostics())?;
    evaluate(&composition, request).map_err(|error| vec![Diagnostic::from_error(&error, &root)])
}

fn evaluate(composition: &Composition, request: &EvaluateRequest) -> crate::Result<Value> {
    let graph = composition.graph();
    let at = At::node(request.node);
    let Some(node) = graph.nodes.get(&request.node) else {
        let template = composition
            .compositions()
            .iter()
            .find(|(_, template)| template.scene.graph.nodes.contains_key(&request.node));
        return Err(match template {
            Some((name, _)) => RenderError::TemplateNode {
                at: at.in_template(Some(name)),
            },
            None => RenderError::MissingNode { at },
        }
        .into());
    };
    let mut ctx = ExpressionContext {
        frame: request.frame,
        fps: composition.timeline().fps,
        width: composition.render_settings().width,
        height: composition.render_settings().height,
        graph: Some(graph),
        ..ExpressionContext::default()
    };
    // `count` is the one param of an `instance_3d` that isn't evaluated for a copy.
    if let (NodeKind::Instance3d(instances), Some(index)) = (node, request.instance)
        && request.param != "count"
    {
        let count = instances.params.count.eval(request.node, "count", &ctx)?;
        ctx.instance = Some(InstanceScope {
            index,
            count: count.0,
        });
    }
    node.evaluate_property(&request.param, &ctx)
        .unwrap_or_else(|| {
            Err(DocumentError::UnknownParam {
                at: At::param(request.node, &request.param),
            }
            .into())
        })
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn composition(nodes: Value, connections: Value) -> String {
        json!({
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 64, "height": 32 },
            "nodes": nodes,
            "connections": connections,
        })
        .to_string()
    }

    fn connect(from_node: u64, to_node: u64, to_port: &str) -> Value {
        json!({ "from_node": from_node, "to_node": to_node, "to_port": to_port })
    }

    fn request(node: u64, param: &str, frame: f64) -> EvaluateRequest {
        EvaluateRequest {
            node: NodeId::new(node),
            param: param.to_string(),
            frame,
            instance: None,
        }
    }

    fn opacity(value: Value) -> String {
        composition(
            json!([
                { "id": 1, "type": "background" },
                { "id": 2, "type": "opacity", "params": { "opacity": value } },
                { "id": 3, "type": "media_output" },
            ]),
            json!([connect(1, 2, "source"), connect(2, 3, "source")]),
        )
    }

    #[test]
    fn a_literal_param_is_returned_as_written() {
        let value = evaluate_param(&opacity(json!(0.25)), &request(2, "opacity", 40.0)).unwrap();
        assert_eq!(value.as_f64(), Some(0.25));
    }

    #[test]
    fn an_expression_is_evaluated_at_the_requested_frame() {
        let json = opacity(json!("=frame / 60 + time"));
        let at = |frame| evaluate_param(&json, &request(2, "opacity", frame)).unwrap();
        assert_eq!(at(0.0).as_f64(), Some(0.0));
        assert_eq!(at(30.0).as_f64(), Some(1.5));
    }

    #[cfg(feature = "3d")]
    #[test]
    fn an_instance_3d_param_is_evaluated_for_the_requested_copy() {
        let json = composition(
            json!([
                { "id": 1, "type": "shape_3d" },
                {
                    "id": 2,
                    "type": "instance_3d",
                    "params": { "count": 4, "position": "=vec3(instance * 2, 0, instance_count)" },
                },
                { "id": 3, "type": "render_3d" },
                { "id": 4, "type": "media_output" },
            ]),
            json!([
                connect(1, 2, "scene"),
                connect(2, 3, "scene"),
                connect(3, 4, "source")
            ]),
        );
        let copy = |index| EvaluateRequest {
            instance: Some(index),
            ..request(2, "position", 0.0)
        };
        assert_eq!(
            evaluate_param(&json, &copy(3)).unwrap(),
            json!([6.0, 0.0, 4.0])
        );
        let diagnostics = evaluate_param(&json, &request(2, "position", 0.0)).unwrap_err();
        assert_eq!(diagnostics[0].code, "expression.evaluate");
        assert_eq!(diagnostics[0].path, "/nodes/1/params/position");
    }

    #[test]
    fn a_request_the_composition_cannot_answer_gets_a_diagnostic() {
        let json = opacity(json!(1));
        let codes = |request| {
            evaluate_param(&json, &request)
                .unwrap_err()
                .into_iter()
                .map(|diagnostic| (diagnostic.code, diagnostic.path))
                .collect::<Vec<_>>()
        };
        assert_eq!(
            codes(request(2, "blur", 0.0)),
            [("node.unknown_param", "/nodes/1/params/blur".to_string())]
        );
        assert_eq!(
            codes(request(9, "opacity", 0.0)),
            [("render.missing_node", "/nodes".to_string())]
        );
        assert_eq!(
            evaluate_param("{", &request(2, "opacity", 0.0)).unwrap_err()[0].code,
            "document.syntax"
        );
    }

    #[test]
    fn a_template_node_is_not_evaluated() {
        let json = json!({
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 64, "height": 32 },
            "nodes": [{ "id": 1, "type": "background" }, { "id": 2, "type": "media_output" }],
            "connections": [connect(1, 2, "source")],
            "compositions": { "card": {
                "nodes": [{ "id": 7, "type": "background" }, { "id": 8, "type": "media_output" }],
                "connections": [connect(7, 8, "source")],
            } },
        })
        .to_string();
        let diagnostics = evaluate_param(&json, &request(7, "width", 0.0)).unwrap_err();
        assert_eq!(diagnostics[0].code, "render.template_node");
        assert_eq!(diagnostics[0].path, "/compositions/card/nodes/0");
    }

    #[test]
    fn a_composition_with_errors_gets_the_diagnostics_check_reports() {
        let json = opacity(json!("=nope"));
        let expected = crate::json::check(&json, None, None);
        assert!(!expected.is_empty());
        assert_eq!(
            evaluate_param(&json, &request(2, "opacity", 0.0)).unwrap_err(),
            expected
        );
    }
}
