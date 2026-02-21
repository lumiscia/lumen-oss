use skia_safe::{Canvas, Rect};

use crate::backend::RenderError;

pub fn render_masked<F>(canvas: &Canvas, _bounds: Rect, render: F) -> Result<(), RenderError>
where
    F: FnOnce(&Canvas) -> Result<(), RenderError>,
{
    render(canvas)
}
