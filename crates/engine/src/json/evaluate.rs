//! Evaluating one param of a node at a frame, without rendering.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::{ParseError, load, read_json};
use crate::{
    composition::{Composition, SceneId},
    diagnostic::Diagnostic,
    error::{At, DocumentError, RenderError},
    expr::InstanceScope,
    node::{Node, NodeId, NodeKind, PropertyEval},
    scope::{FrameTime, MAX_GRAPH_DEPTH, Routed, Scope, route},
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
    /// The composition frame, which may be fractional. A node inside a `clip`, or behind a
    /// `time_remap`, is evaluated at the frame of its own time that this frame maps to.
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
    let scope = Scope::root(composition);
    let frame = local_frame(&scope, request)?;
    let mut ctx = scope.expr_context(frame, None);
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

/// The frame of its own time that `request.node` is evaluated at when the composition is at
/// `request.frame`: the requested frame carried through the clips and time remaps between the
/// output and the node, as rendering does. A node the output doesn't read keeps the requested
/// frame.
fn local_frame(scope: &Scope<'_>, request: &EvaluateRequest) -> crate::Result<FrameTime> {
    let mut walk = Walk {
        target: request.node,
        frames: Vec::new(),
        seen: HashSet::new(),
    };
    walk.visit(scope, scope.output, request.frame.into(), false, 0)?;
    let at = At::node(request.node);
    match walk.frames.as_slice() {
        [frame] => Ok(*frame),
        [] if reads(scope, scope.output, request.node, &mut HashSet::new()) => {
            Err(RenderError::NodeNotEvaluated {
                at,
                frame: request.frame,
            }
            .into())
        }
        [] => Ok(request.frame.into()),
        frames => Err(RenderError::AmbiguousNodeFrame {
            at,
            frame: request.frame,
            local: frames
                .iter()
                .map(|frame| frame.value().to_string())
                .collect::<Vec<_>>()
                .join(", "),
        }
        .into()),
    }
}

/// The walk of the graph from the output that finds the frames a node is evaluated at.
struct Walk {
    target: NodeId,
    /// Each distinct frame the target was reached at.
    frames: Vec<FrameTime>,
    seen: HashSet<(NodeId, u64, bool)>,
}

impl Walk {
    /// Visits `node_id` of the root at `frame`, then the inputs `route` reads from it. Templates
    /// hold no node of the root, so they aren't entered.
    fn visit(
        &mut self,
        scope: &Scope<'_>,
        node_id: NodeId,
        frame: FrameTime,
        sampling: bool,
        depth: u32,
    ) -> crate::Result<()> {
        if !self
            .seen
            .insert((node_id, frame.value().to_bits(), sampling))
        {
            return Ok(());
        }
        if node_id == self.target {
            if !self.frames.iter().any(|seen| seen.value() == frame.value()) {
                self.frames.push(frame);
            }
            return Ok(());
        }
        if depth == MAX_GRAPH_DEPTH {
            return Err(scope.too_deep(node_id));
        }
        let Some(node) = scope.node(node_id) else {
            return Ok(());
        };
        for Routed {
            input,
            frame,
            scope,
            instance,
        } in route(scope, node, frame, None, sampling)?
        {
            if scope.id == SceneId::Root {
                let sampling =
                    sampling || (instance.is_some() && matches!(node, NodeKind::MotionBlur(_)));
                self.visit(&scope, input.node, frame, sampling, depth + 1)?;
            }
        }
        Ok(())
    }
}

/// Whether `target` is `node_id` or among its inputs, however deep.
fn reads(scope: &Scope<'_>, node_id: NodeId, target: NodeId, seen: &mut HashSet<NodeId>) -> bool {
    node_id == target
        || seen.insert(node_id)
            && scope.node(node_id).is_some_and(|node| {
                node.input_ports()
                    .into_iter()
                    .flatten()
                    .any(|input| reads(scope, input.node, target, seen))
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

    fn clip_chain(starts: &[i64]) -> String {
        let mut nodes = vec![
            json!({ "id": 1, "type": "background" }),
            json!({ "id": 2, "type": "opacity", "params": { "opacity": "=frame / 100" } }),
        ];
        let mut connections = vec![connect(1, 2, "source")];
        let mut clips = Vec::new();
        for (index, start) in starts.iter().enumerate() {
            let id = 3 + index as u64;
            nodes.push(json!({ "id": id, "type": "clip", "params": { "start": start } }));
            connections.push(connect(2, id, "source"));
            clips.push(id);
        }
        let output = 3 + starts.len() as u64;
        match clips.as_slice() {
            [only] => connections.push(connect(*only, output, "source")),
            [first, second] => {
                nodes.push(json!({ "id": output, "type": "merge" }));
                connections.push(connect(*first, output, "base"));
                connections.push(connect(*second, output, "overlay"));
                nodes.push(json!({ "id": output + 1, "type": "media_output" }));
                connections.push(connect(output, output + 1, "source"));
                return composition(json!(nodes), json!(connections));
            }
            _ => unreachable!(),
        }
        nodes.push(json!({ "id": output, "type": "media_output" }));
        composition(json!(nodes), json!(connections))
    }

    #[test]
    fn a_node_upstream_of_a_clip_is_evaluated_at_its_local_frame() {
        let json = clip_chain(&[30]);
        let at = |frame| evaluate_param(&json, &request(2, "opacity", frame));
        assert_eq!(at(60.0).unwrap().as_f64(), Some(0.3));
        assert_eq!(at(40.5).unwrap().as_f64(), Some(0.105));
        let diagnostics = at(10.0).unwrap_err();
        assert_eq!(diagnostics[0].code, "render.node_not_evaluated");
        assert_eq!(diagnostics[0].path, "/nodes/1");
    }

    #[test]
    fn a_node_reached_at_several_local_frames_is_ambiguous() {
        let json = clip_chain(&[30, 40]);
        let at = |frame| evaluate_param(&json, &request(2, "opacity", frame));
        assert_eq!(at(45.0).unwrap_err()[0].code, "render.ambiguous_node_frame");
        // The first clip hasn't started, so only the second reaches the node.
        assert_eq!(at(35.0).unwrap().as_f64(), Some(0.05));
    }

    #[test]
    fn expressions_nested_in_wgsl_shader_uniforms_are_evaluated() {
        let shader =
            "struct P { amount: f32, center: vec2<f32> } @group(0) @binding(1) var<uniform> p: P;";
        let json = |uniforms| {
            composition(
                json!([
                    { "id": 1, "type": "background" },
                    { "id": 2, "type": "wgsl_shader", "params": { "shader": shader, "uniforms": uniforms } },
                    { "id": 3, "type": "media_output" },
                ]),
                json!([connect(1, 2, "source"), connect(2, 3, "source")]),
            )
        };
        let evaluated = evaluate_param(
            &json(json!({ "amount": "=frame / 100", "center": [1, "=frame"] })),
            &request(2, "uniforms", 60.0),
        )
        .unwrap();
        assert_eq!(evaluated, json!({ "amount": 0.6, "center": [1.0, 60.0] }));
        let diagnostics = evaluate_param(
            &json(json!({ "amount": "=frame / 100", "center": "='abc'" })),
            &request(2, "uniforms", 60.0),
        )
        .unwrap_err();
        assert_eq!(diagnostics[0].path, "/nodes/1/params/uniforms/center");
    }

    #[test]
    fn expressions_nested_in_clip_props_are_evaluated() {
        let json = |props| {
            json!({
                "timeline": { "fps": 30, "duration_frames": 90 },
                "render_settings": { "width": 64, "height": 32 },
                "nodes": [
                    { "id": 1, "type": "clip", "params": { "composition": "card", "props": props } },
                    { "id": 2, "type": "media_output" },
                ],
                "connections": [connect(1, 2, "source")],
                "compositions": { "card": {
                    "props": { "index": { "type": "number" } },
                    "nodes": [{ "id": 7, "type": "background" }, { "id": 8, "type": "media_output" }],
                    "connections": [connect(7, 8, "source")],
                } },
            })
            .to_string()
        };
        let evaluated = evaluate_param(
            &json(json!({ "index": "=frame / 10" })),
            &request(1, "props", 30.0),
        );
        assert_eq!(evaluated.unwrap(), json!({ "index": 3.0 }));
        let diagnostics = evaluate_param(
            &json(json!({ "index": "=frame > 1" })),
            &request(1, "props", 30.0),
        )
        .unwrap_err();
        assert!(
            diagnostics[0].path.ends_with("/props/index"),
            "{diagnostics:?}"
        );
    }

    #[test]
    fn text_spans_are_evaluated_with_their_styles() {
        let json = composition(
            json!([
                { "id": 1, "type": "text", "params": { "content": [
                    { "text": "a", "font_size": "=frame", "color": "=rgb(255, 0, 0)" },
                    { "text": "b", "underline": { "thickness": "=frame / 5" } },
                ] } },
                { "id": 2, "type": "media_output" },
            ]),
            json!([connect(1, 2, "source")]),
        );
        let content = evaluate_param(&json, &request(1, "content", 10.0)).unwrap();
        assert_eq!(content[0]["font_size"], json!(10.0));
        assert_eq!(content[0]["color"], json!([255, 0, 0, 255]));
        assert_eq!(content[1]["underline"]["thickness"], json!(2.0));
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
