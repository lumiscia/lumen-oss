use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, ShapeGeometry,
        VectorData, VectorPosition, VectorStroke, VectorStyle,
    },
    render::RenderContext,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub geometry: ShapeGeometry,
    pub position: VectorPosition,
    pub style: VectorStyle,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            geometry: ShapeGeometry::Rectangle {
                width: 1,
                height: 1,
                border_radius: 0.0,
            },
            position: VectorPosition::default(),
            style: VectorStyle::default(),
        }
    }
}

impl NodeEval for Shape {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &[]
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &[OutputPortDef {
            name: "vector",
            kind: PortKind::Vector,
        }]
    }

    fn evaluate(
        &self,
        _inputs: &NodeInputs,
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        Ok(PortValue::Vector(VectorData::Shape {
            geometry: self.geometry.clone(),
            style: self.style.clone(),
            position: self.position,
        }))
    }
}

impl Shape {
    pub fn with_color(mut self, color: [u8; 4]) -> Self {
        self.style.color = Some(color);
        self
    }

    pub fn with_stroke(mut self, stroke: VectorStroke) -> Self {
        self.style.stroke = Some(stroke);
        self
    }

    pub fn with_position(mut self, position: VectorPosition) -> Self {
        self.position = position;
        self
    }
}
