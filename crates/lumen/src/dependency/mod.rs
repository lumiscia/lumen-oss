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

impl DependencyPlan {
    pub fn build(expressions: &[Expression]) -> Self {
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
            .collect::<Vec<_>>();

        let mut tree = tree::DependencyTree::default();
        for expression in expressions {
            tree.add_node(tree::DependencyNode::Expression(expression.id.clone()));
        }
        for dependency in &dependencies {
            let target = match &dependency.target {
                ExpressionReferenceTarget::ClipProperty { clip_id, property } => {
                    tree::DependencyNode::ClipProperty {
                        clip_id: clip_id.clone(),
                        property: format!("{property:?}"),
                    }
                }
                ExpressionReferenceTarget::LayoutNodeProperty { node_id, property } => {
                    tree::DependencyNode::LayoutProperty {
                        node_id: node_id.clone(),
                        property: format!("{property:?}"),
                    }
                }
            };
            tree.add_edge(
                target,
                tree::DependencyNode::Expression(dependency.expression_id.clone()),
            );
        }

        let evaluation_order = tree
            .topological_order()
            .unwrap_or_default()
            .into_iter()
            .filter_map(|node| match node {
                tree::DependencyNode::Expression(id) => Some(id),
                tree::DependencyNode::ClipProperty { .. }
                | tree::DependencyNode::LayoutProperty { .. } => None,
            })
            .collect();

        Self {
            dependencies,
            evaluation_order,
        }
    }
}

pub fn build_dependency_plan(expressions: &[Expression]) -> DependencyPlan {
    DependencyPlan::build(expressions)
}

#[cfg(test)]
mod tests {
    use super::DependencyPlan;
    use crate::expr::{
        Expression, ExpressionId, ExpressionProperty, ExpressionReference,
        ExpressionReferenceTarget,
    };

    #[test]
    fn build_collects_dependencies_and_orders_expressions() {
        let expressions = vec![
            Expression {
                id: ExpressionId("b".to_owned()),
                source: "1".to_owned(),
                references: vec![ExpressionReference {
                    target: ExpressionReferenceTarget::ClipProperty {
                        clip_id: "hero".to_owned(),
                        property: ExpressionProperty::Width,
                    },
                    span: 0..18,
                }],
            },
            Expression {
                id: ExpressionId("a".to_owned()),
                source: "2".to_owned(),
                references: vec![],
            },
        ];

        let plan = DependencyPlan::build(&expressions);

        assert_eq!(plan.dependencies.len(), 1);
        assert_eq!(
            plan.evaluation_order,
            vec![ExpressionId("a".to_owned()), ExpressionId("b".to_owned())]
        );
    }
}
