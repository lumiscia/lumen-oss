use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("channel_shuffle.wgsl");

/// Remaps source raster color channels.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ChannelShuffleParams {
    /// Source channel mapped into the red output channel.
    #[meta(format = "channel_selector")]
    pub red: String,
    /// Source channel mapped into the green output channel.
    #[meta(format = "channel_selector")]
    pub green: String,
    /// Source channel mapped into the blue output channel.
    #[meta(format = "channel_selector")]
    pub blue: String,
    /// Source channel mapped into the alpha output channel.
    #[meta(format = "channel_selector")]
    pub alpha: String,
}

impl Default for ChannelShuffleParams {
    fn default() -> Self {
        Self {
            red: "red".to_string(),
            green: "green".to_string(),
            blue: "blue".to_string(),
            alpha: "alpha".to_string(),
        }
    }
}

/// Remaps source raster color channels.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "channel_shuffle",
    name = "Channel Shuffle",
    category = "processing"
)]
pub struct ChannelShuffle {
    pub id: NodeId,
    #[params]
    pub params: ChannelShuffleParamsDelegate,

    #[input()]
    pub source: PortRef,
}

impl Default for ChannelShuffle {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            params: ChannelShuffleParamsDelegate::default(),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct ChannelShuffleFrameBinding {
    node_id: NodeId,
    params: ChannelShuffleParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for ChannelShuffleFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let selectors = [
            compiler::channel_selector(self.node_id, "red", &params.red)?,
            compiler::channel_selector(self.node_id, "green", &params.green)?,
            compiler::channel_selector(self.node_id, "blue", &params.blue)?,
            compiler::channel_selector(self.node_id, "alpha", &params.alpha)?,
        ];
        let gpu_params = compiler::ChannelShuffleParams {
            selector_indices: selectors.map(|selector| selector.index),
            selector_values: selectors.map(|selector| selector.value),
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&gpu_params));
        Ok(())
    }
}

impl GpuCompileNode for ChannelShuffle {
    fn compile_gpu(
        &self,
        ctx: &mut crate::gpu::CompileContext<'_>,
        port: &PortRef,
    ) -> crate::Result<CompiledOutput> {
        let (source, texture, params) = ctx.compile_unary_filter(
            self.id,
            &self.source,
            port,
            "channel-shuffle",
            SHADER,
            std::mem::size_of::<compiler::ChannelShuffleParams>() as u64,
        )?;
        ctx.push_frame_binding(ChannelShuffleFrameBinding {
            node_id: self.id,
            params: self.params.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}
