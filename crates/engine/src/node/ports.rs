use super::ids::NodeId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
    pub optional: bool,
    pub variadic: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputPortDef {
    pub name: &'static str,
    pub kind: PortKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PortKind {
    Raster = 0,
    Vector = 1,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct PortRef {
    pub id: NodeId,
    pub port: String,
}

impl PortRef {
    pub fn new(id: NodeId, port: String) -> Self {
        Self { id, port }
    }

    pub fn empty() -> Self {
        Self {
            id: NodeId::new(0),
            port: String::new(),
        }
    }

    /// Node ids start at 0, so only the port name marks an unconnected input.
    pub fn is_empty(&self) -> bool {
        self.port.is_empty()
    }
}

pub const SINGLE_RASTER_OUTPUT: &[OutputPortDef] = &[OutputPortDef {
    name: "output",
    kind: PortKind::Raster,
}];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn port_from_node_zero_is_connected() {
        assert!(PortRef::empty().is_empty());
        assert!(!PortRef::new(NodeId::new(0), "output".to_string()).is_empty());
    }
}
