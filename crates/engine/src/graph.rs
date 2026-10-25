//! Directed graph model, validation, and topological evaluation ordering.

use std::collections::{HashMap, VecDeque};

use crate::{
    error::{GraphValidationError, LumenError},
    node::{Node, NodeId, NodeKind, PortRef},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub from_node: NodeId,
    pub from_port: String,
    pub to_node: NodeId,
    pub to_port: String,
}

#[derive(Default, Debug)]
pub struct Graph {
    pub nodes: HashMap<NodeId, NodeKind>,
    pub connections: Vec<Connection>,
    outgoing_connection_counts: HashMap<NodeId, usize>,
}

unsafe impl Sync for Graph {}
unsafe impl Send for Graph {}

impl Graph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            connections: Vec::new(),
            outgoing_connection_counts: HashMap::new(),
        }
    }

    pub fn connect(&mut self, connection: Connection) -> crate::Result<()> {
        if !self.nodes.contains_key(&connection.from_node) {
            return Err(GraphValidationError::MissingSourceNode {
                node_id: connection.from_node,
            }
            .into());
        }

        if !self.nodes.contains_key(&connection.to_node) {
            return Err(GraphValidationError::MissingTargetNode {
                node_id: connection.to_node,
            }
            .into());
        }

        *self
            .outgoing_connection_counts
            .entry(connection.from_node)
            .or_default() += 1;

        let Connection {
            from_node,
            from_port,
            to_node,
            to_port,
        } = connection;

        #[cfg(feature = "json")]
        wire_input_port(
            self,
            to_node,
            &to_port,
            PortRef::new(from_node, from_port.clone()),
        )?;

        self.connections.push(Connection {
            from_node,
            from_port,
            to_node,
            to_port,
        });
        Ok(())
    }

    pub fn outgoing_connection_count(&self, node_id: NodeId) -> usize {
        self.outgoing_connection_counts
            .get(&node_id)
            .copied()
            .unwrap_or_default()
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

        for connection in &self.connections {
            let Some(from_node) = self.nodes.get(&connection.from_node) else {
                errors.push(
                    GraphValidationError::MissingSourceNode {
                        node_id: connection.from_node,
                    }
                    .into(),
                );
                continue;
            };
            let Some(to_node) = self.nodes.get(&connection.to_node) else {
                errors.push(
                    GraphValidationError::MissingTargetNode {
                        node_id: connection.to_node,
                    }
                    .into(),
                );
                continue;
            };

            let Some(output_def) = from_node
                .output_port_defs()
                .iter()
                .find(|def| def.name == connection.from_port)
            else {
                errors.push(
                    GraphValidationError::MissingSourcePort {
                        node_id: connection.from_node,
                        port: connection.from_port.clone(),
                    }
                    .into(),
                );
                continue;
            };
            let Some(input_def) = to_node
                .input_port_defs()
                .iter()
                .find(|def| def.name == connection.to_port)
            else {
                errors.push(
                    GraphValidationError::MissingTargetPort {
                        node_id: connection.to_node,
                        port: connection.to_port.clone(),
                    }
                    .into(),
                );
                continue;
            };

            if output_def.kind != input_def.kind {
                errors.push(
                    GraphValidationError::PortKindMismatch {
                        from_node: connection.from_node,
                        from_port: output_def.name.into(),
                        from_kind: output_def.kind,
                        to_node: connection.to_node,
                        to_port: input_def.name.into(),
                        expected_kind: input_def.kind,
                    }
                    .into(),
                );
            }
        }

        for node in self.nodes.values() {
            let mut direct_inputs = node.input_ports().into_iter();
            for input in node.input_port_defs() {
                if input.optional {
                    if !input.variadic {
                        direct_inputs.next();
                    }
                    continue;
                }

                let connected_by_edge = self
                    .connections
                    .iter()
                    .any(|edge| edge.to_node == node.id() && edge.to_port == input.name);
                let connected_directly = if input.variadic {
                    direct_inputs.any(|source| !source.is_empty())
                } else {
                    direct_inputs
                        .next()
                        .is_some_and(|source| !source.is_empty())
                };
                if !connected_by_edge && !connected_directly {
                    errors.push(
                        GraphValidationError::MissingRequiredInput {
                            node_id: node.id(),
                            port: input.name.to_string(),
                        }
                        .into(),
                    );
                }
            }
        }

        if let Err(cycle_error) = self.validate_no_cycle() {
            errors.push(cycle_error.into());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    fn validate_no_cycle(&self) -> Result<(), GraphValidationError> {
        let mut indegree: HashMap<NodeId, usize> =
            self.nodes.keys().copied().map(|id| (id, 0)).collect();

        for edge in &self.connections {
            if let Some(entry) = indegree.get_mut(&edge.to_node) {
                *entry += 1;
            }
        }

        let mut queue: VecDeque<NodeId> = indegree
            .iter()
            .filter_map(|(node_id, degree)| (*degree == 0).then_some(*node_id))
            .collect();

        let mut visited = 0_usize;
        while let Some(node_id) = queue.pop_front() {
            visited += 1;
            for edge in self
                .connections
                .iter()
                .filter(|edge| edge.from_node == node_id)
            {
                if let Some(entry) = indegree.get_mut(&edge.to_node) {
                    *entry -= 1;
                    if *entry == 0 {
                        queue.push_back(edge.to_node);
                    }
                }
            }
        }

        if visited != self.nodes.len() {
            let cycle_nodes = indegree
                .into_iter()
                .filter_map(|(node_id, degree)| (degree > 0).then_some(node_id))
                .collect();
            return Err(GraphValidationError::Cycle { path: cycle_nodes });
        }

        Ok(())
    }
}

#[cfg(feature = "json")]
fn wire_input_port(
    graph: &mut Graph,
    to_node: NodeId,
    to_port: &str,
    source: PortRef,
) -> crate::Result<()> {
    use crate::node::JsonNode;

    let node = graph
        .nodes
        .get_mut(&to_node)
        .ok_or(GraphValidationError::MissingTargetNode { node_id: to_node })?;

    let wire = |error: anyhow::Error| GraphValidationError::MissingTargetPort {
        node_id: to_node,
        port: format!("{to_port} ({error})"),
    };

    match node {
        NodeKind::MediaIn(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Background(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Text(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Path(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Shape(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Boolean(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Merge(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::RasterMultiMerge(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::AlphaPremultiply(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Blur(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::ChannelShuffle(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::ColorGrade(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Curves(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Exposure(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::HueSaturation(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Levels(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Memo(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Opacity(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::TimeRemap(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Transform(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Crop(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Resize(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Shadow(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::WgslShader(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::Switch(node) => node.set_input_json(to_port, source).map_err(wire)?,
        NodeKind::MediaOutput(node) => node.set_input_json(to_port, source).map_err(wire)?,
    }

    Ok(())
}
