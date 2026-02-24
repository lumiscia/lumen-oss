use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, ShapeGeometry,
        VectorData,
    },
    render::RenderContext,
};

#[derive(Debug, Clone, PartialEq)]
pub struct Shape {
    pub geometry: ShapeGeometry,
}

impl Default for Shape {
    fn default() -> Self {
        Self {
            geometry: ShapeGeometry::Rectangle {
                width: 1,
                height: 1,
            },
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
        Ok(PortValue::Vector(VectorData::Shape(self.geometry.clone())))
    }
}
