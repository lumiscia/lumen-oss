use crate::{
    error::LumenError,
    node::{
        InputPortDef, NodeEval, NodeInputs, OutputPortDef, PortKind, PortValue, VectorData,
        VectorPosition, VectorStyle, VectorTextData,
        text::{TextAlignment, TextFontStyle},
    },
    render::RenderContext,
};

#[derive(Debug, Clone, PartialEq)]
pub struct VectorText {
    pub content: String,
    pub font_family: String,
    pub font_size: f32,
    pub font_weight: u16,
    pub font_style: TextFontStyle,
    pub max_width: Option<f32>,
    pub alignment: TextAlignment,
    pub position: VectorPosition,
    pub style: VectorStyle,
}

impl Default for VectorText {
    fn default() -> Self {
        Self {
            content: String::new(),
            font_family: "sans-serif".to_string(),
            font_size: 16.0,
            font_weight: 400,
            font_style: TextFontStyle::Normal,
            max_width: None,
            alignment: TextAlignment::default(),
            position: VectorPosition::default(),
            style: VectorStyle::default(),
        }
    }
}

impl NodeEval for VectorText {
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
        Ok(PortValue::Vector(VectorData::Text(VectorTextData {
            content: self.content.clone(),
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            font_weight: self.font_weight,
            font_style: self.font_style,
            max_width: self.max_width,
            alignment: self.alignment,
            position: self.position,
            style: self.style.clone(),
        })))
    }
}
