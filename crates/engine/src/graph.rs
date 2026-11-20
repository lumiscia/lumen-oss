//! Directed graph model, validation, and topological evaluation ordering.

use std::collections::{HashMap, HashSet};

use crate::{
    error::{At, DocumentError, GraphValidationError, LumenError},
    gpu::frame_graph::RecordNode,
    node::{InputPortDef, Node, NodeId, NodeKind, PortKind},
};

/// An edge as authors write it. `connect` stores it in the target node's input field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub from_node: NodeId,
    pub to_node: NodeId,
    pub to_port: String,
}

/// The nodes of a composition. A node's input fields are the graph's only edges.
#[derive(Default, Debug, lumen_macros::StructuralHash)]
pub struct Graph {
    pub nodes: HashMap<NodeId, NodeKind>,
    /// Each node's index in the document's `nodes`, for a graph read from JSON.
    pub(crate) positions: HashMap<NodeId, usize>,
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
    }

    /// The index of node `id` in the document's `nodes`, for a graph read from JSON.
    pub(crate) fn position(&self, id: NodeId) -> Option<usize> {
        self.positions.get(&id).copied()
    }

    /// Wires the output of `from_node` into the named input of `to_node`. A variadic input
    /// appends; any other input takes one connection.
    pub fn connect(&mut self, connection: Connection) -> crate::Result<()> {
        let Connection {
            from_node,
            to_node,
            to_port,
        } = connection;
        if !self.nodes.contains_key(&from_node) {
            return Err(GraphValidationError::MissingSourceNode {
                at: At::node(from_node),
            }
            .into());
        }
        let target =
            self.nodes
                .get_mut(&to_node)
                .ok_or(GraphValidationError::MissingTargetNode {
                    at: At::node(to_node),
                })?;
        Ok(target.set_input(&to_port, from_node)?)
    }

    /// The `media_output` node of a graph that passed [`Graph::validate`].
    pub(crate) fn output(&self) -> NodeId {
        self.nodes
            .iter()
            .find_map(|(id, node)| matches!(node, NodeKind::MediaOutput(_)).then_some(*id))
            .expect("validation requires one media_output node")
    }

    pub fn validate(&self) -> Result<(), Vec<LumenError>> {
        let mut errors = Vec::new();

        let media_output_count = self
            .nodes
            .values()
            .filter(|node| matches!(node, NodeKind::MediaOutput(_)))
            .count();
        if media_output_count == 0 {
            errors.push(GraphValidationError::MissingMediaOutput { at: At::default() }.into());
        } else if media_output_count > 1 {
            errors.push(
                GraphValidationError::MultipleMediaOutputs {
                    at: At::default(),
                    count: media_output_count,
                }
                .into(),
            );
        }

        // The sources each node reads, which are the edges of the graph.
        let mut sources: HashMap<NodeId, Vec<NodeId>> = HashMap::with_capacity(self.nodes.len());
        let mut nodes = self.nodes.iter().collect::<Vec<_>>();
        nodes.sort_unstable_by_key(|(key, _)| **key);
        for (key, node) in nodes {
            if node.id() != *key {
                errors.push(
                    GraphValidationError::NodeIdMismatch {
                        at: At::node(*key),
                        node_id: node.id(),
                    }
                    .into(),
                );
            }

            check_supported(node, cfg!(feature = "3d"), &mut errors);
            check_inputs(
                node,
                |source| self.nodes.get(&source).map(Node::output_kind),
                &mut errors,
            );
            errors.extend(
                node.unwired_input_param_errors()
                    .into_iter()
                    .map(|(param, details)| {
                        DocumentError::InvalidParam {
                            at: At::param(node.id(), param),
                            details,
                        }
                        .into()
                    }),
            );

            let node_sources = sources.entry(*key).or_default();
            for source in node.input_ports().into_iter().flatten() {
                // A missing source is its own validation error, not part of a cycle.
                if self.nodes.contains_key(&source) {
                    node_sources.push(source);
                } else {
                    errors.push(
                        GraphValidationError::MissingSourceNode {
                            at: At::node(source),
                        }
                        .into(),
                    );
                }
            }
        }

        if let Err(cycle_error) = find_cycle(&sources) {
            errors.push(cycle_error.into());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// Each input `node` declares, with the sources wired into it: one for a single input if
/// connected, any number for a variadic one.
fn wired_inputs(node: &dyn Node) -> Vec<(&'static InputPortDef, Vec<NodeId>)> {
    let defs = node.input_port_defs();
    let inputs = node.input_ports();
    // A node has at most one variadic input, which holds the entries the single inputs don't.
    let variadic_len = inputs.len() - defs.iter().filter(|input| !input.variadic).count();
    let mut wired = inputs.into_iter();
    defs.iter()
        .map(|input| {
            let sources = if input.variadic {
                wired.by_ref().take(variadic_len).flatten().collect()
            } else {
                wired.next().flatten().into_iter().collect()
            };
            (input, sources)
        })
        .collect()
}

/// Reports `node` when it is a 3D node, one that outputs a scene or a material or takes one
/// through a required or wired input, and the engine was built without 3D. An optional scene or
/// material input left unconnected doesn't make a node 3D.
fn check_supported(node: &NodeKind, supports_3d: bool, errors: &mut Vec<LumenError>) {
    let is_3d = |kind: PortKind| matches!(kind, PortKind::Scene | PortKind::Material);
    let scene = is_3d(node.output_kind())
        || wired_inputs(node)
            .iter()
            .any(|(input, sources)| is_3d(input.kind) && (!input.optional || !sources.is_empty()));
    if scene && !supports_3d {
        errors.push(
            GraphValidationError::No3dSupport {
                at: At::node(node.id()),
                kind: node.kind_name(),
            }
            .into(),
        );
    }
}

/// Reports each required input of `node` left unwired, and each source wired into an input of
/// another kind. `output_kind` gives the kind a source outputs, or `None` for a missing source,
/// which is reported on its own.
fn check_inputs(
    node: &dyn Node,
    output_kind: impl Fn(NodeId) -> Option<PortKind>,
    errors: &mut Vec<LumenError>,
) {
    for (input, sources) in wired_inputs(node) {
        if sources.is_empty() && !input.optional {
            errors.push(
                GraphValidationError::MissingRequiredInput {
                    at: At::node(node.id()),
                    port: input.name.to_string(),
                }
                .into(),
            );
        }
        for source in sources {
            if let Some(actual) = output_kind(source)
                && actual != input.kind
            {
                errors.push(
                    GraphValidationError::WrongInputKind {
                        at: At::node(node.id()),
                        port: input.name.to_string(),
                        source_node: source,
                        expected: input.kind,
                        actual,
                    }
                    .into(),
                );
            }
        }
    }
}

/// Kahn's algorithm over `sources`, each node's inputs, which reports a cycle if the nodes
/// can't all be ordered after their inputs.
fn find_cycle(sources: &HashMap<NodeId, Vec<NodeId>>) -> Result<(), GraphValidationError> {
    let mut waiting: HashMap<NodeId, usize> = sources
        .iter()
        .map(|(&node, node_sources)| (node, node_sources.len()))
        .collect();
    let mut consumers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    for (&node, node_sources) in sources {
        for &source in node_sources {
            consumers.entry(source).or_default().push(node);
        }
    }

    let mut ready: Vec<NodeId> = waiting
        .iter()
        .filter_map(|(&node, &count)| (count == 0).then_some(node))
        .collect();
    let mut ordered = 0_usize;
    while let Some(node) = ready.pop() {
        ordered += 1;
        for consumer in consumers.get(&node).into_iter().flatten() {
            let count = waiting.get_mut(consumer).expect("every consumer is a node");
            *count -= 1;
            if *count == 0 {
                ready.push(*consumer);
            }
        }
    }

    if ordered == sources.len() {
        return Ok(());
    }
    let unordered: HashSet<NodeId> = waiting
        .into_iter()
        .filter_map(|(node, count)| (count > 0).then_some(node))
        .collect();
    Err(GraphValidationError::Cycle {
        at: At::default(),
        path: cycle_path(&unordered, sources),
    })
}

/// A cycle among `unordered`, the nodes a topological sort left over, each of which has an input
/// from another of them. It is found by walking inputs from the lowest id until a node repeats,
/// and runs along the edges, ending with the node it starts at.
fn cycle_path(unordered: &HashSet<NodeId>, sources: &HashMap<NodeId, Vec<NodeId>>) -> Vec<NodeId> {
    let lowest_unordered_source = |node: NodeId| {
        sources[&node]
            .iter()
            .copied()
            .filter(|source| unordered.contains(source))
            .min()
    };
    let Some(mut node) = unordered.iter().min().copied() else {
        return Vec::new();
    };
    let mut walked = vec![node];
    loop {
        node = lowest_unordered_source(node).expect("a leftover node has a leftover input");
        if let Some(start) = walked.iter().position(|&seen| seen == node) {
            let mut path = walked.split_off(start);
            path.push(node);
            path.reverse();
            return path;
        }
        walked.push(node);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A scene node with an optional variadic scene input, an optional raster input and an
    /// optional material input.
    #[derive(Debug, Default, lumen_macros::Node)]
    #[node(
        kind = "test_scene",
        name = "Test Scene",
        category = "source",
        output = "scene"
    )]
    struct TestScene {
        id: NodeId,
        #[input(scene, optional, variadic)]
        children: Vec<NodeId>,
        #[input(optional)]
        texture: Option<NodeId>,
        #[input(material, optional)]
        material: Option<NodeId>,
    }

    /// A raster node with a required scene input.
    #[derive(Debug, Default, lumen_macros::Node)]
    #[node(kind = "test_render", name = "Test Render", category = "source")]
    struct TestRender {
        id: NodeId,
        #[input(scene)]
        scene: Option<NodeId>,
    }

    const RASTER: NodeId = NodeId::new(1);
    const SCENE: NodeId = NodeId::new(2);
    const DRAWING: NodeId = NodeId::new(5);
    const MISSING: NodeId = NodeId::new(9);

    fn errors(node: &dyn Node) -> Vec<String> {
        let output_kind = |id| match id {
            RASTER => Some(PortKind::Raster),
            SCENE => Some(PortKind::Scene),
            DRAWING => Some(PortKind::Drawing),
            _ => None,
        };
        let mut errors = Vec::new();
        check_inputs(node, output_kind, &mut errors);
        errors
            .iter()
            .map(|error| format!("{}: {error}", error.code()))
            .collect()
    }

    #[test]
    fn a_source_of_the_wrong_kind_is_named_with_both_kinds() {
        let render = TestRender {
            id: NodeId::new(3),
            scene: Some(RASTER),
        };
        assert_eq!(
            errors(&render),
            [
                "connection.wrong_kind: node 3: input `scene` takes a scene, but node 1 outputs a raster: show the raster in the scene through an image_plane_3d"
            ]
        );
        let scene = TestScene {
            id: NodeId::new(4),
            texture: Some(SCENE),
            ..TestScene::default()
        };
        assert_eq!(
            errors(&scene),
            [
                "connection.wrong_kind: node 4: input `texture` takes a raster, but node 2 outputs a scene: draw the scene into a raster through a render_3d, and wire that in"
            ]
        );
        let scene = TestScene {
            id: NodeId::new(4),
            texture: Some(DRAWING),
            ..TestScene::default()
        };
        assert_eq!(
            errors(&scene),
            [
                "connection.wrong_kind: node 4: input `texture` takes a raster, but node 5 outputs a drawing: draw the drawing into a raster through a render_2d, and wire that in"
            ]
        );
    }

    #[test]
    fn a_raster_or_scene_wired_into_a_material_input_points_at_material_3d() {
        let hint = "connect a material_3d to the object's `material` input";
        for source in [RASTER, SCENE] {
            let object = TestScene {
                id: NodeId::new(5),
                material: Some(source),
                ..TestScene::default()
            };
            let actual = if source == RASTER {
                "a raster"
            } else {
                "a scene"
            };
            assert_eq!(
                errors(&object),
                [format!(
                    "connection.wrong_kind: node 5: input `material` takes a material, but node {source} outputs {actual}: {hint}"
                )]
            );
        }
    }

    #[cfg(not(feature = "3d"))]
    #[test]
    fn a_wgsl_shader_needs_3d_only_with_a_camera_wired() {
        use crate::node::processing::wgsl_shader::WgslShader;

        let shader = |camera| {
            let node = NodeKind::WgslShader(WgslShader {
                camera,
                ..WgslShader::default()
            });
            let mut errors = Vec::new();
            check_supported(&node, false, &mut errors);
            errors
                .iter()
                .map(|error| error.code().to_string())
                .collect::<Vec<_>>()
        };
        assert_eq!(shader(None), Vec::<String>::new());
        assert_eq!(shader(Some(NodeId::new(2))), ["node.unsupported"]);
    }

    #[test]
    fn nodes_that_output_or_take_a_scene_need_3d() {
        use crate::node::{
            processing::opacity::Opacity,
            scene::{Merge3d, Render3d},
        };

        let unsupported = |node: NodeKind, supports_3d| {
            let mut errors = Vec::new();
            check_supported(&node, supports_3d, &mut errors);
            errors
                .iter()
                .map(|error| format!("{}: {error}", error.code()))
                .collect::<Vec<_>>()
        };
        let merge = || NodeKind::Merge3d(Merge3d::default());
        assert_eq!(
            unsupported(merge(), false),
            [
                "node.unsupported: node 0 (`merge_3d`): this build of the engine has no 3D support, as it was built without the `3d` feature"
            ]
        );
        assert_eq!(
            unsupported(NodeKind::Render3d(Render3d::default()), false).len(),
            1
        );
        assert_eq!(unsupported(merge(), true), Vec::<String>::new());
        assert_eq!(
            unsupported(NodeKind::Opacity(Opacity::default()), false),
            Vec::<String>::new()
        );
        assert_eq!(
            unsupported(
                NodeKind::Render2d(crate::node::vector::drawing::Render2d::default()),
                false
            ),
            Vec::<String>::new()
        );
    }

    #[test]
    fn every_source_of_a_variadic_input_is_checked_and_optional_inputs_may_be_unwired() {
        assert_eq!(errors(&TestScene::default()), Vec::<String>::new());
        let scene = TestScene {
            id: NodeId::new(4),
            children: vec![SCENE, RASTER, MISSING, SCENE],
            texture: Some(RASTER),
            material: None,
        };
        assert_eq!(
            errors(&scene),
            [
                "connection.wrong_kind: node 4: input `children` takes a scene, but node 1 outputs a raster: show the raster in the scene through an image_plane_3d"
            ]
        );
        assert_eq!(
            errors(&TestRender::default()),
            ["graph.missing_required_input: node 0 is missing required input `scene`"]
        );
    }
}
