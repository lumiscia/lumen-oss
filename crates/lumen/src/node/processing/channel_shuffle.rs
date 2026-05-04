use crate::node::{NodeId, NodeProperty, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, FrameBinding, GpuCompileNode, GpuFrameBindNode,
    RasterHandle, compiler,
};

pub(crate) const SHADER: &str = include_str!("channel_shuffle.wgsl");

#[derive(Debug, Clone, lumen_macros::Node)]
#[node(
    kind = "channel_shuffle",
    label = "Channel Shuffle",
    description = "Remaps source raster color channels.",
    category = "processing"
)]
pub struct ChannelShuffle {
    pub id: NodeId,
    #[property(kind = "string")]
    pub red: NodeProperty,
    #[property(kind = "string")]
    pub green: NodeProperty,
    #[property(kind = "string")]
    pub blue: NodeProperty,
    #[property(kind = "string")]
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
        ctx.push_frame_binding(FrameBinding::ChannelShuffle {
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

impl GpuFrameBindNode for ChannelShuffle {
    fn bind_gpu_frame(
        &self,
        ctx: &FrameBindContext<'_>,
        binding: &FrameBinding,
        bound: &mut BoundFrame,
    ) -> crate::Result<()> {
        let FrameBinding::ChannelShuffle {
            node_id,
            red,
            green,
            blue,
            alpha,
            buffer,
        } = binding
        else {
            return Ok(());
        };
        let selectors = [
            compiler::channel_selector(
                *node_id,
                "red",
                &red.resolve_string(*node_id, "red", &ctx.expr_context(*node_id, "red"))?,
            )?,
            compiler::channel_selector(
                *node_id,
                "green",
                &green.resolve_string(*node_id, "green", &ctx.expr_context(*node_id, "green"))?,
            )?,
            compiler::channel_selector(
                *node_id,
                "blue",
                &blue.resolve_string(*node_id, "blue", &ctx.expr_context(*node_id, "blue"))?,
            )?,
            compiler::channel_selector(
                *node_id,
                "alpha",
                &alpha.resolve_string(*node_id, "alpha", &ctx.expr_context(*node_id, "alpha"))?,
            )?,
        ];
        let params = compiler::ChannelShuffleParams {
            selector_indices: selectors.map(|selector| selector.index),
            selector_values: selectors.map(|selector| selector.value),
        };
        bound.write_buffer(*buffer, 0, bytemuck::bytes_of(&params));
        Ok(())
    }
}
