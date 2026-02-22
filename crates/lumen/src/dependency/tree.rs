use std::collections::{BTreeSet, HashMap, HashSet};

use thiserror::Error;

use crate::expr::ExpressionId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DependencyNode {
    Expression(ExpressionId),
    ClipProperty { clip_id: String, property: String },
    LayoutProperty { node_id: String, property: String },
}

#[derive(Debug, Clone, Default)]
pub struct DependencyTree {
    pub outgoing: HashMap<DependencyNode, HashSet<DependencyNode>>,
    pub incoming: HashMap<DependencyNode, HashSet<DependencyNode>>,
}

#[derive(Debug, Error)]
pub enum DependencyTreeError {
    #[error("dependency cycle detected")]
    Cycle,
}

impl DependencyTree {
    pub fn add_node(&mut self, node: DependencyNode) {
        self.outgoing.entry(node.clone()).or_default();
        self.incoming.entry(node).or_default();
    }

    pub fn add_edge(&mut self, from: DependencyNode, to: DependencyNode) {
        self.outgoing
            .entry(from.clone())
            .or_default()
            .insert(to.clone());
        self.incoming.entry(to).or_default().insert(from);
    }

    fn all_nodes(&self) -> BTreeSet<DependencyNode> {
        let mut nodes = BTreeSet::new();

        for (node, outgoing) in &self.outgoing {
            nodes.insert(node.clone());
            nodes.extend(outgoing.iter().cloned());
        }

        for (node, incoming) in &self.incoming {
            nodes.insert(node.clone());
            nodes.extend(incoming.iter().cloned());
        }

        nodes
    }

    pub fn topological_order(&self) -> Result<Vec<DependencyNode>, DependencyTreeError> {
        let nodes = self.all_nodes();
        let mut in_degree = HashMap::with_capacity(nodes.len());
        let mut ready = BTreeSet::new();
        let mut order = Vec::with_capacity(nodes.len());

        for node in &nodes {
            let degree = self.incoming.get(node).map_or(0, HashSet::len);
            in_degree.insert(node.clone(), degree);
            if degree == 0 {
                ready.insert(node.clone());
            }
        }

        while let Some(node) = ready.pop_first() {
            order.push(node.clone());

            let mut dependents = self
                .outgoing
                .get(&node)
                .map(|outgoing| outgoing.iter().cloned().collect::<Vec<_>>())
                .unwrap_or_default();
            dependents.sort();

            for dependent in dependents {
                if let Some(degree) = in_degree.get_mut(&dependent) {
                    *degree = degree.saturating_sub(1);
                    if *degree == 0 {
                        ready.insert(dependent);
                    }
                }
            }
        }

        if order.len() != in_degree.len() {
            return Err(DependencyTreeError::Cycle);
        }

        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::{DependencyNode, DependencyTree, DependencyTreeError};
    use crate::expr::ExpressionId;

    fn expr(id: &str) -> DependencyNode {
        DependencyNode::Expression(ExpressionId(id.to_owned()))
    }

    #[test]
    fn topological_order_handles_simple_chain() {
        let mut tree = DependencyTree::default();
        tree.add_edge(expr("a"), expr("b"));
        tree.add_edge(expr("b"), expr("c"));

        let order = tree
            .topological_order()
            .expect("topological sort should succeed");

        assert_eq!(order, vec![expr("a"), expr("b"), expr("c")]);
    }

    #[test]
    fn topological_order_handles_diamond_dependency() {
        let mut tree = DependencyTree::default();
        tree.add_edge(expr("a"), expr("b"));
        tree.add_edge(expr("a"), expr("c"));
        tree.add_edge(expr("b"), expr("d"));
        tree.add_edge(expr("c"), expr("d"));

        let order = tree
            .topological_order()
            .expect("topological sort should succeed");
        let index = |id: &str| {
            order
                .iter()
                .position(|node| node == &expr(id))
                .expect("node should exist")
        };

        assert!(index("a") < index("b"));
        assert!(index("a") < index("c"));
        assert!(index("b") < index("d"));
        assert!(index("c") < index("d"));
    }

    #[test]
    fn topological_order_detects_cycle() {
        let mut tree = DependencyTree::default();
        tree.add_edge(expr("a"), expr("b"));
        tree.add_edge(expr("b"), expr("a"));

        let err = tree.topological_order().expect_err("cycle should error");
        assert!(matches!(err, DependencyTreeError::Cycle));
    }
}
