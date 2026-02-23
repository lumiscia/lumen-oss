//! Directed graph model, validation, and topological evaluation ordering.

use std::collections::{HashMap, HashSet, VecDeque};

use crate::{
    error::{GraphValidationError, LumenError, Warning},
    node::{InputPortDef, Node, NodeId, NodeKind, OutputPortDef, PortKind},
};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum InputPort {
    Named(String),
    Indexed(u16),
}

impl InputPort {
    pub fn named(name: impl Into<String>) -> Self {
        Self::Named(name.into())
    }

    fn matches(&self, def: &InputPortDef, index: usize) -> bool {
        match self {
            Self::Named(name) => def.name == name,
            Self::Indexed(port_index) => usize::from(*port_index) == index,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum OutputPort {
    Named(String),
    Indexed(u16),
}

impl OutputPort {
    pub fn named(name: impl Into<String>) -> Self {
        Self::Named(name.into())
    }

    pub fn default() -> Self {
        Self::Named("output".to_string())
    }

    fn matches(&self, def: &OutputPortDef, index: usize) -> bool {
        match self {
            Self::Named(name) => def.name == name,
            Self::Indexed(port_index) => usize::from(*port_index) == index,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Connection {
    pub from_node: NodeId,
    pub from_port: OutputPort,
    pub to_node: NodeId,
    pub to_port: InputPort,
}

#[derive(Debug, Clone, Default)]
pub struct Graph {
    pub nodes: HashMap<NodeId, Node>,
    pub connections: Vec<Connection>,
    next_node_id: u64,
}

impl Graph {
    pub fn new() -> Self {
        Self {
            nodes: HashMap::new(),
            connections: Vec::new(),
            next_node_id: 1,
        }
    }

    pub fn add_node(&mut self, mut node: Node) -> NodeId {
        if node.id.0 == 0 || self.nodes.contains_key(&node.id) {
            node.id = NodeId(self.next_node_id);
            self.next_node_id += 1;
        } else {
            self.next_node_id = self.next_node_id.max(node.id.0 + 1);
        }

        let id = node.id;
        self.nodes.insert(id, node);
        id
    }

    pub fn connect(&mut self, connection: Connection) -> Result<(), LumenError> {
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

    pub fn remove_node(&mut self, id: NodeId) -> Result<Node, LumenError> {
        let node = self
            .nodes
            .remove(&id)
            .ok_or(GraphValidationError::MissingTargetNode { node_id: id })?;
        self.connections
            .retain(|edge| edge.from_node != id && edge.to_node != id);
        Ok(node)
    }

    pub fn remove_connection(
        &mut self,
        from: (NodeId, OutputPort),
        to: (NodeId, InputPort),
    ) -> Result<(), LumenError> {
        let initial_len = self.connections.len();
        self.connections.retain(|edge| {
            !(edge.from_node == from.0
                && edge.from_port == from.1
                && edge.to_node == to.0
                && edge.to_port == to.1)
        });

        if initial_len == self.connections.len() {
            return Err(GraphValidationError::InvalidEvaluationTarget { node_id: from.0 }.into());
        }

        Ok(())
    }

    pub fn validate(&self) -> Result<Vec<Warning>, Vec<LumenError>> {
        let mut errors = Vec::new();

        let media_output_count = self
            .nodes
            .values()
            .filter(|node| matches!(node.kind, NodeKind::MediaOutput(_)))
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

            let output_kind = resolve_output_kind(from_node, &connection.from_port);
            let input_kind = resolve_input_kind(to_node, &connection.to_port);
            match (output_kind, input_kind) {
                (Some(from_kind), Some(expected_kind)) if from_kind != expected_kind => {
                    errors.push(
                        GraphValidationError::PortKindMismatch {
                            from_node: connection.from_node,
                            from_port: port_name_out(&connection.from_port),
                            from_kind,
                            to_node: connection.to_node,
                            to_port: port_name_in(&connection.to_port),
                            expected_kind,
                        }
                        .into(),
                    );
                }
                (None, _) => errors.push(
                    GraphValidationError::PortKindMismatch {
                        from_node: connection.from_node,
                        from_port: port_name_out(&connection.from_port),
                        from_kind: PortKind::RasterFrame,
                        to_node: connection.to_node,
                        to_port: port_name_in(&connection.to_port),
                        expected_kind: PortKind::RasterFrame,
                    }
                    .into(),
                ),
                (_, None) => errors.push(
                    GraphValidationError::PortKindMismatch {
                        from_node: connection.from_node,
                        from_port: port_name_out(&connection.from_port),
                        from_kind: PortKind::RasterFrame,
                        to_node: connection.to_node,
                        to_port: port_name_in(&connection.to_port),
                        expected_kind: PortKind::RasterFrame,
                    }
                    .into(),
                ),
                _ => {}
            }
        }

        for node in self.nodes.values() {
            for input in node.kind.input_port_defs() {
                if input.optional {
                    continue;
                }

                let connected = self.connections.iter().any(|edge| {
                    edge.to_node == node.id
                        && matches!(&edge.to_port, InputPort::Named(name) if name == input.name)
                });

                if !connected {
                    errors.push(
                        GraphValidationError::MissingRequiredInput {
                            node_id: node.id,
                            node_kind: node.kind.kind_name(),
                            port: input.name.to_string(),
                        }
                        .into(),
                    );
                }
            }
        }


        for node in self.nodes.values() {
            if let NodeKind::Switch(switch_node) = &node.kind {
                let mut ranges: Vec<_> = switch_node.map.values().cloned().collect();
                ranges.sort_by_key(|range| (range.start, range.end));

                for pair in ranges.windows(2) {
                    if let [first, second] = pair {
                        if first.end > second.start {
                            errors.push(
                                GraphValidationError::SwitchRangeOverlap {
                                    node_id: node.id,
                                    first: first.clone(),
                                    second: second.clone(),
                                }
                                .into(),
                            );
                        }
                    }
                }
            }
        }
        if let Err(cycle_error) = self.validate_no_cycle() {
            errors.push(cycle_error.into());
        }

        if errors.is_empty() {
            Ok(Vec::new())
        } else {
            Err(errors)
        }
    }

    pub fn evaluation_order(&self, target: NodeId) -> Result<Vec<NodeId>, LumenError> {
        if !self.nodes.contains_key(&target) {
            return Err(GraphValidationError::InvalidEvaluationTarget { node_id: target }.into());
        }

        let mut reachable = HashSet::new();
        let mut stack = vec![target];
        while let Some(node_id) = stack.pop() {
            if !reachable.insert(node_id) {
                continue;
            }
            for connection in self
                .connections
                .iter()
                .filter(|edge| edge.to_node == node_id)
            {
                stack.push(connection.from_node);
            }
        }

        let mut indegree: HashMap<NodeId, usize> = reachable
            .iter()
            .copied()
            .map(|node_id| (node_id, 0))
            .collect();

        for edge in self
            .connections
            .iter()
            .filter(|edge| reachable.contains(&edge.from_node) && reachable.contains(&edge.to_node))
        {
            if let Some(in_degree) = indegree.get_mut(&edge.to_node) {
                *in_degree += 1;
            }
        }

        let mut queue: VecDeque<NodeId> = indegree
            .iter()
            .filter_map(|(node_id, degree)| (*degree == 0).then_some(*node_id))
            .collect();

        let mut ordered = Vec::with_capacity(reachable.len());
        while let Some(node_id) = queue.pop_front() {
            ordered.push(node_id);
            for edge in self
                .connections
                .iter()
                .filter(|edge| edge.from_node == node_id)
            {
                if !reachable.contains(&edge.to_node) {
                    continue;
                }
                if let Some(entry) = indegree.get_mut(&edge.to_node) {
                    *entry -= 1;
                    if *entry == 0 {
                        queue.push_back(edge.to_node);
                    }
                }
            }
        }

        if ordered.len() != reachable.len() {
            let cycle = indegree
                .into_iter()
                .filter_map(|(node_id, degree)| (degree > 0).then_some(node_id))
                .collect();
            return Err(GraphValidationError::Cycle { path: cycle }.into());
        }

        Ok(ordered)
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

fn resolve_output_kind(node: &Node, port: &OutputPort) -> Option<PortKind> {
    node.kind
        .output_port_defs()
        .iter()
        .enumerate()
        .find(|(index, def)| port.matches(def, *index))
        .map(|(_, def)| def.kind)
}

fn resolve_input_kind(node: &Node, port: &InputPort) -> Option<PortKind> {
    node.kind
        .input_port_defs()
        .iter()
        .enumerate()
        .find(|(index, def)| port.matches(def, *index))
        .map(|(_, def)| def.kind)
}

fn port_name_in(port: &InputPort) -> String {
    match port {
        InputPort::Named(name) => name.clone(),
        InputPort::Indexed(index) => format!("input_{index}"),
    }
}

fn port_name_out(port: &OutputPort) -> String {
    match port {
        OutputPort::Named(name) => name.clone(),
        OutputPort::Indexed(index) => format!("output_{index}"),
    }
}
