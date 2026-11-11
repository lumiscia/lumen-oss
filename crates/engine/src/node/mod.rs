//! Renderer-agnostic node schema and shared parameter types.
//!
//! Node structs stay intentionally small here: they describe graph shape and
//! animatable parameters. GPU lowering lives in `crate::gpu`.

mod clip_props;
mod deferred;
mod delegate;
mod ids;
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
pub mod scene;
pub mod source;
pub mod vector;

use crate::expr::Expression;

pub use clip_props::{ClipPropValue, ClipProps};
pub use deferred::Deferred;
pub use deferred::{ParamValue, enum_value, truncated_int};
pub use delegate::{
    DelegateEvalContext, DelegateEvaluable, Delegated, NodeParams, Override, PropertyRead,
    assert_unique_params, field_path, nested_fields,
};
pub use ids::NodeId;
pub use kind::NodeKind;
pub use media_source::{
    MediaParam, MediaSource, MediaSourceField, MediaValue, selected_media_kind,
};
pub use param_type::NodeParamType;
pub(crate) use ports::required_input;
pub use ports::{InputPortDef, PortKind};
pub use schema::JsonNode;
pub use schema::{EnumDef, EnumOptionDef, NodeEnum};
pub use schema::{NodeCategory, NodeSchemaDef, PropertyDef, PropertyKind};
#[cfg(feature = "metadata")]
pub use schema::{NodeSchema, PropertyConstraints};
pub use shader_uniforms::{ShaderUniformValue, ShaderUniforms};

pub trait Node: Send + Sync {
    fn id(&self) -> NodeId;
    fn input_port_defs(&self) -> &'static [InputPortDef];
    /// What the node outputs.
    fn output_kind(&self) -> PortKind;
    /// The node wired to each input, in the order of `input_port_defs`, with a variadic input
    /// expanded in place. `None` is an unconnected input.
    fn input_ports(&self) -> Vec<Option<NodeId>>;
    /// Calls `f` with every expression in the node's params.
    fn for_each_expression(&self, f: &mut dyn FnMut(&Expression));
    /// Calls `f` with every param the node declares with `#[media(...)]`.
    fn for_each_media_source<'a>(&'a self, f: &mut dyn FnMut(MediaSource<'a>));

    /// Whether any of the node's params is an expression.
    fn has_expressions(&self) -> bool {
        let mut any = false;
        self.for_each_expression(&mut |_| any = true);
        any
    }
}

pub trait PropertyEval {
    fn get_property(&self, id: &str) -> Option<PropertyRead<'_>>;
}
