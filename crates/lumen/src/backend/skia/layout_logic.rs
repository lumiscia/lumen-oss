#[derive(Debug, Clone, Copy)]
struct ClipBounds {
    left: f32,
    top: f32,
    right: f32,
    bottom: f32,
}

fn layout_cache_key(operation: &CompiledOperation) -> usize {
    operation as *const CompiledOperation as usize
}

fn approx_eq(left: f32, right: f32) -> bool {
    (left - right).abs() <= 0.5
}

fn scalar_opt_to_f32(s: &Option<Scalar>) -> Option<f32> {
    match s {
        None => None,
        Some(Scalar::Literal(v)) => Some(*v),
        Some(Scalar::Expr(_)) => None, // deferred — callers handle Expr separately
    }
}

fn build_layout_render_tree(
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    layout: &LayoutClip,
    width: f32,
    height: f32,
    clip_index: &ClipPropertyIndex,
) -> Result<LayoutRenderTree, RenderError> {
    let mut taffy = TaffyTree::<()>::new();
    let mut node_id_map: HashMap<String, TaffyNodeId> = HashMap::new();
    let root = build_layout_render_node(
        &mut taffy,
        typeface,
        font_cache,
        &layout.root,
        &mut node_id_map,
    )?;

    let available = TaffySize {
        width: AvailableSpace::Definite(width),
        height: AvailableSpace::Definite(height),
    };
    taffy
        .compute_layout(root.taffy_node, available)
        .map_err(|err| RenderError::SurfaceCreation(format!("taffy compute failed: {err}")))?;

    // Second pass if any nodes have deferred dimension exprs
    let first_pass_layouts = collect_named_layouts(&taffy, &node_id_map);
    let layout_ctx = LayoutNodeExprCtx {
        layouts: first_pass_layouts,
    };
    let combined_ctx = RuntimeExprCtx {
        static_ctx: clip_index,
        layout_ctx: Some(&layout_ctx),
    };
    let any_changed = apply_deferred_layout_dims(&mut taffy, &root, &combined_ctx);
    if any_changed {
        taffy
            .compute_layout(root.taffy_node, available)
            .map_err(|err| {
                RenderError::SurfaceCreation(format!("taffy recompute failed: {err}"))
            })?;
    }

    let named_layouts = collect_named_layouts(&taffy, &node_id_map);

    Ok(LayoutRenderTree {
        taffy,
        root,
        named_layouts,
    })
}

fn collect_named_layouts(
    taffy: &TaffyTree<()>,
    node_id_map: &HashMap<String, TaffyNodeId>,
) -> HashMap<String, (f32, f32, f32, f32)> {
    let mut named_layouts: HashMap<String, (f32, f32, f32, f32)> = HashMap::new();
    for (id, taffy_id) in node_id_map {
        if let Ok(lay) = taffy.layout(*taffy_id) {
            named_layouts.insert(
                id.clone(),
                (
                    lay.size.width,
                    lay.size.height,
                    lay.location.x,
                    lay.location.y,
                ),
            );
        }
    }
    named_layouts
}

