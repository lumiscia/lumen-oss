use std::collections::{HashMap, VecDeque};

use thiserror::Error;

use crate::expression::ExprRef;

#[derive(Debug, Clone)]
pub struct DependencyNode {
    pub path: String,
    pub refs: Vec<ExprRef>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum DependencyError {
    #[error("circular dependency detected among: {nodes:?}")]
    CircularDependency { nodes: Vec<String> },
}

pub fn build_eval_order(nodes: &[DependencyNode]) -> Result<Vec<usize>, DependencyError> {
    let mut index_by_path = HashMap::with_capacity(nodes.len());
    for (index, node) in nodes.iter().enumerate() {
        index_by_path.insert(node.path.clone(), index);
    }

    let mut indegree = vec![0usize; nodes.len()];
    let mut outgoing = vec![Vec::<usize>::new(); nodes.len()];

    for (node_index, node) in nodes.iter().enumerate() {
        for reference in &node.refs {
            let dep_path = format!("{}.{}", reference.target, reference.property);
            if let Some(dep_index) = index_by_path.get(dep_path.as_str()) {
                outgoing[*dep_index].push(node_index);
                indegree[node_index] += 1;
            }
        }
    }

    let mut queue = VecDeque::new();
    for (index, degree) in indegree.iter().enumerate() {
        if *degree == 0 {
            queue.push_back(index);
        }
    }

    let mut ordered = Vec::with_capacity(nodes.len());
    while let Some(index) = queue.pop_front() {
        ordered.push(index);
        for edge in &outgoing[index] {
            indegree[*edge] = indegree[*edge].saturating_sub(1);
            if indegree[*edge] == 0 {
                queue.push_back(*edge);
            }
        }
    }

    if ordered.len() == nodes.len() {
        return Ok(ordered);
    }

    let mut cycle_nodes = Vec::new();
    for (index, degree) in indegree.iter().enumerate() {
        if *degree > 0 {
            cycle_nodes.push(nodes[index].path.clone());
        }
    }

    Err(DependencyError::CircularDependency { nodes: cycle_nodes })
}

#[cfg(test)]
mod tests {
    use super::{DependencyError, DependencyNode, build_eval_order};
    use crate::expression::ExprRef;

    #[test]
    fn orders_linear_dependencies() {
        let nodes = vec![
            DependencyNode {
                path: "a.x".to_string(),
                refs: vec![],
            },
            DependencyNode {
                path: "b.x".to_string(),
                refs: vec![ExprRef {
                    target: "a".to_string(),
                    property: "x".to_string(),
                }],
            },
        ];

        let order = build_eval_order(nodes.as_slice()).expect("order");
        assert_eq!(order.len(), 2);
        assert_eq!(order[0], 0);
        assert_eq!(order[1], 1);
    }

    #[test]
    fn detects_cycle() {
        let nodes = vec![
            DependencyNode {
                path: "a.x".to_string(),
                refs: vec![ExprRef {
                    target: "b".to_string(),
                    property: "x".to_string(),
                }],
            },
            DependencyNode {
                path: "b.x".to_string(),
                refs: vec![ExprRef {
                    target: "a".to_string(),
                    property: "x".to_string(),
                }],
            },
        ];

        let error = build_eval_order(nodes.as_slice()).expect_err("cycle expected");
        assert!(matches!(error, DependencyError::CircularDependency { .. }));
    }
}
