//! Directed graph model, validation, and topological evaluation ordering.

use std::collections::{HashMap, VecDeque};

use crate::{
    error::{GraphValidationError, LumenError},
    node::{Node, NodeId, NodeKind},
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
    // TODO: index nodes by source for faster lookup when caching node results
}

unsafe impl Sync for Graph {}
unsafe impl Send for Graph {}

impl Graph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            connections: Vec::new(),
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

        self.connections.push(connection);
        Ok(())
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

            let output_def = match from_node
                .output_port_defs()
                .iter()
                .find(|def| def.name == &connection.to_port)
            {
                Some(output_def) => output_def,
                None => {
                    errors.push(
                        GraphValidationError::MissingSourceNode {
                            node_id: connection.to_node,
                        }
                        .into(),
                    );
                    continue;
                }
            };
            let input_def = match to_node
                .input_port_defs()
                .iter()
                .find(|def| def.name == &connection.to_port)
            {
                Some(input_def) => input_def,
                None => {
                    errors.push(
                        GraphValidationError::MissingTargetNode {
                            node_id: connection.to_node,
                        }
                        .into(),
                    );
                    continue;
                }
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
            for input in node.input_port_defs() {
                if input.optional {
                    continue;
                }

                let connected = self
                    .connections
                    .iter()
                    .any(|edge| edge.to_node == node.id() && &edge.to_port == input.name);

                if !connected {
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
