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
        Self::try_build(expressions).unwrap_or_else(|_| Self {
            dependencies: expressions
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
                .collect(),
            evaluation_order: expressions.iter().map(|expr| expr.id.clone()).collect(),
        })
    }

    pub fn try_build(expressions: &[Expression]) -> Result<Self, tree::DependencyTreeError> {
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
                        property: property.as_str().to_owned(),
                    }
                }
                ExpressionReferenceTarget::LayoutNodeProperty { node_id, property } => {
                    tree::DependencyNode::LayoutProperty {
                        node_id: node_id.clone(),
                        property: property.as_str().to_owned(),
                    }
                }
            };
            tree.add_edge(
                target,
                tree::DependencyNode::Expression(dependency.expression_id.clone()),
            );
        }

        let evaluation_order = tree
            .topological_order()?
            .into_iter()
            .filter_map(|node| match node {
                tree::DependencyNode::Expression(id) => Some(id),
                tree::DependencyNode::ClipProperty { .. }
                | tree::DependencyNode::LayoutProperty { .. } => None,
            })
            .collect();

        Ok(Self {
            dependencies,
            evaluation_order,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::DependencyPlan;
    use crate::expr::{Expression, ExpressionId};

    #[test]
    fn build_collects_dependencies_and_orders_expressions() {
        let expressions = vec![
            Expression::parse(ExpressionId("b".to_owned()), "clip('hero').width")
                .expect("should parse"),
            Expression::parse(ExpressionId("a".to_owned()), "2").expect("should parse"),
        ];

        let plan = DependencyPlan::build(&expressions);

        assert_eq!(plan.dependencies.len(), 1);
        assert_eq!(
            plan.evaluation_order,
            vec![ExpressionId("a".to_owned()), ExpressionId("b".to_owned())]
        );
    }

    #[test]
    fn try_build_matches_build_on_success() {
        let expressions = vec![
            Expression::parse(ExpressionId("a".to_owned()), "1").expect("should parse"),
        ];

        let plan = DependencyPlan::build(&expressions);
        let try_plan = DependencyPlan::try_build(&expressions).expect("plan should build");

        assert_eq!(plan.evaluation_order, try_plan.evaluation_order);
        assert_eq!(plan.dependencies.len(), try_plan.dependencies.len());
    }
}
