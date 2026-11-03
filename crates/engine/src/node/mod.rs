//! Renderer-agnostic node schema and shared parameter types.
//!
//! Node structs stay intentionally small here: they describe graph shape and
//! animatable parameters. GPU lowering lives in `crate::gpu`.

mod clip_props;
mod deferred;
mod delegate;
mod ids;
mod kind;
pub(crate) mod motion;
mod param_type;
mod ports;
mod schema;
mod shader_uniforms;

pub mod compositing;
pub mod media_output;
pub mod processing;
pub mod source;
pub mod vector;

pub use clip_props::{ClipPropValue, ClipProps};
pub use deferred::Deferred;
pub use deferred::{ParamValue, truncated_int};
pub use delegate::{
    DelegateEvalContext, DelegateEvaluable, Delegated, NodeParams, Override, PropertyRead,
    field_path, nested_fields,
};
pub use ids::NodeId;
pub use kind::NodeKind;
pub use param_type::NodeParamType;
pub use ports::InputPortDef;
pub(crate) use ports::required_input;
pub use schema::JsonNode;
pub use schema::{EnumDef, EnumOptionDef, NodeEnum};
pub use schema::{NodeCategory, NodeSchemaDef, PropertyDef, PropertyKind};
#[cfg(feature = "metadata")]
pub use schema::{NodeSchema, PropertyConstraints};
pub use shader_uniforms::{ShaderUniformValue, ShaderUniforms};

pub trait Node: Send + Sync {
    fn id(&self) -> NodeId;
    fn input_port_defs(&self) -> &'static [InputPortDef];
    /// The node wired to each input, in the order of `input_port_defs`, with a variadic input
    /// expanded in place. `None` is an unconnected input.
    fn input_ports(&self) -> Vec<Option<NodeId>>;
}

pub trait PropertyEval {
    fn get_property(&self, id: &str) -> Option<PropertyRead<'_>>;
}
