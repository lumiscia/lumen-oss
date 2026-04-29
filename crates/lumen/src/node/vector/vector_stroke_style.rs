use crate::{
    node::{NodeId, NodeProperty, PortRef, VectorData, VectorStroke, VectorStyle},
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct VectorStrokeStyle {
    pub id: NodeId,

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
    #[property(expected = Bool)]
    pub override_existing: NodeProperty,

    #[input(kind = Vector)]
    pub source: PortRef,
}

impl Default for VectorStrokeStyle {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            fill_enabled: NodeProperty::Bool(true),
            fill_color: NodeProperty::Color([255, 255, 255, 255]),
            stroke_enabled: NodeProperty::Bool(false),
            stroke_color: NodeProperty::Color([0, 0, 0, 255]),
            stroke_width: NodeProperty::Float(1.0),
            override_existing: NodeProperty::Bool(false),
            source: PortRef::empty(),
        }
    }
}

#[node_impl]
impl VectorStrokeStyle {
    #[output(port = "vector", kind = Vector)]
    fn eval_vector(&self, ctx: &mut RenderContext) -> crate::Result<VectorData> {
        let source = ctx.eval(&self.source)?.as_vector()?.clone();
        let defaults = VectorStyle {
            color: if self.resolve_fill_enabled(ctx)? {
                Some(self.resolve_fill_color(ctx)?)
            } else {
                None
            },
            stroke: if self.resolve_stroke_enabled(ctx)? {
                Some(VectorStroke {
                    color: self.resolve_stroke_color(ctx)?,
                    width: (self.resolve_stroke_width(ctx)? as f32).max(0.0),
                })
            } else {
                None
            },
        };

        Ok(apply_style_defaults(
            source,
            &defaults,
            self.resolve_override_existing(ctx)?,
        ))
    }
}

pub fn apply_style_defaults(
    mut vector: VectorData,
    defaults: &VectorStyle,
    override_existing: bool,
) -> VectorData {
    match &mut vector {
        VectorData::Shape { style, .. } => merge_style(style, defaults, override_existing),
        VectorData::Text(text) => merge_style(&mut text.style, defaults, override_existing),
        VectorData::Group { children, .. } => {
            for child in children {
                apply_style_defaults_in_place(child, defaults, override_existing);
            }
        }
        VectorData::Transformed { child, .. } => {
            apply_style_defaults_in_place(child, defaults, override_existing);
        }
    }
    vector
}

fn apply_style_defaults_in_place(
    vector: &mut VectorData,
    defaults: &VectorStyle,
    override_existing: bool,
) {
    match vector {
        VectorData::Shape { style, .. } => merge_style(style, defaults, override_existing),
        VectorData::Text(text) => merge_style(&mut text.style, defaults, override_existing),
        VectorData::Group { children, .. } => {
            for child in children {
                apply_style_defaults_in_place(child, defaults, override_existing);
            }
        }
        VectorData::Transformed { child, .. } => {
            apply_style_defaults_in_place(child, defaults, override_existing);
        }
    }
}

fn merge_style(style: &mut VectorStyle, defaults: &VectorStyle, override_existing: bool) {
    if override_existing || style.color.is_none() {
        style.color = defaults.color;
    }
    if override_existing || style.stroke.is_none() {
        style.stroke = defaults.stroke;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::node::{ShapeGeometry, VectorPosition};

    #[test]
    fn applies_fill_and_stroke_to_missing_styles() {
        let vector = VectorData::Shape {
            geometry: ShapeGeometry::Rectangle {
                width: 4,
                height: 2,
                border_radius: 0.0,
            },
            style: VectorStyle::default(),
            position: VectorPosition::default(),
        };

        let styled = apply_style_defaults(
            vector,
            &VectorStyle {
                color: Some([1, 2, 3, 255]),
                stroke: Some(VectorStroke {
                    color: [4, 5, 6, 255],
                    width: 2.0,
                }),
            },
            false,
        );

        match styled {
            VectorData::Shape { style, .. } => {
                assert_eq!(style.color, Some([1, 2, 3, 255]));
                assert_eq!(
                    style.stroke,
                    Some(VectorStroke {
                        color: [4, 5, 6, 255],
                        width: 2.0
                    })
                );
            }
            other => panic!("expected shape, got {other:?}"),
        }
    }
}
