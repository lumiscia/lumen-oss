use crate::{
    error::{LumenError, PropertyError},
    node::{InputPortDef, NodeEval, NodeId, NodeInputs, OutputPortDef, PortKind, PortValue},
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
pub struct Memo {
    pub cache_id: String,
    pub allow_expressions: bool,
}

impl NodeEval for Memo {
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
        if self.cache_id.trim().is_empty() {
            return Err(PropertyError::MissingProperty {
                node_id: NodeId(0),
                property_path: "cache_id".to_string(),
            }
            .into());
        }

        let source = inputs.get_raster("source")?.clone();
        Ok(PortValue::RasterFrame(source))
    }
}
