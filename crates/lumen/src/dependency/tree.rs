use std::collections::{BTreeSet, HashMap, HashSet};

use thiserror::Error;

use crate::expr::ExpressionId;

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum DependencyNode {
    Expression(ExpressionId),
    ClipProperty { clip_id: String, property: String },
    LayoutProperty { node_id: String, property: String },
    ClipRender(String),
}

#[derive(Debug, Clone, Default)]
pub struct DependencyTree {
    pub outgoing: HashMap<DependencyNode, HashSet<DependencyNode>>,
    pub incoming: HashMap<DependencyNode, HashSet<DependencyNode>>,
}

#[derive(Debug, Error)]
pub enum DependencyTreeError {
    #[error("dependency cycle detected involving {nodes_len} node(s)")]
    Cycle {
        nodes: Vec<DependencyNode>,
        nodes_len: usize,
    },
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
            let mut remaining = in_degree
                .into_iter()
                .filter_map(|(node, degree)| if degree > 0 { Some(node) } else { None })
                .collect::<Vec<_>>();
            remaining.sort();
            let nodes_len = remaining.len();
            return Err(DependencyTreeError::Cycle {
                nodes: remaining,
                nodes_len,
            });
        }

        Ok(order)
    }
}

#[cfg(test)]
mod tests {
    use super::{DependencyNode, DependencyTree, DependencyTreeError};
    use crate::expr::ExpressionId;

    use std::{collections::HashMap, panic::catch_unwind};

    fn expr(id: &str) -> DependencyNode {
        DependencyNode::Expression(ExpressionId(id.to_owned()))
    }

    fn next_seed(seed: &mut u64) -> u64 {
        *seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        *seed
    }

    fn node(index: usize) -> DependencyNode {
        expr(&format!("n{index}"))
    }

    fn build_dag(
        seed: u64,
        node_count: usize,
    ) -> (DependencyTree, Vec<(DependencyNode, DependencyNode)>) {
        let mut tree = DependencyTree::default();
        let mut edges = Vec::new();
        let mut state = seed;

        for index in 0..node_count {
            tree.add_node(node(index));
        }

        for from in 0..node_count {
            for to in (from + 1)..node_count {
                if next_seed(&mut state) & 0b11 == 0 {
                    let from_node = node(from);
                    let to_node = node(to);
                    tree.add_edge(from_node.clone(), to_node.clone());
                    edges.push((from_node, to_node));
                }
            }
        }

        (tree, edges)
    }

    #[test]
    fn topological_order_randomized_dags_preserve_edge_order_without_panics() {
        for seed in [1_u64, 7, 19, 42, 1337] {
            let (tree, edges) = build_dag(seed, 12);
            let attempt = catch_unwind(|| tree.topological_order());
            assert!(
                attempt.is_ok(),
                "topological_order panicked for seed {seed}"
            );

            let order = attempt
                .expect("topological_order should not panic")
                .expect("deterministic DAG should sort");
            assert_eq!(order.len(), 12);

            let positions: HashMap<_, _> = order
                .iter()
                .cloned()
                .enumerate()
                .map(|(index, node)| (node, index))
                .collect();

            assert_eq!(positions.len(), order.len());
            for (from, to) in edges {
                let from_index = positions
                    .get(&from)
                    .copied()
                    .expect("from-node must exist in topological order");
                let to_index = positions
                    .get(&to)
                    .copied()
                    .expect("to-node must exist in topological order");
                assert!(
                    from_index < to_index,
                    "edge order violated for {from:?} -> {to:?} with seed {seed}"
                );
            }
        }
    }

    #[test]
    fn topological_order_randomized_cycles_return_cycle_errors_without_panics() {
        for seed in [5_u64, 11, 17, 23] {
            let (mut tree, _) = build_dag(seed, 8);
            for index in 0..7 {
                tree.add_edge(node(index), node(index + 1));
            }
            tree.add_edge(node(7), node(0));

            let attempt = catch_unwind(|| tree.topological_order());
            assert!(
                attempt.is_ok(),
                "topological_order panicked for cyclic seed {seed}"
            );

            let err = attempt
                .expect("topological_order should not panic")
                .expect_err("cycle should return an error");
            match err {
                DependencyTreeError::Cycle { nodes, nodes_len } => {
                    assert_eq!(nodes_len, nodes.len());
                    assert!(!nodes.is_empty());
                    assert!(nodes.contains(&node(0)));
                    assert!(nodes.contains(&node(7)));
                }
            }
        }
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
        assert!(matches!(err, DependencyTreeError::Cycle { .. }));
    }
}
