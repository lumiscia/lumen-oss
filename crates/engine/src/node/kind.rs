#[cfg(feature = "metadata")]
use super::schema::NodeSchema;
use super::{
    JsonNode, Node, PropertyEval, PropertyRead, compositing, ids::NodeId, media_output, processing,
    scene, source, vector,
};
use crate::{
    error::{DocumentError, GraphValidationError},
    expr::Expression,
};

/// Declares the node kinds. Each line names a variant and its type, which derives
/// `lumen_macros::Node`. The generated definitions list the kinds in this order.
macro_rules! node_kinds {
    ($($variant:ident($node:ty),)+) => {
        #[derive(Debug, lumen_macros::StructuralHash)]
        #[allow(clippy::large_enum_variant)]
        pub enum NodeKind {
            $($variant($node),)+
        }

        impl NodeKind {
            #[cfg(feature = "metadata")]
            pub fn schemas() -> Vec<super::NodeSchemaDef> {
                vec![$(<$node>::schema(),)+]
            }

            /// The node's `type` in JSON, which errors name the node's kind by.
            pub(crate) fn kind_name(&self) -> &'static str {
                match self {
                    $(Self::$variant(_) => <$node as JsonNode>::KIND,)+
                }
            }

            /// Builds a node from the `type` and `params` of its JSON.
            pub(crate) fn from_json(
                kind: &str,
                id: NodeId,
                params: Option<&serde_json::Map<String, serde_json::Value>>,
            ) -> Result<Self, Vec<DocumentError>> {
                match kind {
                    $(<$node as JsonNode>::KIND => Ok(Self::$variant(<$node>::from_json(id, params)?)),)+
                    other => Err(vec![DocumentError::UnknownNodeType {
                        node_id: id,
                        kind: other.to_string(),
                    }]),
                }
            }

            /// Wires `source` into the named input.
            pub(crate) fn set_input(
                &mut self,
                port: &str,
                source: NodeId,
            ) -> Result<(), GraphValidationError> {
                match self {
                    $(Self::$variant(node) => node.set_input_json(port, source),)+
                }
            }
        }

        impl Node for NodeKind {
            fn id(&self) -> NodeId {
                match self {
                    $(Self::$variant(node) => node.id,)+
                }
            }

            fn input_port_defs(&self) -> &'static [super::InputPortDef] {
                match self {
                    $(Self::$variant(node) => node.input_port_defs(),)+
                }
            }

            fn output_kind(&self) -> super::PortKind {
                match self {
                    $(Self::$variant(node) => node.output_kind(),)+
                }
            }

            fn input_ports(&self) -> Vec<Option<NodeId>> {
                match self {
                    $(Self::$variant(node) => node.input_ports(),)+
                }
            }

            fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
                match self {
                    $(Self::$variant(node) => node.for_each_expression(f),)+
                }
            }
        }

        impl PropertyEval for NodeKind {
            fn get_property(&self, id: &str) -> Option<PropertyRead<'_>> {
                match self {
                    $(Self::$variant(node) => node.get_property(id),)+
                }
            }
        }
    };
}

node_kinds! {
    MediaIn(source::media_in::MediaIn),
    Background(source::background::Background),
    FractalNoise(source::fractal_noise::FractalNoise),
    Text(source::text::Text),
    Path(vector::path::Path),
    Shape(vector::shape::Shape),
    Boolean(compositing::boolean::Boolean),
    Merge(compositing::merge::Merge),
    RasterMultiMerge(compositing::raster_multimerge::RasterMultiMerge),
    Switch(compositing::switch::Switch),
    Clip(compositing::clip::Clip),
    Opacity(processing::opacity::Opacity),
    Blur(processing::blur::Blur),
    ColorGrade(processing::color_grade::ColorGrade),
    Exposure(processing::exposure::Exposure),
    HueSaturation(processing::hue_saturation::HueSaturation),
    Levels(processing::levels::Levels),
    TimeRemap(processing::time_remap::TimeRemap),
    Transform(processing::transform::Transform),
    MotionBlur(processing::motion_blur::MotionBlur),
    Crop(processing::crop::Crop),
    Resize(processing::resize::Resize),
    Shadow(processing::shadow::Shadow),
    Glow(processing::glow::Glow),
    DirectionalBlur(processing::directional_blur::DirectionalBlur),
    RadialBlur(processing::directional_blur::RadialBlur),
    ZoomBlur(processing::directional_blur::ZoomBlur),
    DisplacementMap(processing::displacement_map::DisplacementMap),
    WgslShader(processing::wgsl_shader::WgslShader),
    MediaOutput(media_output::MediaOutput),
    Shape3d(scene::Shape3d),
    ImagePlane3d(scene::ImagePlane3d),
    Camera3d(scene::Camera3d),
    Light3d(scene::Light3d),
    Merge3d(scene::Merge3d),
    Render3d(scene::Render3d),
}
