use skia_safe::{
    Data, Paint, Rect, SamplingOptions, TileMode,
    runtime_effect::{ChildPtr, RuntimeEffect},
};

use crate::{
    error::RenderError,
    media::MediaStore,
    node::{
        NodeId,
        pixel_utils::{ClearMode, render_to_surface_ephemeral},
    },
    raster::{AlphaMode, RasterFrame},
    render::{RenderContext, surface::SurfacePool},
};

pub(crate) const SOURCE_CHILD_NAME: &str = "source";
pub(crate) const INPUT_CHILD_NAME: &str = "input";
pub(crate) const RESOLUTION_UNIFORM_NAME: &str = "resolution";

pub(crate) struct ShaderUniform<'a> {
    pub name: &'a str,
    pub values: &'a [f32],
}

pub(crate) struct ChildShader<'a> {
    pub name: &'a str,
    pub shader: skia_safe::Shader,
}

pub(crate) fn apply_runtime_shader<S: SurfacePool, M: MediaStore>(
    source: &RasterFrame,
    shader_source: &str,
    uniforms: &[ShaderUniform<'_>],
    alpha_mode: AlphaMode,
    node_id: NodeId,
    node_kind: &'static str,
    frame: u32,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<RasterFrame> {
    apply_runtime_shader_with_children(
        source,
        shader_source,
        uniforms,
        &[],
        alpha_mode,
        node_id,
        node_kind,
        frame,
        ctx,
    )
}

pub(crate) fn apply_runtime_shader_with_children<S: SurfacePool, M: MediaStore>(
    source: &RasterFrame,
    shader_source: &str,
    uniforms: &[ShaderUniform<'_>],
    child_shaders: &[ChildShader<'_>],
    alpha_mode: AlphaMode,
    node_id: NodeId,
    node_kind: &'static str,
    frame: u32,
    ctx: &mut RenderContext<'_, S, M>,
) -> crate::Result<RasterFrame> {
    let (image, width, height) = match source.image_parts() {
        Some(parts) => parts,
        None => return source.snapshot(),
    };

    if width == 0 || height == 0 {
        return source.snapshot();
    }

    let effect = RuntimeEffect::make_for_shader(shader_source, None).map_err(|details| {
        shader_error(
            node_id,
            node_kind,
            frame,
            format!("SkSL shader compilation failed: {details}"),
        )
    })?;

    let source_shader = image
        .to_shader(
            Some((TileMode::Clamp, TileMode::Clamp)),
            SamplingOptions::default(),
            None,
        )
        .ok_or_else(|| {
            shader_error(
                node_id,
                node_kind,
                frame,
                "source image shader creation failed",
            )
        })?;

    let children = build_children(
        &effect,
        source_shader,
        child_shaders,
        node_id,
        node_kind,
        frame,
    )?;
    let uniform_data = build_uniform_data(&effect, uniforms, width, height);
    let shader = effect
        .make_shader(Data::new_copy(&uniform_data), &children, None)
        .ok_or_else(|| shader_error(node_id, node_kind, frame, "runtime shader creation failed"))?;

    render_to_surface_ephemeral(
        width,
        height,
        ctx,
        source.format_rect(),
        source.data_rect(),
        alpha_mode,
        ClearMode::Transparent,
        |canvas| {
            let mut paint = Paint::default();
            paint.set_shader(shader);
            canvas.draw_rect(Rect::from_wh(width as f32, height as f32), &paint);
        },
    )
}

fn build_children(
    effect: &RuntimeEffect,
    source_shader: skia_safe::Shader,
    child_shaders: &[ChildShader<'_>],
    node_id: NodeId,
    node_kind: &'static str,
    frame: u32,
) -> crate::Result<Vec<ChildPtr>> {
    let children = effect.children();
    let mut resolved = Vec::with_capacity(children.len());

    for child in children {
        let name = child.name();
        if name == SOURCE_CHILD_NAME || name == INPUT_CHILD_NAME {
            resolved.push(ChildPtr::Shader(source_shader.clone()));
        } else if let Some(child_shader) = child_shaders.iter().find(|shader| shader.name == name) {
            resolved.push(ChildPtr::Shader(child_shader.shader.clone()));
        } else {
            return Err(shader_error(
                node_id,
                node_kind,
                frame,
                format!(
                    "unsupported shader child `{name}`; use `{SOURCE_CHILD_NAME}` or `{INPUT_CHILD_NAME}`"
                ),
            ));
        }
    }

    Ok(resolved)
}

fn build_uniform_data(
    effect: &RuntimeEffect,
    uniforms: &[ShaderUniform<'_>],
    width: u32,
    height: u32,
) -> Vec<u8> {
    let mut data = vec![0; effect.uniform_size()];

    for uniform in uniforms {
        write_uniform_f32(effect, &mut data, uniform.name, uniform.values);
    }
    write_uniform_f32(
        effect,
        &mut data,
        RESOLUTION_UNIFORM_NAME,
        &[width as f32, height as f32],
    );

    data
}

pub(crate) fn write_uniform_f32(
    effect: &RuntimeEffect,
    data: &mut [u8],
    name: &str,
    values: &[f32],
) {
    let Some(uniform) = effect.find_uniform(name) else {
        return;
    };
    let start = uniform.offset();
    let byte_len = values.len().saturating_mul(size_of::<f32>());
    let end = start.saturating_add(byte_len);
    if end > data.len() || byte_len > uniform.size_in_bytes() {
        return;
    }

    for (index, value) in values.iter().enumerate() {
        let offset = start + index * size_of::<f32>();
        data[offset..offset + size_of::<f32>()].copy_from_slice(&value.to_ne_bytes());
    }
}

pub(crate) fn shader_error(
    node_id: NodeId,
    node_kind: &'static str,
    frame: u32,
    details: impl Into<String>,
) -> crate::error::LumenError {
    RenderError::NodeEvaluation {
        frame,
        node_id,
        node_kind,
        details: details.into(),
    }
    .into()
}