fn build_layout_render_node(
    taffy: &mut TaffyTree<()>,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    node: &LayoutNode,
    node_id_map: &mut HashMap<String, TaffyNodeId>,
) -> Result<LayoutRenderNode, RenderError> {
    let base_style = layout_style_to_taffy(&node.style);

    let has_deferred_dims = [
        &node.style.width,
        &node.style.height,
        &node.style.min_width,
        &node.style.min_height,
        &node.style.max_width,
        &node.style.max_height,
    ]
    .iter()
    .any(|s| matches!(s, Some(Scalar::Expr(_))));

    match &node.kind {
        LayoutNodeKind::Container { children } => {
            let mut rendered_children = Vec::with_capacity(children.len());
            let mut child_nodes = Vec::with_capacity(children.len());
            for child in children {
                let rendered_child =
                    build_layout_render_node(taffy, typeface, font_cache, child, node_id_map)?;
                child_nodes.push(rendered_child.taffy_node);
                rendered_children.push(rendered_child);
            }
            let taffy_node = taffy
                .new_with_children(base_style, &child_nodes)
                .map_err(|err| RenderError::SurfaceCreation(format!("taffy node failed: {err}")))?;
            if let Some(id) = &node.id {
                node_id_map.insert(id.clone(), taffy_node);
            }
            Ok(LayoutRenderNode {
                taffy_node,
                style: node.style.clone(),
                has_deferred_dims,
                kind: LayoutRenderNodeKind::Container {
                    children: rendered_children,
                },
            })
        }
        LayoutNodeKind::Text(text_node) => {
            let measured = measure_layout_text_block(typeface, font_cache, text_node, &node.style);
            let width = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.width),
                scalar_opt_to_f32(&node.style.min_width),
                scalar_opt_to_f32(&node.style.max_width),
                measured.width,
            );
            let height = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.height),
                scalar_opt_to_f32(&node.style.min_height),
                scalar_opt_to_f32(&node.style.max_height),
                measured.height,
            );

            let mut leaf_style = base_style;
            leaf_style.size = TaffySize {
                width: Dimension::length(width),
                height: Dimension::length(height),
            };

            let taffy_node = taffy
                .new_leaf(leaf_style)
                .map_err(|err| RenderError::SurfaceCreation(format!("taffy leaf failed: {err}")))?;
            if let Some(id) = &node.id {
                node_id_map.insert(id.clone(), taffy_node);
            }
            Ok(LayoutRenderNode {
                taffy_node,
                style: node.style.clone(),
                has_deferred_dims,
                kind: LayoutRenderNodeKind::Text(LayoutTextRender {
                    lines: measured.lines,
                    line_widths: measured.line_widths,
                    font_size: text_node.font_size.max(1.0),
                    line_height: measured.line_height,
                    color: text_node.color,
                    align: text_node.align,
                }),
            })
        }
        LayoutNodeKind::Image(image_node) => {
            let intrinsic_width = scalar_opt_to_f32(&node.style.width)
                .or_else(|| scalar_opt_to_f32(&node.style.max_width))
                .or_else(|| scalar_opt_to_f32(&node.style.min_width))
                .unwrap_or(1.0)
                .max(1.0);
            let intrinsic_height = scalar_opt_to_f32(&node.style.height)
                .or_else(|| scalar_opt_to_f32(&node.style.max_height))
                .or_else(|| scalar_opt_to_f32(&node.style.min_height))
                .unwrap_or(1.0)
                .max(1.0);
            let width = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.width),
                scalar_opt_to_f32(&node.style.min_width),
                scalar_opt_to_f32(&node.style.max_width),
                intrinsic_width,
            );
            let height = resolve_layout_dimension(
                scalar_opt_to_f32(&node.style.height),
                scalar_opt_to_f32(&node.style.min_height),
                scalar_opt_to_f32(&node.style.max_height),
                intrinsic_height,
            );

            let mut leaf_style = base_style;
            leaf_style.size = TaffySize {
                width: Dimension::length(width),
                height: Dimension::length(height),
            };
            let taffy_node = taffy
                .new_leaf(leaf_style)
                .map_err(|err| RenderError::SurfaceCreation(format!("taffy leaf failed: {err}")))?;
            if let Some(id) = &node.id {
                node_id_map.insert(id.clone(), taffy_node);
            }
            Ok(LayoutRenderNode {
                taffy_node,
                style: node.style.clone(),
                has_deferred_dims,
                kind: LayoutRenderNodeKind::Image(LayoutImageRender {
                    source: image_node.source.clone(),
                    fit: image_node.fit,
                    corner_radius: image_node.corner_radius,
                }),
            })
        }
    }
}

