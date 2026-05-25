use super::schema::PropertyKind;

#[cfg(any(feature = "json", feature = "metadata"))]
use super::schema::EnumDef;

pub trait NodeParamType {
    fn property_kind() -> PropertyKind;
    #[cfg(any(feature = "json", feature = "metadata"))]
    fn enum_def() -> Option<&'static EnumDef> {
        None
    }
}

impl NodeParamType for f64 {
    fn property_kind() -> PropertyKind {
        PropertyKind::Float
    }
}

impl NodeParamType for f32 {
    fn property_kind() -> PropertyKind {
        PropertyKind::Float
    }
}

impl NodeParamType for u8 {
    fn property_kind() -> PropertyKind {
        PropertyKind::Int
    }
}

impl NodeParamType for u32 {
    fn property_kind() -> PropertyKind {
        PropertyKind::Int
    }
}

impl NodeParamType for i64 {
    fn property_kind() -> PropertyKind {
        PropertyKind::Int
    }
}

impl NodeParamType for bool {
    fn property_kind() -> PropertyKind {
        PropertyKind::Bool
    }
}

impl NodeParamType for String {
    fn property_kind() -> PropertyKind {
        PropertyKind::String
    }
}

impl NodeParamType for [u8; 4] {
    fn property_kind() -> PropertyKind {
        PropertyKind::Color
    }
}

impl NodeParamType for (f64, f64) {
    fn property_kind() -> PropertyKind {
        PropertyKind::Vec2
    }
}

impl NodeParamType for [f32; 2] {
    fn property_kind() -> PropertyKind {
        PropertyKind::Vec2
    }
}

impl<T> NodeParamType for Vec<T> {
    fn property_kind() -> PropertyKind {
        PropertyKind::String
    }
}
