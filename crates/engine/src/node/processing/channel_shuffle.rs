use crate::node::{NodeId, NodeParamEvalContext, NodeParams, PortRef};

use crate::gpu::{
    BoundFrame, CompiledOutput, FrameBindContext, GpuCompileNode, GpuCompiledNode, RasterHandle,
    compiler,
};

pub(crate) const SHADER: &str = include_str!("channel_shuffle.wgsl");

#[derive(
    Debug, Clone, Copy, PartialEq, Eq, Default, lumen_macros::NodeEnum, lumen_macros::Delegate,
)]
#[repr(i64)]
#[delegate(kind = "enum")]
pub enum ChannelSelector {
    #[default]
    Red = 0,
    Green = 1,
    Blue = 2,
    Alpha = 3,
    Zero = 4,
    One = 5,
}

impl ChannelSelector {
    fn as_spec(self) -> &'static str {
        match self {
            Self::Red => "red",
            Self::Green => "green",
            Self::Blue => "blue",
            Self::Alpha => "alpha",
            Self::Zero => "zero",
            Self::One => "one",
        }
    }
}

/// Remaps source raster color channels.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ChannelShuffleParams {
    /// Source channel mapped into the red output channel.
    #[meta(kind = "enum", enum_type = ChannelSelector)]
    pub red: ChannelSelector,
    /// Source channel mapped into the green output channel.
    #[meta(kind = "enum", enum_type = ChannelSelector)]
    pub green: ChannelSelector,
    /// Source channel mapped into the blue output channel.
    #[meta(kind = "enum", enum_type = ChannelSelector)]
    pub blue: ChannelSelector,
    /// Source channel mapped into the alpha output channel.
    #[meta(kind = "enum", enum_type = ChannelSelector)]
    pub alpha: ChannelSelector,
}

impl Default for ChannelShuffleParams {
    fn default() -> Self {
        Self {
            red: ChannelSelector::Red,
            green: ChannelSelector::Green,
            blue: ChannelSelector::Blue,
            alpha: ChannelSelector::Alpha,
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
struct CompiledChannelShuffle {
    node_id: NodeId,
    params: ChannelShuffleParamsDelegate,
    buffer: lumen_gpu::BufferId,
}

impl GpuCompiledNode for CompiledChannelShuffle {
    fn node_id(&self) -> NodeId {
        self.node_id
    }

    fn bind(&self, ctx: &FrameBindContext<'_>, bound: &mut BoundFrame) -> crate::Result<()> {
        let params = self.params.eval(&NodeParamEvalContext {
            node_id: self.node_id,
            expr: &ctx.expr_context(self.node_id, "params"),
        })?;
        let selectors = [
            compiler::channel_selector(self.node_id, "red", params.red.as_spec())?,
            compiler::channel_selector(self.node_id, "green", params.green.as_spec())?,
            compiler::channel_selector(self.node_id, "blue", params.blue.as_spec())?,
            compiler::channel_selector(self.node_id, "alpha", params.alpha.as_spec())?,
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
        ctx.register_compiled_node(CompiledChannelShuffle {
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
