#[derive(Debug, Clone)]
struct CachedLayoutClip {
    width: f32,
    height: f32,
    tree: LayoutRenderTree,
}

#[derive(Debug, Clone)]
struct LayoutRenderTree {
    taffy: TaffyTree<()>,
    root: LayoutRenderNode,
    named_layouts: HashMap<String, (f32, f32, f32, f32)>,
}

#[derive(Debug, Clone)]
struct LayoutRenderNode {
    taffy_node: TaffyNodeId,
    style: LayoutNodeStyle,
    kind: LayoutRenderNodeKind,
    has_deferred_dims: bool,
}

#[derive(Debug, Clone)]
enum LayoutRenderNodeKind {
    Container { children: Vec<LayoutRenderNode> },
    Text(LayoutTextRender),
    Image(LayoutImageRender),
}

#[derive(Debug, Clone)]
struct LayoutTextRender {
    lines: Vec<String>,
    line_widths: Vec<f32>,
    font_size: f32,
    line_height: f32,
    color: ColorRgba,
    align: TextAlign,
}

#[derive(Debug, Clone)]
struct LayoutImageRender {
    source: String,
    fit: FitMode,
    corner_radius: f32,
}
struct LayoutNodeExprCtx {
    // node_id -> (computed_width, computed_height, computed_x, computed_y)
    layouts: HashMap<String, (f32, f32, f32, f32)>,
}

impl ExprEvalCtx for LayoutNodeExprCtx {
    fn resolve(&self, target: &str, property: ExprProp) -> Option<f32> {
        let (w, h, x, y) = self.layouts.get(target)?;
        match property {
            ExprProp::Width => Some(*w),
            ExprProp::Height => Some(*h),
            ExprProp::X => Some(*x),
            ExprProp::Y => Some(*y),
        }
    }
}
