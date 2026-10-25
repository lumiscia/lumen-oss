use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("transform.wgsl");

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum TransformSampling {
    Nearest = 0,
    #[default]
    Linear = 1,
}

/// Transforms a raster into the composition canvas bounds.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct TransformParams {
    /// Horizontal scale multiplier.
    #[meta(name = "Scale X", step = 0.1)]
    pub scale_x: f64,
    /// Vertical scale multiplier.
    #[meta(name = "Scale Y", step = 0.1)]
    pub scale_y: f64,
    /// Horizontal translation in pixels.
    #[meta(name = "Translate X", step = 1)]
    pub translate_x: f64,
    /// Vertical translation in pixels.
    #[meta(name = "Translate Y", step = 1)]
    pub translate_y: f64,
    /// Rotation in degrees.
    #[meta(name = "Rotate", step = 1)]
    pub rotate: f64,
    /// Horizontal pivot point in pixels.
    #[meta(name = "Pivot X", step = 1)]
    pub pivot_x: f64,
    /// Vertical pivot point in pixels.
    #[meta(name = "Pivot Y", step = 1)]
    pub pivot_y: f64,
    /// Uses the pivot as given; when off, a (0, 0) pivot means the source centre.
    #[meta(name = "Custom pivot")]
    pub custom_pivot: bool,
    /// Sampling filter used when transforming.
    #[meta(kind = "enum", enum_type = TransformSampling)]
    pub sampling: TransformSampling,
}

impl Default for TransformParams {
    fn default() -> Self {
        Self {
            scale_x: 1.0,
            scale_y: 1.0,
            translate_x: 0.0,
            translate_y: 0.0,
            rotate: 0.0,
            pivot_x: 0.0,
            pivot_y: 0.0,
            custom_pivot: false,
            sampling: TransformSampling::Linear,
        }
    }
}

/// Transforms a raster into the composition canvas bounds.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(kind = "transform", name = "Transform", category = "processing")]
pub struct Transform {
    pub id: NodeId,
    #[params]
    pub params: TransformParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for Transform {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: TransformParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

impl GpuCompileNode for Transform {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "transform",
            SHADER,
            std::mem::size_of::<compiler::TransformParams>() as u64,
            Some(lumen_gpu::Size::new(
                ctx.composition().render_settings.width.max(1),
                ctx.composition().render_settings.height.max(1),
            )),
        )?;
        ctx.register_compiled_node(CompiledTransform {
            node_id: self.id,
            params: self.params.clone(),
            source_size: source.domain.storage_size,
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(lumen_gpu::Size::new(
                ctx.composition().render_settings.width.max(1),
                ctx.composition().render_settings.height.max(1),
            )),
            metadata: source.metadata,
        }))
    }
}

#[derive(Debug, Clone)]
struct CompiledTransform {
    node_id: NodeId,
    params: TransformParamsDelegate,
    source_size: lumen_gpu::Size,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledTransform {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let evaluated = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let pivot = resolve_pivot(
            evaluated.custom_pivot,
            [evaluated.pivot_x as f32, evaluated.pivot_y as f32],
            self.source_size,
        );
        let params = compiler::TransformParams {
            scale: [evaluated.scale_x as f32, evaluated.scale_y as f32],
            translate: [evaluated.translate_x as f32, evaluated.translate_y as f32],
            pivot,
            rotate_radians: (evaluated.rotate as f32).to_radians(),
            sampling: evaluated.sampling as u32,
            _pad: [0; 4],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}

/// Without a custom pivot, a pivot of (0, 0) within 1e-6 selects the source centre.
fn resolve_pivot(custom_pivot: bool, pivot: [f32; 2], source_size: lumen_gpu::Size) -> [f32; 2] {
    if custom_pivot || pivot.iter().any(|component| component.abs() > 0.000001) {
        pivot
    } else {
        [
            source_size.width as f32 * 0.5,
            source_size.height as f32 * 0.5,
        ]
    }
}

#[cfg(test)]
mod tests {
    use super::resolve_pivot;

    #[test]
    fn legacy_pivot_fallback_tolerates_near_zero() {
        let size = lumen_gpu::Size {
            width: 8,
            height: 4,
        };
        assert_eq!(resolve_pivot(false, [0.0000001, 0.0], size), [4.0, 2.0]);
        assert_eq!(resolve_pivot(false, [0.0, 0.0001], size), [0.0, 0.0001]);
        assert_eq!(
            resolve_pivot(true, [0.0000001, 0.0], size),
            [0.0000001, 0.0]
        );
    }
}
