use std::fmt;

use super::ids::NodeId;
use crate::error::GraphValidationError;

/// What a node outputs and an input takes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortKind {
    /// A raster image.
    Raster,
    /// A 3D scene of objects, lights and a camera.
    Scene,
    /// A 2D outline without paint, which a fill or stroke styles.
    Geometry,
    /// Styled 2D drawables in order, the first lowest, which a `render_2d` rasterizes.
    Drawing,
    /// How a 3D object's surface responds to light, from a `material_3d`.
    Material,
}

impl PortKind {
    pub const ALL: [Self; 5] = [
        Self::Raster,
        Self::Scene,
        Self::Geometry,
        Self::Drawing,
        Self::Material,
    ];

    /// The kind's name in the generated definitions.
    pub fn name(self) -> &'static str {
        match self {
            Self::Raster => "raster",
            Self::Scene => "scene",
            Self::Geometry => "geometry",
            Self::Drawing => "drawing",
            Self::Material => "material",
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
            at: crate::error::At::node(node_id),
            port: port.to_string(),
        }
        .into()
    })
}
