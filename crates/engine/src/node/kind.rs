#[cfg(feature = "metadata")]
use super::schema::NodeSchema;
use super::{
    JsonNode, Node, PropertyEval, PropertyRead, compositing, ids::NodeId, media_output, processing,
    source, vector,
};
use crate::{
    error::GraphValidationError,
    gpu::{CompileContext, GpuCompileNode, RasterHandle},
};

/// Declares the node kinds. Each line names a variant and its type, which derives
/// `lumen_macros::Node` and implements `GpuCompileNode`. The generated definitions list the
/// kinds in this order.
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

            /// Builds a node from the `type` and `params` of its JSON.
            pub(crate) fn from_json(
                kind: &str,
                id: NodeId,
                params: Option<&serde_json::Map<String, serde_json::Value>>,
            ) -> anyhow::Result<Self> {
                match kind {
                    $(<$node as JsonNode>::KIND => Ok(Self::$variant(<$node>::from_json(id, params)?)),)+
                    other => anyhow::bail!("unknown or unsupported node type `{other}`"),
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

            fn input_ports(&self) -> Vec<Option<NodeId>> {
                match self {
                    $(Self::$variant(node) => node.input_ports(),)+
                }
            }

            fn has_expressions(&self) -> bool {
                match self {
                    $(Self::$variant(node) => node.has_expressions(),)+
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

        impl GpuCompileNode for NodeKind {
            fn compile_gpu(&self, ctx: &mut CompileContext<'_>) -> crate::Result<RasterHandle> {
                match self {
                    $(Self::$variant(node) => node.compile_gpu(ctx),)+
                }
            }
        }
    };
}

node_kinds! {
    MediaIn(source::media_in::MediaIn),
    Background(source::background::Background),
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
    Crop(processing::crop::Crop),
    Resize(processing::resize::Resize),
    Shadow(processing::shadow::Shadow),
    WgslShader(processing::wgsl_shader::WgslShader),
    MediaOutput(media_output::MediaOutput),
}
