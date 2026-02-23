use crate::{
    error::LumenError,
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    render::RenderContext,
};

const INPUT_PORTS: [InputPortDef; 1] = [InputPortDef {
    name: "source",
    kind: PortKind::RasterFrame,
    optional: false,
}];

const OUTPUT_PORTS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

#[derive(Debug, Clone)]
pub struct FrameHold {
    pub hold_frame: u32,
}

impl NodeEval for FrameHold {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &INPUT_PORTS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &OUTPUT_PORTS
    }

    fn evaluate(
        &self,
        inputs: &NodeInputs,
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        let source = inputs.get_raster("source")?.clone();
        Ok(PortValue::RasterFrame(source))
    }
}