fn apply_deferred_layout_dims(
    taffy: &mut TaffyTree<()>,
    node: &LayoutRenderNode,
    ctx: &dyn ExprEvalCtx,
) -> bool {
    let mut any_changed = false;

    // Recurse into children first
    if let LayoutRenderNodeKind::Container { children } = &node.kind {
        for child in children {
            any_changed |= apply_deferred_layout_dims(taffy, child, ctx);
        }
    }

    if !node.has_deferred_dims {
        return any_changed;
    }

    let Ok(current_style) = taffy.style(node.taffy_node) else {
        return any_changed;
    };
    let mut updated_style = current_style.clone();
    let mut node_changed = false;

    // Resolve a single Scalar::Expr field against ctx
    let try_resolve = |s: &Option<Scalar>| -> Option<f32> {
        match s {
            Some(Scalar::Expr(expr_str)) => {
                let parsed = parse_expr(expr_str.as_str()).ok()?;
                eval_expr(&parsed, ctx).ok()
            }
            _ => None,
        }
    };

    // Helper: check if a Dimension is significantly different from a resolved f32.
    // Uses resolve_to_option since Dimension is a newtype (taffy 0.9), not an enum.
    let is_significant_change = |dim: Dimension, v: f32| -> bool {
        match dim.into_option() {
            Some(x) => (x - v).abs() > 0.5,
            None => true, // auto → any concrete value is a change
        }
    };

    // size.width
    if let Some(v) = try_resolve(&node.style.width) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.size.width, v) {
            updated_style.size.width = new_dim;
            node_changed = true;
        }
    }
    // size.height
    if let Some(v) = try_resolve(&node.style.height) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.size.height, v) {
            updated_style.size.height = new_dim;
            node_changed = true;
        }
    }
    // min_size.width
    if let Some(v) = try_resolve(&node.style.min_width) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.min_size.width, v) {
            updated_style.min_size.width = new_dim;
            node_changed = true;
        }
    }
    // min_size.height
    if let Some(v) = try_resolve(&node.style.min_height) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.min_size.height, v) {
            updated_style.min_size.height = new_dim;
            node_changed = true;
        }
    }
    // max_size.width
    if let Some(v) = try_resolve(&node.style.max_width) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.max_size.width, v) {
            updated_style.max_size.width = new_dim;
            node_changed = true;
        }
    }
    // max_size.height
    if let Some(v) = try_resolve(&node.style.max_height) {
        let new_dim = Dimension::length(v.max(0.0));
        if is_significant_change(updated_style.max_size.height, v) {
            updated_style.max_size.height = new_dim;
            node_changed = true;
        }
    }

    if node_changed {
        let _ = taffy.set_style(node.taffy_node, updated_style);
        any_changed = true;
    }

    any_changed
}

fn draw_layout_render_tree(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    transform: CompiledTransform,
    opacity: f32,
    blend_mode: BlendMode,
    tree: &LayoutRenderTree,
    provider: &mut dyn FrameProvider,
) -> Result<bool, RenderError> {
    let target_width = transform.width.unwrap_or(1.0).max(1.0);
    let target_height = transform.height.unwrap_or(1.0).max(1.0);
    let clip_bounds = ClipBounds {
        left: transform.x,
        top: transform.y,
        right: transform.x + target_width,
        bottom: transform.y + target_height,
    };

    if transform.rotation_degrees != 0.0 {
        canvas.save();
        let cx = transform.x + target_width * 0.5;
        let cy = transform.y + target_height * 0.5;
        canvas.translate(Point::new(cx, cy));
        canvas.rotate(transform.rotation_degrees, None);
        canvas.translate(Point::new(-cx, -cy));
    }

    let drew = draw_layout_render_node(
        canvas,
        typeface,
        font_cache,
        tree,
        &tree.root,
        transform.x,
        transform.y,
        opacity,
        blend_mode,
        provider,
        &clip_bounds,
    )?;

    if transform.rotation_degrees != 0.0 {
        canvas.restore();
    }

    Ok(drew)
}

