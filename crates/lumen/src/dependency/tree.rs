use std::collections::{HashMap, HashSet};

use thiserror::Error;

use crate::expr::ExpressionId;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
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
    pub fn topological_order(&self) -> Result<Vec<DependencyNode>, DependencyTreeError> {
        Err(DependencyTreeError::Cycle)
    }
}
