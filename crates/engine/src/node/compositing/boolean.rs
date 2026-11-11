use crate::node::{DelegateEvaluable, NodeId, NodeParams};

use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/sampling.wgsl"),
    include_str!("boolean.wgsl")
);

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum", expressions)]
pub enum BooleanOperation {
    #[default]
    Union = 0,
    Intersect = 1,
    Subtract = 2,
    Xor = 3,
}

/// Combines two raster alpha masks with boolean operations.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct BooleanParams {
    /// Boolean operation used to combine the two input masks.
    #[meta(kind = "enum", enum_type = BooleanOperation)]
    pub operation: BooleanOperation,
    /// Alpha above this cutoff counts as opaque; 0 applies no cutoff and keeps antialiased edges.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub threshold: f64,
}

impl Default for BooleanParams {
    fn default() -> Self {
        Self {
            operation: BooleanOperation::Union,
            threshold: 0.0,
        }
    }
}

/// Combines two raster alpha masks with boolean operations.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "boolean", name = "Boolean", category = "compositing")]
pub struct Boolean {
    pub id: NodeId,
    #[params]
    pub params: BooleanParamsDelegate,

    #[input()]
    pub a: Option<NodeId>,
    #[input()]
    pub b: Option<NodeId>,
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(3, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

impl crate::gpu::frame_graph::RecordNode for Boolean {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("boolean", SHADER, "cs_main", RECORD_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let a = cx.required_full(self.id, "a", self.a)?;
        let b = cx.required_full(self.id, "b", self.b)?;
        let params = self.params.eval(&crate::node::DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let gpu_params = params::BooleanParams {
            values: [params.operation as u32 as f32, params.threshold as f32],
        };
        let size = a.domain.storage_size;
        let tex = cx.storage_target(size);
        let program = cx.program("boolean", SHADER, "cs_main", RECORD_LAYOUT)?;
        let uniform = cx.rec.uniform(&gpu_params);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(a.tex)),
                (1, lumen_gpu::Res::Tex(b.tex)),
                (2, uniform),
                (3, lumen_gpu::Res::Tex(tex)),
            ],
            params::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}
