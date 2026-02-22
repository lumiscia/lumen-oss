pub mod tree;

use crate::expr::{Expression, ExpressionId, ExpressionReferenceTarget};

#[derive(Debug, Clone)]
pub struct ExpressionDependency {
    pub expression_id: ExpressionId,
    pub target: ExpressionReferenceTarget,
}

#[derive(Debug, Clone, Default)]
pub struct DependencyPlan {
    pub dependencies: Vec<ExpressionDependency>,
    pub evaluation_order: Vec<ExpressionId>,
}

pub fn build_dependency_plan(expressions: &[Expression]) -> DependencyPlan {
    let dependencies = expressions
        .iter()
        .flat_map(|expression| {
            expression
                .references
                .iter()
                .cloned()
                .map(|reference| ExpressionDependency {
                    expression_id: expression.id.clone(),
                    target: reference.target,
                })
        })
        .collect();

    DependencyPlan {
        dependencies,
        evaluation_order: Vec::new(),
    }
}
