use crate::{
    sequence::{BlendMode, TextAlign, Transform},
    time::{FrameIndex, Rational, Time},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CanvasSpec {
    pub width: u32,
    pub height: u32,
    pub background: crate::sequence::ColorRGBA,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderPlan {
    pub canvas: CanvasSpec,
    pub fps: Rational,
    pub duration: Time,
    pub total_frames: u64,
    pub operations: Vec<RenderOp>,
}

impl RenderPlan {
    pub fn operations_for_frame(&self, frame: FrameIndex) -> impl Iterator<Item = &RenderOp> {
        self.operations
            .iter()
            .filter(move |op| frame >= op.start_frame && frame < op.end_frame)
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOp {
    pub id: String,
    pub start_frame: FrameIndex,
    pub end_frame: FrameIndex,
    pub z_index: u32,
    pub clip_index: usize,
    pub opacity: f32,
    pub blend_mode: BlendMode,
    pub transform: Transform,
    pub kind: RenderOpKind,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RenderOpKind {
    Text(TextRenderOp),
    Image(AssetRenderOp),
    Video(AssetRenderOp),
    Solid(SolidRenderOp),
}

#[derive(Debug, Clone, PartialEq)]
pub struct TextRenderOp {
    pub text: String,
    pub font_family: Option<String>,
    pub font_size: f32,
    pub color: crate::sequence::ColorRGBA,
    pub align: TextAlign,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssetRenderOp {
    pub asset_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SolidRenderOp {
    pub color: crate::sequence::ColorRGBA,
}
