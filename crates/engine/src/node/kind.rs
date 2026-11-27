#[cfg(feature = "3d")]
use super::scene;
#[cfg(feature = "metadata")]
use super::schema::NodeSchema;
use super::{
    Deferred, JsonNode, Node, PropertyEval, PropertyRead, compositing, ids::NodeId, media_output,
    processing, source, vector,
};
use crate::{
    error::{DocumentError, GraphValidationError},
    expr::Expression,
    gpu::frame_graph::{FrameCx, Programs, Raster, RecordNode, Staticness},
};

/// Declares the node kinds. Each line names a variant and its type, which derives
/// `lumen_macros::Node` and implements `RecordNode`. The generated definitions list the kinds in
/// this order.
macro_rules! node_kinds {
    ($($(#[$meta:meta])* $variant:ident($node:ty),)+) => {
        #[derive(Debug, lumen_macros::StructuralHash)]
        #[allow(clippy::large_enum_variant)]
        pub enum NodeKind {
            $($(#[$meta])* $variant($node),)+
        }

        impl NodeKind {
            #[cfg(feature = "metadata")]
            pub fn schemas() -> Vec<super::NodeSchemaDef> {
                let mut schemas = Vec::new();
                $($(#[$meta])* schemas.push(<$node>::schema());)+
                schemas
            }

            /// The node's `type` in JSON, which errors name the node's kind by.
            pub(crate) fn kind_name(&self) -> &'static str {
                match self {
                    $($(#[$meta])* Self::$variant(_) => <$node as JsonNode>::KIND,)+
                }
            }

            /// Builds a node from the `type` and `params` of its JSON.
            pub(crate) fn from_json(
                kind: &str,
                id: NodeId,
                params: Option<&serde_json::Map<String, serde_json::Value>>,
            ) -> Result<Self, Vec<DocumentError>> {
                match kind {
                    $($(#[$meta])* <$node as JsonNode>::KIND => Ok(Self::$variant(<$node>::from_json(id, params)?)),)+
                    other => Err(vec![DocumentError::UnknownNodeType {
                        at: crate::error::At::node(id),
                        kind: other.to_string(),
                    }]),
                }
            }

            /// Wires `source` into the named input.
            pub(crate) fn set_input(
                &mut self,
                port: &str,
                source: super::OutputRef,
            ) -> Result<(), GraphValidationError> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.set_input_json(port, source),)+
                }
            }

            /// Sorts unique nodes by their [`NodeId`].
            pub(crate) fn sort_by_id(nodes: &mut [&Self]) {
                #[inline(never)]
                fn key(node: &&NodeKind) -> NodeId {
                    node.id()
                }
                nodes.sort_unstable_by_key(key);
            }

            /// Returns the canonical name when `kind` is one of the 3D node kinds omitted from
            /// builds without the `3d` feature.
            #[cfg(not(feature = "3d"))]
            pub(crate) fn unsupported_3d_kind(kind: &str) -> Option<&'static str> {
                const KINDS: &[&str] = &[
                    "material_3d",
                    "shape_3d",
                    "image_plane_3d",
                    "text_3d",
                    "extrude_3d",
                    "lathe_3d",
                    "model_3d",
                    "camera_3d",
                    "light_3d",
                    "merge_3d",
                    "instance_3d",
                    "particles_3d",
                    "render_3d",
                ];
                KINDS.iter().copied().find(|&name| name == kind)
            }
        }

        impl Node for NodeKind {
            fn id(&self) -> NodeId {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.id,)+
                }
            }

            fn input_port_defs(&self) -> &'static [super::InputPortDef] {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.input_port_defs(),)+
                }
            }

            fn output_kind(&self) -> super::PortKind {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.output_kind(),)+
                }
            }

            fn output_port_defs(&self) -> &'static [super::OutputPortDef] {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.output_port_defs(),)+
                }
            }

            fn input_ports(&self) -> Vec<Option<super::OutputRef>> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.input_ports(),)+
                }
            }

            fn for_each_expression(&self, f: &mut dyn FnMut(&Expression)) {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.for_each_expression(f),)+
                }
            }

            fn for_each_media_source<'a>(&'a self, f: &mut dyn FnMut(super::MediaSource<'a>)) {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.for_each_media_source(f),)+
                }
            }
        }

        impl PropertyEval for NodeKind {
            fn get_property(&self, id: &str) -> Option<PropertyRead<'_>> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.get_property(id),)+
                }
            }

            fn evaluate_property(
                &self,
                id: &str,
                ctx: &crate::expr::ExpressionContext<'_>,
            ) -> Option<crate::Result<serde_json::Value>> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.evaluate_property(id, ctx),)+
                }
            }
        }

        impl RecordNode for NodeKind {
            fn record(&self, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.record(cx),)+
                }
            }

            fn canonical_port(&self, port: super::OutputPort) -> super::OutputPort {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.canonical_port(port),)+
                }
            }

            fn record_output(
                &self,
                cx: &mut FrameCx<'_>,
                port: super::OutputPort,
            ) -> crate::Result<Raster> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.record_output(cx, port),)+
                }
            }

            fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.programs(programs),)+
                }
            }

            fn staticness(&self, reads_frame: bool) -> Staticness {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.staticness(reads_frame),)+
                }
            }

            fn analytic_shutter(&self) -> Option<&Deferred<f64>> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.analytic_shutter(),)+
                }
            }

            fn magnification(&self) -> f64 {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.magnification(),)+
                }
            }

            fn reads_reduced(&self) -> Option<crate::gpu::image_size::ReducedRead> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.reads_reduced(),)+
                }
            }

            fn literal_param_errors(&self) -> Vec<(&'static str, String)> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.literal_param_errors(),)+
                }
            }

            fn unwired_input_param_errors(&self) -> Vec<(&'static str, String)> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.unwired_input_param_errors(),)+
                }
            }

            fn literal_font_families(&self) -> Vec<(String, &str)> {
                match self {
                    $($(#[$meta])* Self::$variant(node) => node.literal_font_families(),)+
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
    Path2d(vector::drawing::Path2d),
    Shape2d(vector::drawing::Shape2d),
    Text2d(vector::drawing::Text2d),
    Offset2d(vector::drawing::Offset2d),
    Fill2d(vector::drawing::Fill2d),
    Stroke2d(vector::drawing::Stroke2d),
    Shadow2d(vector::drawing::Shadow2d),
    Merge2d(vector::drawing::Merge2d),
    Instance2d(vector::drawing::Instance2d),
    Render2d(vector::drawing::Render2d),
    Boolean(compositing::boolean::Boolean),
    Merge(compositing::merge::Merge),
    RasterMultiMerge(compositing::raster_multimerge::RasterMultiMerge),
    Switch(compositing::switch::Switch),
    Clip(compositing::clip::Clip),
    Opacity(processing::opacity::Opacity),
    Blur(processing::blur::Blur),
    Channels(processing::channels::Channels),
    ChromaKey(processing::chroma_key::ChromaKey),
    ColorCorrect(processing::color_correct::ColorCorrect),
    ColorGrade(processing::color_grade::ColorGrade),
    CornerPin(processing::corner_pin::CornerPin),
    Exposure(processing::exposure::Exposure),
    FilmGrain(processing::film_grain::FilmGrain),
    Halftone(processing::halftone::Halftone),
    HueSaturation(processing::hue_saturation::HueSaturation),
    Levels(processing::levels::Levels),
    OrderedDither(processing::ordered_dither::OrderedDither),
    Posterize(processing::posterize::Posterize),
    PixelSort(processing::pixel_sort::PixelSort),
    TimeRemap(processing::time_remap::TimeRemap),
    Transform(processing::transform::Transform),
    MotionBlur(processing::motion_blur::MotionBlur),
    Crop(processing::crop::Crop),
    Resize(processing::resize::Resize),
    Shadow(processing::shadow::Shadow),
    Glow(processing::glow::Glow),
    Sharpen(processing::sharpen::Sharpen),
    DirectionalBlur(processing::directional_blur::DirectionalBlur),
    RadialBlur(processing::directional_blur::RadialBlur),
    ZoomBlur(processing::directional_blur::ZoomBlur),
    DisplacementMap(processing::displacement_map::DisplacementMap),
    LensDistortion(processing::lens_distortion::LensDistortion),
    LensBlur(processing::lens_blur::LensBlur),
    ErodeDilate(processing::erode_dilate::ErodeDilate),
    WgslShader(processing::wgsl_shader::WgslShader),
    MediaOutput(media_output::MediaOutput),
    #[cfg(feature = "3d")]
    Material3d(scene::Material3d),
    #[cfg(feature = "3d")]
    Shape3d(scene::Shape3d),
    #[cfg(feature = "3d")]
    ImagePlane3d(scene::ImagePlane3d),
    #[cfg(feature = "3d")]
    Text3d(scene::Text3d),
    #[cfg(feature = "3d")]
    Extrude3d(scene::Extrude3d),
    #[cfg(feature = "3d")]
    Lathe3d(scene::Lathe3d),
    #[cfg(feature = "3d")]
    Model3d(scene::Model3d),
    #[cfg(feature = "3d")]
    Camera3d(scene::Camera3d),
    #[cfg(feature = "3d")]
    Light3d(scene::Light3d),
    #[cfg(feature = "3d")]
    Merge3d(scene::Merge3d),
    #[cfg(feature = "3d")]
    Instance3d(scene::Instance3d),
    #[cfg(feature = "3d")]
    Particles3d(scene::Particles3d),
    #[cfg(feature = "3d")]
    Render3d(scene::Render3d),
}