#[allow(clippy::too_many_arguments)]
fn draw_layout_render_node(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    tree: &LayoutRenderTree,
    node: &LayoutRenderNode,
    origin_x: f32,
    origin_y: f32,
    opacity: f32,
    blend_mode: BlendMode,
    provider: &mut dyn FrameProvider,
    clip_bounds: &ClipBounds,
) -> Result<bool, RenderError> {
    let layout = tree
        .taffy
        .layout(node.taffy_node)
        .map_err(|err| RenderError::SurfaceCreation(format!("taffy layout failed: {err}")))?;
    let x = origin_x + layout.location.x;
    let y = origin_y + layout.location.y;
    let width = layout.size.width.max(0.0);
    let height = layout.size.height.max(0.0);
    if width <= 0.0 || height <= 0.0 {
        return Ok(false);
    }
    if x >= clip_bounds.right
        || y >= clip_bounds.bottom
        || x + width <= clip_bounds.left
        || y + height <= clip_bounds.top
    {
        return Ok(false);
    }

    let mut drew_any = draw_layout_background(
        canvas,
        &node.style,
        x,
        y,
        width,
        height,
        opacity,
        blend_mode,
    );

    let mut clipped_children = false;
    if node.style.overflow == LayoutOverflow::Hidden {
        if matches!(node.kind, LayoutRenderNodeKind::Container { .. }) {
            canvas.save();
            if node.style.corner_radius > 0.0 {
                let radius = node.style.corner_radius.min(width.min(height) * 0.5);
                let rrect =
                    RRect::new_rect_xy(Rect::from_xywh(x, y, width, height), radius, radius);
                canvas.clip_rrect(rrect, None, Some(true));
            } else {
                canvas.clip_rect(Rect::from_xywh(x, y, width, height), None, Some(true));
            }
            clipped_children = true;
        }
    }
    match &node.kind {
        LayoutRenderNodeKind::Container { children } => {
            for child in children {
                drew_any |= draw_layout_render_node(
                    canvas,
                    typeface,
                    font_cache,
                    tree,
                    child,
                    x,
                    y,
                    opacity,
                    blend_mode,
                    provider,
                    clip_bounds,
                )?;
            }
        }
        LayoutRenderNodeKind::Text(text) => {
            drew_any |= draw_layout_text_lines(
                canvas, typeface, font_cache, x, y, width, opacity, blend_mode, text,
            );
        }
        LayoutRenderNodeKind::Image(image) => {
            if let Some(frame_image) = provider.image(image.source.as_str())? {
                draw_image(
                    canvas,
                    CompiledTransform {
                        x,
                        y,
                        width: Some(width.max(1.0)),
                        height: Some(height.max(1.0)),
                        rotation_degrees: 0.0,
                    },
                    opacity,
                    image.fit,
                    image.corner_radius,
                    &frame_image,
                    blend_mode,
                );
                drew_any = true;
            }
        }
    }

    if clipped_children {
        canvas.restore();
    }

    Ok(drew_any)
}

fn draw_layout_background(
    canvas: &Canvas,
    style: &LayoutNodeStyle,
    x: f32,
    y: f32,
    width: f32,
    height: f32,
    opacity: f32,
    blend_mode: BlendMode,
) -> bool {
    let Some(background) = style.background else {
        return false;
    };
    if width <= 0.0 || height <= 0.0 {
        return false;
    }

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(background, opacity));
    paint.set_anti_alias(true);
    paint.set_style(PaintStyle::Fill);
    paint.set_blend_mode(blend_mode);

    if style.corner_radius > 0.0 {
        let radius = style.corner_radius.min(width.min(height) * 0.5);
        let rrect = RRect::new_rect_xy(Rect::from_xywh(x, y, width, height), radius, radius);
        canvas.draw_rrect(rrect, &paint);
    } else {
        canvas.draw_rect(Rect::from_xywh(x, y, width, height), &paint);
    }

    background.a() > 0 && opacity > 0.0
}

fn draw_layout_text_lines(
    canvas: &Canvas,
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    x: f32,
    y: f32,
    width: f32,
    opacity: f32,
    blend_mode: BlendMode,
    text: &LayoutTextRender,
) -> bool {
    if width <= 0.0 {
        return false;
    }
    let font = font_cache
        .entry(text.font_size.to_bits())
        .or_insert_with(|| Font::from_typeface(typeface, text.font_size.max(1.0)));

    let mut paint = Paint::default();
    paint.set_color(to_sk_color(text.color, opacity));
    paint.set_anti_alias(true);
    paint.set_blend_mode(blend_mode);

    let mut y_cursor = y;
    for (line, line_width) in text.lines.iter().zip(text.line_widths.iter().copied()) {
        let line_x = match text.align {
            TextAlign::Left => x,
            TextAlign::Center => x + (width - line_width) * 0.5,
            TextAlign::Right => x + (width - line_width),
        };
        canvas.draw_str(
            line.as_str(),
            Point::new(line_x, y_cursor + text.font_size),
            font,
            &paint,
        );
        y_cursor += text.line_height;
    }

    !text.lines.is_empty() && text.color.a() > 0 && opacity > 0.0
}

