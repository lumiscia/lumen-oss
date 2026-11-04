use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

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

impl crate::gpu::frame_graph::RecordNode for Blur {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        gaussian::programs(programs, false)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let bounds = cx.bounds_of(gaussian::blur_bounds(
            source.domain.data_rect,
            [0.0; 2],
            params.radius,
        ));
        let domain = cx.domain_within(source.domain.display_size(), bounds);
        let tex = gaussian::record_passes(
            cx,
            source,
            domain,
            false,
            &gaussian::GaussianSettings {
                radius: params.radius,
                transparent_edge: false,
                offset: [0.0; 2],
            },
        )?;
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain,
            bounds: None,
        })
    }
}
