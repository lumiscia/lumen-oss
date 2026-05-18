use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuFrameBinding, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("channel_shuffle.wgsl");

/// Remaps source raster color channels.
#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "channel_shuffle",
    name = "Channel Shuffle",
    category = "processing"
)]
pub struct ChannelShuffle {
    pub id: NodeId,
    /// Source channel mapped into the red output channel.
    #[property(kind = "string", format = "channel_selector")]
    pub red: NodeProperty,
    /// Source channel mapped into the green output channel.
    #[property(kind = "string", format = "channel_selector")]
    pub green: NodeProperty,
    /// Source channel mapped into the blue output channel.
    #[property(kind = "string", format = "channel_selector")]
    pub blue: NodeProperty,
    /// Source channel mapped into the alpha output channel.
    #[property(kind = "string", format = "channel_selector")]
    pub alpha: NodeProperty,
    #[input()]
    pub source: PortRef,
}

impl Default for ChannelShuffle {
    fn default() -> Self {
        Self {
            id: NodeId::new(0),
            red: NodeProperty::String("red".to_string()),
            green: NodeProperty::String("green".to_string()),
            blue: NodeProperty::String("blue".to_string()),
            alpha: NodeProperty::String("alpha".to_string()),
            source: PortRef::empty(),
        }
    }
}

#[derive(Debug, Clone)]
struct ChannelShuffleFrameBinding {
    node_id: NodeId,
    red: NodeProperty,
    green: NodeProperty,
    blue: NodeProperty,
    alpha: NodeProperty,
    buffer: lumen_gpu::BufferId,
}

impl GpuFrameBinding for ChannelShuffleFrameBinding {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let selectors = [
            compiler::channel_selector(
                self.node_id,
                "red",
                &self.red.resolve_string(
                    self.node_id,
                    "red",
                    &ctx.expr_context(self.node_id, "red"),
                )?,
            )?,
            compiler::channel_selector(
                self.node_id,
                "green",
                &self.green.resolve_string(
                    self.node_id,
                    "green",
                    &ctx.expr_context(self.node_id, "green"),
                )?,
            )?,
            compiler::channel_selector(
                self.node_id,
                "blue",
                &self.blue.resolve_string(
                    self.node_id,
                    "blue",
                    &ctx.expr_context(self.node_id, "blue"),
                )?,
            )?,
            compiler::channel_selector(
                self.node_id,
                "alpha",
                &self.alpha.resolve_string(
                    self.node_id,
                    "alpha",
                    &ctx.expr_context(self.node_id, "alpha"),
                )?,
            )?,
        ];
        let params = compiler::ChannelShuffleParams {
            selector_indices: selectors.map(|selector| selector.index),
            selector_values: selectors.map(|selector| selector.value),
        };
        bound.write_buffer(self.buffer, 0, bytemuck::bytes_of(&params));
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
            red: self.red.clone(),
            green: self.green.clone(),
            blue: self.blue.clone(),
            alpha: self.alpha.clone(),
            buffer: params,
        });
        Ok(CompiledOutput::Raster(RasterHandle {
            texture,
            domain: source.domain,
            metadata: source.metadata,
        }))
    }
}
