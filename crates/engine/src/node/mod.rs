//! Renderer-agnostic node schema and shared parameter types.
//!
//! Node structs stay intentionally small here: they describe graph shape and
//! animatable parameters. GPU lowering lives in `crate::gpu`.

mod clip_props;
mod deferred;
mod delegate;
mod ids;
mod instance_count;
mod kind;
mod media_source;
pub(crate) mod motion;
mod param_type;
mod ports;
mod schema;
mod shader_uniforms;

pub mod compositing;
pub mod media_output;
pub mod processing;
#[cfg(feature = "3d")]
pub mod scene;
pub mod source;
pub mod vector;

use crate::expr::Expression;

pub use clip_props::{ClipPropValue, ClipProps};
pub use deferred::Deferred;
pub use deferred::{ParamValue, truncated_int};
pub use delegate::{
    DelegateEvalContext, DelegateEvaluable, Delegated, NodeParams, Override, PixelScale,
    PropertyRead,
};
pub use ids::NodeId;
pub use instance_count::{InstanceCount, MAX_INSTANCES};
pub use kind::NodeKind;
pub use media_source::{MediaParam, MediaSource, MediaValue};
pub use param_type::NodeParamType;
pub(crate) use ports::required_input;
pub use ports::{
    DEFAULT_OUTPUT, InputPortDef, OutputPort, OutputPortDef, OutputRef, OutputSet, PortKind,
};
pub use schema::JsonNode;
pub use schema::{EnumDef, EnumOptionDef, NodeEnum};
pub use schema::{NodeCategory, NodeSchemaDef, ParamTypeDef, PropertyDef, PropertyKind};
#[cfg(feature = "metadata")]
pub use schema::{NodeSchema, PropertyConstraints};
pub use shader_uniforms::{ShaderUniformValue, ShaderUniforms};

// The derive macros' output names these, so they are public for it and not for hosts.
#[doc(hidden)]
pub use deferred::enum_value;
#[doc(hidden)]
pub use delegate::{assert_unique_params, field_path, nested_fields};
#[doc(hidden)]
pub use media_source::{MediaSourceField, selected_media_kind};

pub trait Node: Send + Sync {
    fn id(&self) -> NodeId;
    fn input_port_defs(&self) -> &'static [InputPortDef];
    /// What the node's default `output` outputs.
    fn output_kind(&self) -> PortKind;
    /// The outputs the node's kind names beside `output`, in the order their ports count.
    fn output_port_defs(&self) -> &'static [OutputPortDef];
    /// The output wired to each input, in the order of `input_port_defs`, with a variadic input
    /// expanded in place. `None` is an unconnected input.
    fn input_ports(&self) -> Vec<Option<OutputRef>>;
    /// Calls `f` with every expression in the node's params.
    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression));
    /// Calls `f` with every param the node declares with `#[media(...)]`.
    fn for_each_media_source<'a>(&'a self, f: &mut dyn FnMut(MediaSource<'a>));

    /// What output `port` outputs, or `None` when the node has no such output.
    fn port_kind(&self, port: OutputPort) -> Option<PortKind> {
        match port.index() {
            None => Some(self.output_kind()),
            Some(index) => self.output_port_defs().get(index).map(|def| def.kind),
        }
    }

    /// The output named `name`: [`DEFAULT_OUTPUT`] or one of those the node's kind names.
    fn output_port(&self, name: &str) -> Option<OutputPort> {
        if name == DEFAULT_OUTPUT {
            return Some(OutputPort::DEFAULT);
        }
        let index = self
            .output_port_defs()
            .iter()
            .position(|def| def.name == name)?;
        Some(OutputPort::named(index))
    }

    /// The definition of output `port`, which the default `output` has none of.
    fn output_port_def(&self, port: OutputPort) -> Option<&'static OutputPortDef> {
        port.index()
            .and_then(|index| self.output_port_defs().get(index))
    }

    /// Whether any of the node's params is an expression.
    fn has_expressions(&self) -> bool {
        let mut any = false;
        self.for_each_expression(&mut |_| any = true);
        any
    }

    /// Whether any of the node's params may give a different value at another frame, as
    /// [`Expression::is_frame_dependent`] finds.
    fn is_frame_dependent(&self) -> bool {
        let mut dependent = false;
        self.for_each_expression(&mut |expression| {
            dependent |= expression.is_frame_dependent();
        });
        dependent
    }
}

pub trait PropertyEval {
    fn get_property(&self, id: &str) -> Option<PropertyRead<'_>>;
    /// The param `id` as it evaluates at `ctx`, in the JSON that sets it, or `None` when the node
    /// has no such param. A param with no value evaluates to `null`.
    fn evaluate_property(
        &self,
        id: &str,
        ctx: &crate::expr::ExpressionContext<'_>,
    ) -> Option<crate::Result<serde_json::Value>>;
}
