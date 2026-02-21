use std::collections::HashSet;

use crate::model::{Layer, LayerItem, Project};

use super::CompileError;

/// Upper bound on timeline duration to prevent OOM from malicious/malformed projects.
/// 1_000_000 frames at 30 fps ≈ 9.25 hours — generous for any legitimate use.
const MAX_DURATION_FRAMES: u64 = 1_000_000;

pub(super) fn validate_project(project: &Project, scale: f32) -> Result<(), CompileError> {
    if !scale.is_finite() || scale <= 0.0 {
        return Err(CompileError::InvalidScale(scale));
    }

    if project.version != "1" {
        return Err(CompileError::UnsupportedVersion(project.version.clone()));
    }

    if project.canvas.width == 0 || project.canvas.height == 0 {
        return Err(CompileError::InvalidCanvas(
            "canvas width and height must be > 0".to_string(),
        ));
    }

    if project.timeline.fps.num == 0 || project.timeline.fps.den == 0 {
        return Err(CompileError::InvalidTimeline(
            "timeline fps numerator and denominator must be > 0".to_string(),
        ));
    }

    if project.timeline.duration_frames == 0 {
        return Err(CompileError::InvalidTimeline(
            "timeline duration_frames must be > 0".to_string(),
        ));
    }

    if project.timeline.duration_frames > MAX_DURATION_FRAMES {
        return Err(CompileError::InvalidTimeline(format!(
            "timeline duration_frames {} exceeds maximum {}",
            project.timeline.duration_frames, MAX_DURATION_FRAMES
        )));
    }

    Ok(())
}

pub(super) fn validate_item_ids(layers: &[Layer]) -> Result<(), CompileError> {
    let mut layer_ids = HashSet::new();
    let mut mask_ids = HashSet::new();

    for layer in layers {
        for item in &layer.items {
            collect_item_ids(item, false, &mut layer_ids, &mut mask_ids)?;
        }
    }

    Ok(())
}

fn collect_item_ids(
    item: &LayerItem,
    in_mask: bool,
    layer_ids: &mut HashSet<String>,
    mask_ids: &mut HashSet<String>,
) -> Result<(), CompileError> {
    let id = item.id().to_string();
    if in_mask {
        if layer_ids.contains(id.as_str()) {
            return Err(CompileError::MaskIdCollision { mask_id: id });
        }
        if !mask_ids.insert(id.clone()) {
            return Err(CompileError::DuplicateItemId(id));
        }
    } else if !layer_ids.insert(id.clone()) {
        return Err(CompileError::DuplicateItemId(id));
    }

    match item {
        LayerItem::Clip(clip) => {
            if let Some(mask) = &clip.mask {
                collect_item_ids(mask, true, layer_ids, mask_ids)?;
            }
        }
        LayerItem::Group(group) => {
            for child in &group.items {
                collect_item_ids(child, in_mask, layer_ids, mask_ids)?;
            }
            if let Some(mask) = &group.mask {
                collect_item_ids(mask, true, layer_ids, mask_ids)?;
            }
        }
    }

    Ok(())
}