fn layout_style_to_taffy(style: &LayoutNodeStyle) -> TaffyStyle {
    let mut taffy = TaffyStyle::default();
    taffy.display = match style.display {
        LayoutDisplay::Flex => TaffyDisplay::Flex,
        LayoutDisplay::None => TaffyDisplay::None,
    };
    taffy.flex_direction = match style.flex_direction {
        LayoutFlexDirection::Row => TaffyFlexDirection::Row,
        LayoutFlexDirection::Column => TaffyFlexDirection::Column,
    };
    taffy.justify_content = Some(match style.justify_content {
        LayoutJustifyContent::FlexStart => TaffyJustifyContent::FlexStart,
        LayoutJustifyContent::Center => TaffyJustifyContent::Center,
        LayoutJustifyContent::FlexEnd => TaffyJustifyContent::FlexEnd,
        LayoutJustifyContent::SpaceBetween => TaffyJustifyContent::SpaceBetween,
        LayoutJustifyContent::SpaceAround => TaffyJustifyContent::SpaceAround,
        LayoutJustifyContent::SpaceEvenly => TaffyJustifyContent::SpaceEvenly,
    });
    taffy.align_items = Some(match style.align_items {
        LayoutAlignItems::Stretch => TaffyAlignItems::Stretch,
        LayoutAlignItems::FlexStart => TaffyAlignItems::FlexStart,
        LayoutAlignItems::Center => TaffyAlignItems::Center,
        LayoutAlignItems::FlexEnd => TaffyAlignItems::FlexEnd,
    });
    taffy.align_self = match style.align_self {
        LayoutAlignSelf::Auto => None,
        LayoutAlignSelf::Stretch => Some(TaffyAlignSelf::Stretch),
        LayoutAlignSelf::FlexStart => Some(TaffyAlignSelf::FlexStart),
        LayoutAlignSelf::Center => Some(TaffyAlignSelf::Center),
        LayoutAlignSelf::FlexEnd => Some(TaffyAlignSelf::FlexEnd),
    };
    taffy.flex_grow = style.flex_grow;
    taffy.flex_shrink = style.flex_shrink;
    taffy.size = TaffySize {
        width: to_taffy_dimension(&style.width),
        height: to_taffy_dimension(&style.height),
    };
    taffy.min_size = TaffySize {
        width: to_taffy_dimension(&style.min_width),
        height: to_taffy_dimension(&style.min_height),
    };
    taffy.max_size = TaffySize {
        width: to_taffy_dimension(&style.max_width),
        height: to_taffy_dimension(&style.max_height),
    };
    taffy.padding = TaffyRect {
        left: TaffyLengthPercentage::length(style.padding.left),
        right: TaffyLengthPercentage::length(style.padding.right),
        top: TaffyLengthPercentage::length(style.padding.top),
        bottom: TaffyLengthPercentage::length(style.padding.bottom),
    };
    taffy.margin = TaffyRect {
        left: TaffyLengthPercentageAuto::length(style.margin.left),
        right: TaffyLengthPercentageAuto::length(style.margin.right),
        top: TaffyLengthPercentageAuto::length(style.margin.top),
        bottom: TaffyLengthPercentageAuto::length(style.margin.bottom),
    };
    taffy.gap = TaffySize {
        width: TaffyLengthPercentage::length(style.gap),
        height: TaffyLengthPercentage::length(style.gap),
    };
    taffy
}

fn to_taffy_dimension(value: &Option<Scalar>) -> Dimension {
    match value {
        None => Dimension::auto(),
        Some(Scalar::Literal(v)) => Dimension::length(v.max(0.0)),
        Some(Scalar::Expr(_)) => Dimension::auto(), // deferred, first-pass uses auto
    }
}

fn resolve_layout_dimension(
    preferred: Option<f32>,
    min: Option<f32>,
    max: Option<f32>,
    fallback: f32,
) -> f32 {
    let mut resolved = preferred.unwrap_or(fallback).max(0.0);
    if let Some(min) = min {
        resolved = resolved.max(min.max(0.0));
    }
    if let Some(max) = max {
        resolved = resolved.min(max.max(0.0));
    }
    resolved.max(1.0)
}

