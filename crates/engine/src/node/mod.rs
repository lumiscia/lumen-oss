//! Renderer-agnostic node schema and shared parameter types.
//!
//! Node structs stay intentionally small here: they describe graph shape and
//! animatable parameters. GPU lowering lives in `crate::gpu`.

mod deferred;
mod delegate;
mod ids;
mod kind;
mod param_type;
mod ports;
mod property;
mod schema;

pub mod compositing;
pub mod media_output;
pub mod processing;
pub mod source;
pub mod vector;

pub use deferred::Deferred;
#[cfg(feature = "json")]
pub use deferred::DeferredJsonValue;
pub use deferred::DeferredValue;
pub use delegate::{
    DelegateEvalContext, DelegateEvaluable, DelegateValue, DelegateVec, Delegated,
    NodeParamEvalContext, NodeParams,
};
pub use ids::{NodeId, TrackId};
pub use kind::NodeKind;
pub use param_type::NodeParamType;
pub use ports::{InputPortDef, OutputPortDef, PortKind, PortRef, SINGLE_RASTER_OUTPUT};
pub use property::{PropertyExpression, PropertyValue};
#[cfg(feature = "json")]
pub use schema::JsonNode;
#[cfg(any(feature = "json", feature = "metadata"))]
pub use schema::{EnumDef, EnumOptionDef, NodeEnum};
pub use schema::{NodeCategory, NodeSchemaDef, PropertyDef, PropertyKind};
#[cfg(feature = "metadata")]
pub use schema::{NodeSchema, PropertyConstraints};

pub trait Node: Send + Sync {
    fn id(&self) -> NodeId;
    fn input_port_defs(&self) -> &'static [InputPortDef];
    fn input_ports(&self) -> Vec<PortRef>;
    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        SINGLE_RASTER_OUTPUT
    }
}

pub trait PropertyEval {
    fn get_property(&self, id: &str) -> crate::Result<Option<PropertyExpression>>;
}
