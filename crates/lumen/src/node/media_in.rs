use crate::{
    error::{LumenError, MediaError},
    node::{InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue},
    render::RenderContext,
};

const INPUT_PORTS: [InputPortDef; 0] = [];

const OUTPUT_PORTS: [OutputPortDef; 1] = [OutputPortDef {
    name: "output",
    kind: PortKind::RasterFrame,
}];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaInKind {
    Image,
    Video,
    ImageSequence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LoopMode {
    None,
    Loop,
    PingPong,
}

#[derive(Debug, Clone)]
pub struct MediaIn {
    pub media_source: String,
    pub kind: MediaInKind,
    pub loop_mode: LoopMode,
}

impl MediaIn {
    fn media_source_context(&self) -> String {
        format!(
            "source={}, kind={:?}, loop_mode={:?}",
            self.media_source, self.kind, self.loop_mode
        )
    }
}

impl NodeEval for MediaIn {
    fn input_port_defs(&self) -> &'static [InputPortDef] {
        &INPUT_PORTS
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        &OUTPUT_PORTS
    }

    fn evaluate(
        &self,
        _inputs: &NodeInputs,
        _ctx: &mut RenderContext,
    ) -> Result<PortValue, LumenError> {
        Err(MediaError::SourceNotFound {
            media_source: self.media_source_context(),
        }
        .into())
    }
}
