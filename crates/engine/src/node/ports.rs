use super::ids::NodeId;
use crate::error::GraphValidationError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPortDef {
    pub name: &'static str,
    pub optional: bool,
    pub variadic: bool,
}

/// The node wired to the required input `port` of `node_id`.
pub(crate) fn required_input(
    node_id: NodeId,
    port: &str,
    input: Option<NodeId>,
) -> crate::Result<NodeId> {
    input.ok_or_else(|| {
        GraphValidationError::MissingRequiredInput {
            node_id,
            port: port.to_string(),
        }
        .into()
    })
}
