use crate::clip::style::{EllipseStyle, PolygonStyle, RectStyle};
use crate::clip::{Clip, ClipMeta};
use crate::render::context::FrameContext;

#[derive(Debug, Clone)]
pub enum ShapeKind {
    Rectangle(RectStyle),
    Ellipse(EllipseStyle),
    Polygon(PolygonStyle),
}

#[derive(Debug, Clone)]
pub struct ShapeClip {
    pub meta: ClipMeta,
    pub kind: ShapeKind,
}

impl Clip for ShapeClip {
    fn meta(&self) -> &ClipMeta {
        &self.meta
    }

    fn draw(&self, _frame: u32, _frame_ctx: &FrameContext) {}
}
