use std::fmt;

use super::ids::NodeId;
use crate::error::GraphValidationError;

/// What a node outputs and an input takes: a raster image, or a scene of objects, lights and a
/// camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    Raster,
    Scene,
}

impl PortKind {
    pub const ALL: [Self; 2] = [Self::Raster, Self::Scene];

    /// The kind's name in the generated definitions.
    pub fn name(self) -> &'static str {
        match self {
            Self::Raster => "raster",
            Self::Scene => "scene",
        }
    }
}

impl fmt::Display for PortKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "a {}", self.name())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
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
