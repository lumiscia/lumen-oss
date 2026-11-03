//! Directed graph model, validation, and topological evaluation ordering.

use std::collections::{HashMap, HashSet};

use crate::{
    error::{GraphValidationError, LumenError},
    node::{Node, NodeId, NodeKind},
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
        Ok(wire_input_port(target, &to_port, from_node)?)
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
        for (key, node) in &self.nodes {
            if node.id() != *key {
                errors.push(
                    GraphValidationError::NodeIdMismatch {
                        key: *key,
                        node_id: node.id(),
                    }
                    .into(),
                );
            }

            let inputs = node.input_ports();
            let mut wired = inputs.iter();
            for input in node.input_port_defs() {
                let connected = if input.variadic {
                    wired.by_ref().any(Option::is_some)
                } else {
                    wired.next().is_some_and(Option::is_some)
                };
                if !connected && !input.optional {
                    errors.push(
                        GraphValidationError::MissingRequiredInput {
                            node_id: node.id(),
                            port: input.name.to_string(),
                        }
                        .into(),
                    );
                }
            }

            let node_sources = sources.entry(*key).or_default();
            for source in inputs.into_iter().flatten() {
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

fn wire_input_port(
    node: &mut NodeKind,
    to_port: &str,
    source: NodeId,
) -> Result<(), GraphValidationError> {
    use crate::node::JsonNode;

    match node {
        NodeKind::MediaIn(node) => node.set_input_json(to_port, source),
        NodeKind::Background(node) => node.set_input_json(to_port, source),
        NodeKind::Text(node) => node.set_input_json(to_port, source),
        NodeKind::Path(node) => node.set_input_json(to_port, source),
        NodeKind::Shape(node) => node.set_input_json(to_port, source),
        NodeKind::Boolean(node) => node.set_input_json(to_port, source),
        NodeKind::Merge(node) => node.set_input_json(to_port, source),
        NodeKind::RasterMultiMerge(node) => node.set_input_json(to_port, source),
        NodeKind::AlphaPremultiply(node) => node.set_input_json(to_port, source),
        NodeKind::Blur(node) => node.set_input_json(to_port, source),
        NodeKind::ChannelShuffle(node) => node.set_input_json(to_port, source),
        NodeKind::ColorGrade(node) => node.set_input_json(to_port, source),
        NodeKind::Curves(node) => node.set_input_json(to_port, source),
        NodeKind::Exposure(node) => node.set_input_json(to_port, source),
        NodeKind::HueSaturation(node) => node.set_input_json(to_port, source),
        NodeKind::Levels(node) => node.set_input_json(to_port, source),
        NodeKind::Memo(node) => node.set_input_json(to_port, source),
        NodeKind::Opacity(node) => node.set_input_json(to_port, source),
        NodeKind::TimeRemap(node) => node.set_input_json(to_port, source),
        NodeKind::Transform(node) => node.set_input_json(to_port, source),
        NodeKind::Crop(node) => node.set_input_json(to_port, source),
        NodeKind::Resize(node) => node.set_input_json(to_port, source),
        NodeKind::Shadow(node) => node.set_input_json(to_port, source),
        NodeKind::WgslShader(node) => node.set_input_json(to_port, source),
        NodeKind::Switch(node) => node.set_input_json(to_port, source),
        NodeKind::Clip(node) => node.set_input_json(to_port, source),
        NodeKind::MediaOutput(node) => node.set_input_json(to_port, source),
    }
}
