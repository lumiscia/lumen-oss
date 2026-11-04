use crate::node::{DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams};

use crate::gpu::compiler::Slot;
use crate::gpu::{
    BoundFrame, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle, compiler,
};

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

#[derive(Debug, Clone)]
struct CompiledBoolean {
    node_id: NodeId,
    params: BooleanParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledBoolean {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&DelegateEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(),
        })?;
        let gpu_params = compiler::BooleanParams {
            values: [params.operation as u32 as f32, params.threshold as f32],
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}

impl GpuCompileNode for Boolean {
    fn compile_gpu(&self, ctx: &mut crate::gpu::CompileContext<'_>) -> crate::Result<RasterHandle> {
        let a = ctx.compile_input(self.id, "a", self.a)?;
        let b = ctx.compile_input(self.id, "b", self.b)?;
        let size = a.domain.storage_size;
        let texture = ctx.builder_mut().texture(
            Some(format!("boolean:{}:output", self.id.0)),
            lumen_gpu::TextureDesc::storage(size, lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
        );
        let params = ctx.builder_mut().buffer(
            Some(format!("boolean:{}:params", self.id.0)),
            lumen_gpu::BufferDesc::uniform(std::mem::size_of::<compiler::BooleanParams>() as u64),
        );
        let program = ctx.compute_program(
            "boolean",
            SHADER,
            &[Slot::Texture, Slot::Texture, Slot::Uniform, Slot::Output],
        );
        ctx.builder_mut().compute_pass(lumen_gpu::ComputePassDesc {
            label: Some(format!("boolean:{}:apply", self.id.0)),
            program,
            bindings: vec![
                lumen_gpu::Binding::sampled_texture(0, 0, a.texture),
                lumen_gpu::Binding::sampled_texture(0, 1, b.texture),
                lumen_gpu::Binding::uniform(0, 2, params),
                lumen_gpu::Binding::storage_texture(0, 3, texture),
            ],
            dispatch: compiler::dispatch_for(size),
        });
        ctx.register_compiled_node(CompiledBoolean {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });

        Ok(RasterHandle {
            texture,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(
        3,
        lumen_gpu::LayoutKind::StorageTexture(lumen_gpu::wgpu::TextureFormat::Rgba8Unorm),
    ),
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
        let gpu_params = compiler::BooleanParams {
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
            compiler::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster {
            tex,
            domain: lumen_gpu::TextureDomain::full_frame(size),
            bounds: None,
        })
    }
}
