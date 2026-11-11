//! Directed graph model, validation, and topological evaluation ordering.

use std::collections::{HashMap, HashSet};

use crate::{
    error::{GraphValidationError, LumenError},
    node::{Node, NodeId, NodeKind, PortKind},
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
}

impl Graph {
    pub fn new() -> Self {
        Self::default()
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
            return Err(GraphValidationError::MissingSourceNode { node_id: from_node }.into());
        }
        let target = self
            .nodes
            .get_mut(&to_node)
            .ok_or(GraphValidationError::MissingTargetNode { node_id: to_node })?;
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
            errors.push(GraphValidationError::MissingMediaOutput.into());
        } else if media_output_count > 1 {
            errors.push(
                GraphValidationError::MultipleMediaOutputs {
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
                        key: *key,
                        node_id: node.id(),
                    }
                    .into(),
                );
            }

            check_inputs(
                node,
                |source| self.nodes.get(&source).map(Node::output_kind),
                &mut errors,
            );

            let node_sources = sources.entry(*key).or_default();
            for source in node.input_ports().into_iter().flatten() {
                // A missing source is its own validation error, not part of a cycle.
                if self.nodes.contains_key(&source) {
                    node_sources.push(source);
                } else {
                    errors.push(GraphValidationError::MissingSourceNode { node_id: source }.into());
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

/// Reports each required input of `node` left unwired, and each source wired into an input of
/// another kind. `output_kind` gives the kind a source outputs, or `None` for a missing source,
/// which is reported on its own.
fn check_inputs(
    node: &dyn Node,
    output_kind: impl Fn(NodeId) -> Option<PortKind>,
    errors: &mut Vec<LumenError>,
) {
    let defs = node.input_port_defs();
    let inputs = node.input_ports();
    // A node has at most one variadic input, which holds the entries the single inputs don't.
    let variadic_len = inputs.len() - defs.iter().filter(|input| !input.variadic).count();
    let mut wired = inputs.into_iter();
    for input in defs {
        let sources: Vec<NodeId> = if input.variadic {
            wired.by_ref().take(variadic_len).flatten().collect()
        } else {
            wired.next().flatten().into_iter().collect()
        };
        if sources.is_empty() && !input.optional {
            errors.push(
                GraphValidationError::MissingRequiredInput {
                    node_id: node.id(),
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
                        node_id: node.id(),
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

    /// A scene node with an optional variadic scene input and an optional raster input.
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
    const MISSING: NodeId = NodeId::new(9);

    fn errors(node: &dyn Node) -> Vec<String> {
        let output_kind = |id| match id {
            RASTER => Some(PortKind::Raster),
            SCENE => Some(PortKind::Scene),
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
                "connection.wrong_kind: node 1 outputs a raster, but input `scene` of node 3 takes a scene: show the raster in the scene through a scene node that takes a raster input"
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
                "connection.wrong_kind: node 2 outputs a scene, but input `texture` of node 4 takes a raster: render the scene to a raster first, and wire that in"
            ]
        );
    }

    #[test]
    fn every_source_of_a_variadic_input_is_checked_and_optional_inputs_may_be_unwired() {
        assert_eq!(errors(&TestScene::default()), Vec::<String>::new());
        let scene = TestScene {
            id: NodeId::new(4),
            children: vec![SCENE, RASTER, MISSING, SCENE],
            texture: Some(RASTER),
        };
        assert_eq!(
            errors(&scene),
            [
                "connection.wrong_kind: node 1 outputs a raster, but input `children` of node 4 takes a scene: show the raster in the scene through a scene node that takes a raster input"
            ]
        );
        assert_eq!(
            errors(&TestRender::default()),
            ["graph.missing_required_input: node 0 is missing required input `scene`"]
        );
    }
}
