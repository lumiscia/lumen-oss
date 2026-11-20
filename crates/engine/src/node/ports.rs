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

/// An output a node kind names beside its default `output`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
}

/// The name of every node's default output.
pub const DEFAULT_OUTPUT: &str = "output";

/// One of a node's outputs: its default `output`, or one of those its kind names.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, lumen_macros::StructuralHash,
)]
pub struct OutputPort(u8);

impl OutputPort {
    pub const DEFAULT: Self = Self(0);
    /// The most outputs a node kind may name.
    pub const MAX_NAMED: usize = 31;

    /// The output at `index` of those the node kind names.
    pub const fn named(index: usize) -> Self {
        assert!(index < Self::MAX_NAMED);
        Self(index as u8 + 1)
    }

    /// The index of the output among those the node kind names, or `None` for `output`.
    pub const fn index(self) -> Option<usize> {
        match self.0 {
            0 => None,
            port => Some(port as usize - 1),
        }
    }
}

/// What an input reads: one output of a node.
#[derive(
    Debug, Clone, Copy, Default, PartialEq, Eq, Hash, PartialOrd, Ord, lumen_macros::StructuralHash,
)]
pub struct OutputRef {
    pub node: NodeId,
    pub port: OutputPort,
}

impl OutputRef {
    pub const fn new(node: NodeId, port: OutputPort) -> Self {
        Self { node, port }
    }
}

impl From<NodeId> for OutputRef {
    /// The default output of `node`.
    fn from(node: NodeId) -> Self {
        Self::new(node, OutputPort::DEFAULT)
    }
}

/// A set of one node's outputs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct OutputSet(u32);

impl OutputSet {
    pub fn insert(&mut self, port: OutputPort) {
        self.0 |= 1 << port.0;
    }

    pub fn contains(self, port: OutputPort) -> bool {
        self.0 & (1 << port.0) != 0
    }

    pub fn iter(self) -> impl Iterator<Item = OutputPort> {
        (0..=OutputPort::MAX_NAMED as u8)
            .map(OutputPort)
            .filter(move |&port| self.contains(port))
    }
}

impl FromIterator<OutputPort> for OutputSet {
    fn from_iter<I: IntoIterator<Item = OutputPort>>(ports: I) -> Self {
        let mut set = Self::default();
        for port in ports {
            set.insert(port);
        }
        set
    }
}

/// The output wired to the required input `port` of `node_id`.
pub(crate) fn required_input(
    node_id: NodeId,
    port: &str,
    input: Option<OutputRef>,
) -> crate::Result<OutputRef> {
    input.ok_or_else(|| {
        GraphValidationError::MissingRequiredInput {
            at: crate::error::At::node(node_id),
            port: port.to_string(),
        }
        .into()
    })
}
