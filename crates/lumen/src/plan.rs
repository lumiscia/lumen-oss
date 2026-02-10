use crate::{
    sequence::{BlendMode, ShapeContent, TextAlign, Transform},
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
    frame_index: Vec<Vec<usize>>,
}

impl RenderPlan {
    pub fn operations_for_frame(&self, frame: FrameIndex) -> impl Iterator<Item = &RenderOp> {
        self.frame_index[frame.0 as usize]
            .iter()
            .filter_map(|index| self.operations.get(*index))
    }

    pub fn with_operations_index(
        canvas: CanvasSpec,
        fps: Rational,
        duration: Time,
        total_frames: u64,
        operations: Vec<RenderOp>,
    ) -> Self {
        let mut frame_index = vec![Vec::new(); total_frames as usize];
        for (op_index, op) in operations.iter().enumerate() {
            let start = op.start_frame.0.min(total_frames) as usize;
            let end = op.end_frame.0.min(total_frames) as usize;
            for frame in start..end {
                frame_index[frame].push(op_index);
            }
        }

        Self {
            canvas,
            fps,
            duration,
            total_frames,
            operations,
            frame_index,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct RenderOp {
    pub id: String,
    pub start_frame: FrameIndex,
    pub end_frame: FrameIndex,
    pub source_in_frame: FrameIndex,
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
    Shape(ShapeRenderOp),
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

#[derive(Debug, Clone, PartialEq)]
pub struct ShapeRenderOp {
    pub shape: ShapeContent,
}
