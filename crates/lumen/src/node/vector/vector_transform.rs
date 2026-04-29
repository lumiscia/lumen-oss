use crate::{
    node::{NodeId, NodeProperty, PortRef, VectorData, VectorPosition, VectorTransformData},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct VectorTransform {
    pub id: NodeId,

    #[property(expected = Float)]
    pub scale_x: NodeProperty,
    #[property(expected = Float)]
    pub scale_y: NodeProperty,
    #[property(expected = Float)]
    pub translate_x: NodeProperty,
    #[property(expected = Float)]
    pub translate_y: NodeProperty,
    #[property(expected = Float)]
    pub rotate: NodeProperty,
    #[property(expected = Float)]
    pub pivot_x: NodeProperty,
    #[property(expected = Float)]
    pub pivot_y: NodeProperty,

    #[input(kind = Vector)]
    pub source: PortRef,
}

impl Default for VectorTransform {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            scale_x: NodeProperty::Float(1.0),
            scale_y: NodeProperty::Float(1.0),
            translate_x: NodeProperty::Float(0.0),
            translate_y: NodeProperty::Float(0.0),
            rotate: NodeProperty::Float(0.0),
            pivot_x: NodeProperty::Float(0.0),
            pivot_y: NodeProperty::Float(0.0),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl VectorTransform {
    #[output(port = "vector", kind = Vector)]
    fn eval_vector(&self, ctx: &mut RenderContext) -> crate::Result<VectorData> {
        let source = ctx.eval(&self.source)?.as_vector()?.clone();
        let transform = VectorTransformData {
            translate: VectorPosition {
                x: self.resolve_translate_x(ctx)? as f32,
                y: self.resolve_translate_y(ctx)? as f32,
            },
            scale_x: self.resolve_scale_x(ctx)? as f32,
            scale_y: self.resolve_scale_y(ctx)? as f32,
            rotate: self.resolve_rotate(ctx)? as f32,
            pivot: VectorPosition {
                x: self.resolve_pivot_x(ctx)? as f32,
                y: self.resolve_pivot_y(ctx)? as f32,
            },
        };

        Ok(apply_transform(source, transform))
    }
}

pub fn apply_transform(vector: VectorData, transform: VectorTransformData) -> VectorData {
    if transform.is_identity() {
        return vector;
    }

    VectorData::Transformed {
        child: Box::new(vector),
        transform,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{ShapeGeometry, VectorStyle};

    #[test]
    fn wraps_vector_with_transform_data() {
        let shape = VectorData::Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 4,
                height: 2,
                border_radius: 0.0,
            },
            style: VectorStyle::default(),
            position: VectorPosition { x: 1.0, y: 2.0 },
        };
        let transformed = apply_transform(
            shape,
            VectorTransformData {
                translate: VectorPosition { x: 5.0, y: -3.0 },
                scale_x: 2.0,
                scale_y: 1.5,
                rotate: 30.0,
                pivot: VectorPosition { x: 1.0, y: 1.0 },
            },
        );

        match transformed {
            VectorData::Transformed { transform, .. } => {
                assert_eq!(transform.translate, VectorPosition { x: 5.0, y: -3.0 });
                assert_eq!(transform.scale_x, 2.0);
                assert_eq!(transform.scale_y, 1.5);
                assert_eq!(transform.rotate, 30.0);
            }
            other => panic!("expected transformed vector, got {other:?}"),
        }
    }
}