#[derive(Debug, Clone)]
struct MeasuredLayoutTextBlock {
    lines: Vec<String>,
    line_widths: Vec<f32>,
    width: f32,
    height: f32,
    line_height: f32,
}

fn measure_layout_text_block(
    typeface: &Typeface,
    font_cache: &mut HashMap<u32, Font>,
    text_node: &crate::model::LayoutTextNode,
    style: &LayoutNodeStyle,
) -> MeasuredLayoutTextBlock {
    let font_size = text_node.font_size.max(1.0);
    let font = font_cache
        .entry(font_size.to_bits())
        .or_insert_with(|| Font::from_typeface(typeface, font_size));

    let (_, metrics) = font.metrics();
    let default_line_height = (metrics.descent - metrics.ascent + metrics.leading).max(font_size);
    let line_height = text_node
        .line_height
        .unwrap_or(default_line_height)
        .max(1.0);
    let wrap_width = scalar_opt_to_f32(&style.width)
        .or_else(|| scalar_opt_to_f32(&style.max_width))
        .map(|value| value.max(1.0));
    let lines = wrap_text_for_layout(font, text_node.text.as_str(), wrap_width);
    let line_widths = lines
        .iter()
        .map(|line| measure_font_width(font, line.as_str()))
        .collect::<Vec<_>>();
    let max_line_width = line_widths.iter().copied().fold(0.0_f32, f32::max).max(1.0);
    let width = wrap_width
        .map(|limit| max_line_width.min(limit))
        .unwrap_or(max_line_width)
        .max(1.0);
    let height = (line_height * lines.len().max(1) as f32).max(line_height);

    MeasuredLayoutTextBlock {
        lines,
        line_widths,
        width,
        height,
        line_height,
    }
}

fn wrap_text_for_layout(font: &Font, text: &str, max_width: Option<f32>) -> Vec<String> {
    let Some(max_width) = max_width else {
        let lines: Vec<String> = text.lines().map(ToOwned::to_owned).collect();
        return if lines.is_empty() {
            vec![String::new()]
        } else {
            lines
        };
    };

    let mut wrapped: Vec<String> = Vec::new();
    for paragraph in text.split('\n') {
        wrapped.extend(wrap_layout_paragraph(font, paragraph, max_width));
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
}

fn wrap_layout_paragraph(font: &Font, paragraph: &str, max_width: f32) -> Vec<String> {
    let words: Vec<&str> = paragraph
        .split_whitespace()
        .filter(|word| !word.is_empty())
        .collect();
    if words.is_empty() {
        return vec![String::new()];
    }

    let mut lines: Vec<String> = Vec::new();
    let mut current = String::new();

    for word in words {
        if current.is_empty() {
            if measure_font_width(font, word) <= max_width {
                current.push_str(word);
                continue;
            }
            lines.extend(hard_break_word(font, word, max_width));
            continue;
        }

        let candidate = format!("{current} {word}");
        if measure_font_width(font, candidate.as_str()) <= max_width {
            current = candidate;
            continue;
        }

        lines.push(std::mem::take(&mut current));
        if measure_font_width(font, word) <= max_width {
            current = word.to_string();
            continue;
        }
        lines.extend(hard_break_word(font, word, max_width));
        current.clear();
    }

    if !current.is_empty() {
        lines.push(current);
    }
    if lines.is_empty() {
        lines.push(String::new());
    }
    lines
}

fn hard_break_word(font: &Font, word: &str, max_width: f32) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();

    for character in word.chars() {
        let candidate = format!("{current}{character}");
        if !current.is_empty() && measure_font_width(font, candidate.as_str()) > max_width {
            tokens.push(current);
            current = character.to_string();
            continue;
        }
        if current.is_empty() && measure_font_width(font, candidate.as_str()) > max_width {
            tokens.push(character.to_string());
            continue;
        }
        current.push(character);
    }

    if !current.is_empty() {
        tokens.push(current);
    }
    if tokens.is_empty() {
        tokens.push(word.to_string());
    }
    tokens
}

fn measure_font_width(font: &Font, text: &str) -> f32 {
    let (width, _) = font.measure_str(text, None);
    width
}
