use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use crate::gpu::{
    AlphaMode, BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

use super::gaussian;

#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct BlurParams {
    /// Blur radius in pixels. Fractional radii blend smoothly. Radii above about 588 look the same.
    #[meta(min = 0, step = 0.5)]
    pub radius: f64,
}

impl Default for BlurParams {
    fn default() -> Self {
        Self { radius: 4.0 }
    }
}

/// Applies a separable Gaussian blur to a raster.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "blur", name = "Blur", category = "processing")]
pub struct Blur {
    pub id: NodeId,
    #[params]
    pub params: BlurParamsDelegate,
    #[input()]
    pub source: Option<NodeId>,
}

#[derive(Debug, Clone)]
struct CompiledBlur {
    node_id: NodeId,
    params: BlurParamsDelegate,
    passes: gaussian::GaussianPasses,
    straight_source: bool,
}

impl GpuCompiledNode for CompiledBlur {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        self.passes.write(
            bound,
            &gaussian::GaussianSettings {
                radius: params.radius,
                straight_source: self.straight_source,
                transparent_edge: false,
                offset: [0.0; 2],
            },
        );
        Ok(())
    }
}

impl GpuCompileNode for Blur {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let source = ctx.compile_bounded_input(self.id, "source", self.source)?;
        // The blur spreads the source's data by the radius, up to its largest over the timeline.
        let bounds = ctx.source_bounds(
            compiler::SourceMotion::of(self.params.radius.is_expr(), &self.params),
            |expr| {
                let params = self.params.eval(&DelegateEvalContext {
                    node_id: self.id,
                    expr,
                })?;
                Ok(gaussian::blur_bounds(
                    source.domain.data_rect,
                    [0.0; 2],
                    params.radius,
                ))
            },
        );
        let domain = gaussian::domain_within(source.domain.display_size(), bounds);
        let passes = gaussian::compile_passes_with(
            ctx,
            self.id,
            "blur",
            source,
            domain,
            gaussian::GaussianOptions::default(),
        );
        let output = passes.output;
        ctx.register_compiled_node(CompiledBlur {
            node_id: self.id,
            params: self.params.clone(),
            passes,
            straight_source: source.alpha_mode == AlphaMode::Unpremultiplied,
        });
        Ok(RasterHandle {
            texture: output,
            domain,
            alpha_mode: AlphaMode::Premultiplied,
            bounds: None,
        })
    }
}
