use crate::{
    error::{LumenError, PropertyError},
    node::{
        NodeId, NodeProperty, ShapeGeometry, VectorData, VectorPosition, VectorStroke, VectorStyle,
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct BezierPath {
    pub id: NodeId,

    #[property(expected = String)]
    pub commands: NodeProperty,

    #[property(expected = Vec2)]
    pub position: NodeProperty,

    #[property(expected = Bool)]
    pub fill_enabled: NodeProperty,
    #[property(expected = Color)]
    pub fill_color: NodeProperty,
    #[property(expected = Bool)]
    pub stroke_enabled: NodeProperty,
    #[property(expected = Color)]
    pub stroke_color: NodeProperty,
    #[property(expected = Float)]
    pub stroke_width: NodeProperty,
}

impl Default for BezierPath {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            commands: NodeProperty::String(String::new()),
            position: NodeProperty::Vec2((0.0, 0.0)),
            fill_enabled: NodeProperty::Bool(false),
            fill_color: NodeProperty::Color([255, 255, 255, 255]),
            stroke_enabled: NodeProperty::Bool(false),
            stroke_color: NodeProperty::Color([0, 0, 0, 255]),
            stroke_width: NodeProperty::Float(1.0),
        }
    }
}

#[node_impl]
impl BezierPath {
    #[output(port = "vector", kind = Vector)]
    fn eval_vector(&self, ctx: &mut RenderContext) -> crate::Result<VectorData> {
        let commands = self.resolve_commands(ctx)?;
        validate_path_commands(self.id, &commands)?;

        let (x, y) = self.resolve_position(ctx)?;
        let fill = if self.resolve_fill_enabled(ctx)? {
            Some(self.resolve_fill_color(ctx)?)
        } else {
            None
        };
        let stroke = if self.resolve_stroke_enabled(ctx)? {
            Some(VectorStroke {
                color: self.resolve_stroke_color(ctx)?,
                width: (self.resolve_stroke_width(ctx)? as f32).max(0.0),
            })
        } else {
            None
        };

        Ok(VectorData::Shape {
            geometry: ShapeGeometry::Path { commands },
            style: VectorStyle {
                color: fill,
                stroke,
            },
            position: VectorPosition {
                x: x as f32,
                y: y as f32,
            },
        })
    }
}

pub fn validate_path_commands(node_id: NodeId, commands: &str) -> crate::Result<()> {
    let Some(path) = skia_safe::Path::from_svg(commands) else {
        return Err(invalid_path(node_id));
    };
    let bounds = path.compute_tight_bounds();
    if bounds.is_empty() || !bounds.is_finite() {
        return Err(invalid_path(node_id));
    }
    Ok(())
}

fn invalid_path(node_id: NodeId) -> LumenError {
    LumenError::Property(PropertyError::InvalidType {
        node_id,
        property_path: "commands".to_string(),
        expected: "valid SVG path commands",
        actual: "String",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_invalid_path_commands() {
        assert!(validate_path_commands(NodeId::new(7), "M 0 0 L nope").is_err());
    }

    #[test]
    fn accepts_negative_and_non_zero_local_coordinates() {
        assert!(validate_path_commands(NodeId::new(7), "M -5 10 C 2 12 8 18 12 20").is_ok());
    }
}
