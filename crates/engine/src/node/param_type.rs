use super::schema::PropertyKind;

use super::schema::EnumDef;

pub trait NodeParamType {
    fn property_kind() -> PropertyKind;
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

impl NodeParamType for [f64; 3] {
    fn property_kind() -> PropertyKind {
        PropertyKind::Vec3
    }
}

impl NodeParamType for [f32; 3] {
    fn property_kind() -> PropertyKind {
        PropertyKind::Vec3
    }
}

impl NodeParamType for super::ShaderUniforms {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::shader_uniforms::SHADER_UNIFORMS)
    }
}

impl NodeParamType for super::ClipProps {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::clip_props::CLIP_PROPS)
    }
}

impl NodeParamType for super::vector::paint::Paint {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::vector::paint::PAINT)
    }
}

impl<T: NodeParamType> NodeParamType for Option<T> {
    fn property_kind() -> PropertyKind {
        T::property_kind()
    }
    fn enum_def() -> Option<&'static EnumDef> {
        T::enum_def()
    }
}

impl<T: NodeParamType> NodeParamType for super::Override<T> {
    fn property_kind() -> PropertyKind {
        T::property_kind()
    }
    fn enum_def() -> Option<&'static EnumDef> {
        T::enum_def()
    }
}

impl NodeParamType for super::source::text_content::TextContent {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::source::text_content::TEXT_CONTENT)
    }
}

impl NodeParamType for super::source::text_content::TextLine {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::source::text_content::TEXT_LINE)
    }
}

impl NodeParamType for super::source::text_content::TextHighlight {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::source::text_content::TEXT_HIGHLIGHT)
    }
}

impl NodeParamType for super::source::text_content::FontFeatures {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::source::text_content::FONT_FEATURES)
    }
}

impl NodeParamType for super::source::text_content::TextStroke {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::source::text_content::TEXT_STROKE)
    }
}

impl NodeParamType for super::vector::dash::DashPattern {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::vector::dash::DASH_PATTERN)
    }
}

impl NodeParamType for super::vector::drawing::Shadows {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::vector::drawing::SHADOWS)
    }
}

impl NodeParamType for super::scene::Region {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::scene::REGION)
    }
}

impl NodeParamType for super::vector::repeat::Repeat {
    fn property_kind() -> PropertyKind {
        PropertyKind::Custom(&super::vector::repeat::REPEAT)
    }
}

impl NodeParamType for super::vector::repeat::RepeatCount {
    fn property_kind() -> PropertyKind {
        PropertyKind::Int
    }
}

// Only nested delegates may hold `Vec` fields: `#[derive(Node)]` rejects params with
// `NodeParams::HAS_VEC_FIELDS` because the schema has no array kind.
impl<T> NodeParamType for Vec<T> {
    fn property_kind() -> PropertyKind {
        PropertyKind::String
    }
}
