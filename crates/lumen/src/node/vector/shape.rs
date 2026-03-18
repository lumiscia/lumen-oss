use crate::{
    error::LumenError,
    node::{
        NodeId, NodeProperty, ShapeGeometry, VectorData, VectorPosition, VectorStroke, VectorStyle,
    },
    render::RenderContext,
};
use lumen_macros::{Node, node_impl};

#[derive(Debug, Clone, Node)]
pub struct Shape {
    pub id: NodeId,

    #[property(expected = Int)]
    pub geometry_kind: NodeProperty,
    #[property(expected = Int)]
    pub width: NodeProperty,
    #[property(expected = Int)]
    pub height: NodeProperty,
    #[property(expected = Float)]
    pub border_radius: NodeProperty,
    #[property(expected = String)]
    pub polygon_points: NodeProperty,

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

impl Default for Shape {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            geometry_kind: NodeProperty::Int(0),
            width: NodeProperty::Int(1),
            height: NodeProperty::Int(1),
            border_radius: NodeProperty::Float(0.0),
            polygon_points: NodeProperty::String(String::new()),
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
impl Shape {
    #[output(port = "vector", kind = Vector)]
    fn eval_vector(&self, ctx: &mut RenderContext) -> crate::Result<VectorData> {
        let width = self.resolve_width(ctx)?;
        let height = self.resolve_height(ctx)?;
        let border_radius = self.resolve_border_radius(ctx)? as f32;
        let polygon_points = self.resolve_polygon_points(ctx)?;
        let geometry = resolve_geometry(
            self.resolve_geometry_kind(ctx)?,
            width,
            height,
            border_radius,
            &polygon_points,
        );

        let (x, y) = self.resolve_position(ctx)?;
        let position = VectorPosition {
            x: x as f32,
            y: y as f32,
        };

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
            geometry,
            style: VectorStyle {
                color: fill,
                stroke,
            },
            position,
        })
    }
}

impl Shape {
    pub fn with_color(mut self, color: [u8; 4]) -> Self {
        self.fill_enabled = NodeProperty::Bool(true);
        self.fill_color = NodeProperty::Color(color);
        self
    }

    pub fn with_stroke(mut self, stroke: VectorStroke) -> Self {
        self.stroke_enabled = NodeProperty::Bool(true);
        self.stroke_color = NodeProperty::Color(stroke.color);
        self.stroke_width = NodeProperty::Float(f64::from(stroke.width));
        self
    }

    pub fn with_position(mut self, position: VectorPosition) -> Self {
        self.position = NodeProperty::Vec2((f64::from(position.x), f64::from(position.y)));
        self
    }
}

fn resolve_geometry(
    geometry_kind: i64,
    width: i64,
    height: i64,
    border_radius: f32,
    polygon_points: &str,
) -> ShapeGeometry {
    let width = width.max(1) as u32;
    let height = height.max(1) as u32;

    match geometry_kind {
        1 => ShapeGeometry::Ellipse { width, height },
        2 => ShapeGeometry::Polygon {
            points: parse_polygon_points(polygon_points),
        },
        _ => ShapeGeometry::Rectangle {
            width,
            height,
            border_radius,
        },
    }
}

fn parse_polygon_points(raw: &str) -> Vec<(f32, f32)> {
    raw.split(';')
        .filter_map(|pair| {
            let mut parts = pair.trim().split(',');
            let x = parts.next()?.trim().parse::<f32>().ok()?;
            let y = parts.next()?.trim().parse::<f32>().ok()?;
            Some((x, y))
        })
        .collect()
}
