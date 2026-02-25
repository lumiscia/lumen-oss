use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, VectorData},
    render::RenderContext,
};

const INPUT_PORT_DEFS: &[InputPortDef] = &[
    InputPortDef {
        name: "base",
        kind: PortKind::Vector,
        optional: false,
    },
    InputPortDef {
        name: "overlay",
        kind: PortKind::Vector,
        optional: false,
    },
];

const OUTPUT_PORT_DEFS: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::Vector,
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct VectorMerge;

impl NodeEval for VectorMerge {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        INPUT_PORT_DEFS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        OUTPUT_PORT_DEFS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let base = inputs.get_vector("base")?.clone();
        let overlay = inputs.get_vector("overlay")?.clone();

        Ok(PortValue::Vector(VectorData::Group(vec![base, overlay])))
    }
}
