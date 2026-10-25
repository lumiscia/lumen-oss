#[cfg(feature = "metadata")]
use super::schema::NodeSchema;
use super::{
    Node, OutputPortDef, PortRef, PropertyEval, PropertyExpression, compositing, ids::NodeId,
    media_output, ports::SINGLE_RASTER_OUTPUT, processing, source, vector,
};

#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum NodeKind {
    MediaIn(source::media_in::MediaIn),
    Background(source::background::Background),
    Text(source::text::Text),
    Path(vector::path::Path),
    Shape(vector::shape::Shape),
    Boolean(compositing::boolean::Boolean),
    Merge(compositing::merge::Merge),
    RasterMultiMerge(compositing::raster_multimerge::RasterMultiMerge),
    AlphaPremultiply(processing::alpha_premultiply::AlphaPremultiply),
    Blur(processing::blur::Blur),
    ChannelShuffle(processing::channel_shuffle::ChannelShuffle),
    ColorGrade(processing::color_grade::ColorGrade),
    Curves(processing::curves::Curves),
    Exposure(processing::exposure::Exposure),
    HueSaturation(processing::hue_saturation::HueSaturation),
    Levels(processing::levels::Levels),
    Memo(processing::memo::Memo),
    Opacity(processing::opacity::Opacity),
    TimeRemap(processing::time_remap::TimeRemap),
    Transform(processing::transform::Transform),
    Crop(processing::crop::Crop),
    Resize(processing::resize::Resize),
    Shadow(processing::shadow::Shadow),
    WgslShader(processing::wgsl_shader::WgslShader),
    Switch(compositing::switch::Switch),
    MediaOutput(media_output::MediaOutput),
}

impl NodeKind {
    pub fn id(&self) -> NodeId {
        match self {
            Self::MediaIn(node) => node.id,
            Self::Background(node) => node.id,
            Self::Text(node) => node.id,
            Self::Path(node) => node.id,
            Self::Shape(node) => node.id,
            Self::Boolean(node) => node.id,
            Self::Merge(node) => node.id,
            Self::RasterMultiMerge(node) => node.id,
            Self::AlphaPremultiply(node) => node.id,
            Self::Blur(node) => node.id,
            Self::ChannelShuffle(node) => node.id,
            Self::ColorGrade(node) => node.id,
            Self::Curves(node) => node.id,
            Self::Exposure(node) => node.id,
            Self::HueSaturation(node) => node.id,
            Self::Levels(node) => node.id,
            Self::Memo(node) => node.id,
            Self::Opacity(node) => node.id,
            Self::TimeRemap(node) => node.id,
            Self::Transform(node) => node.id,
            Self::Crop(node) => node.id,
            Self::Resize(node) => node.id,
            Self::Shadow(node) => node.id,
            Self::WgslShader(node) => node.id,
            Self::Switch(node) => node.id,
            Self::MediaOutput(node) => node.id,
        }
    }

    pub fn input_ports(&self) -> Vec<PortRef> {
        match self {
            Self::MediaIn(node) => node.input_ports(),
            Self::Background(node) => node.input_ports(),
            Self::Text(node) => node.input_ports(),
            Self::Path(node) => node.input_ports(),
            Self::Shape(node) => node.input_ports(),
            Self::Boolean(node) => node.input_ports(),
            Self::Merge(node) => node.input_ports(),
            Self::RasterMultiMerge(node) => node.input_ports(),
            Self::AlphaPremultiply(node) => node.input_ports(),
            Self::Blur(node) => node.input_ports(),
            Self::ChannelShuffle(node) => node.input_ports(),
            Self::ColorGrade(node) => node.input_ports(),
            Self::Curves(node) => node.input_ports(),
            Self::Exposure(node) => node.input_ports(),
            Self::HueSaturation(node) => node.input_ports(),
            Self::Levels(node) => node.input_ports(),
            Self::Memo(node) => node.input_ports(),
            Self::Opacity(node) => node.input_ports(),
            Self::TimeRemap(node) => node.input_ports(),
            Self::Transform(node) => node.input_ports(),
            Self::Crop(node) => node.input_ports(),
            Self::Resize(node) => node.input_ports(),
            Self::Shadow(node) => node.input_ports(),
            Self::WgslShader(node) => node.input_ports(),
            Self::Switch(node) => node.input_ports(),
            Self::MediaOutput(node) => node.input_ports(),
        }
    }

    pub fn as_property_eval(&self) -> &dyn PropertyEval {
        self
    }

    #[cfg(feature = "metadata")]
    pub fn schemas() -> Vec<super::NodeSchemaDef> {
        vec![
            source::media_in::MediaIn::schema(),
            source::background::Background::schema(),
            source::text::Text::schema(),
            vector::path::Path::schema(),
            vector::shape::Shape::schema(),
            compositing::boolean::Boolean::schema(),
            compositing::merge::Merge::schema(),
            compositing::raster_multimerge::RasterMultiMerge::schema(),
            compositing::switch::Switch::schema(),
            processing::memo::Memo::schema(),
            processing::opacity::Opacity::schema(),
            processing::alpha_premultiply::AlphaPremultiply::schema(),
            processing::blur::Blur::schema(),
            processing::channel_shuffle::ChannelShuffle::schema(),
            processing::color_grade::ColorGrade::schema(),
            processing::curves::Curves::schema(),
            processing::exposure::Exposure::schema(),
            processing::hue_saturation::HueSaturation::schema(),
            processing::levels::Levels::schema(),
            processing::time_remap::TimeRemap::schema(),
            processing::transform::Transform::schema(),
            processing::crop::Crop::schema(),
            processing::resize::Resize::schema(),
            processing::shadow::Shadow::schema(),
            processing::wgsl_shader::WgslShader::schema(),
            media_output::MediaOutput::schema(),
        ]
    }
}

impl Node for NodeKind {
    fn id(&self) -> NodeId {
        self.id()
    }

    fn input_port_defs(&self) -> &'static [super::InputPortDef] {
        match self {
            Self::MediaIn(node) => node.input_port_defs(),
            Self::Background(node) => node.input_port_defs(),
            Self::Text(node) => node.input_port_defs(),
            Self::Path(node) => node.input_port_defs(),
            Self::Shape(node) => node.input_port_defs(),
            Self::Boolean(node) => node.input_port_defs(),
            Self::Merge(node) => node.input_port_defs(),
            Self::RasterMultiMerge(node) => node.input_port_defs(),
            Self::AlphaPremultiply(node) => node.input_port_defs(),
            Self::Blur(node) => node.input_port_defs(),
            Self::ChannelShuffle(node) => node.input_port_defs(),
            Self::ColorGrade(node) => node.input_port_defs(),
            Self::Curves(node) => node.input_port_defs(),
            Self::Exposure(node) => node.input_port_defs(),
            Self::HueSaturation(node) => node.input_port_defs(),
            Self::Levels(node) => node.input_port_defs(),
            Self::Memo(node) => node.input_port_defs(),
            Self::Opacity(node) => node.input_port_defs(),
            Self::TimeRemap(node) => node.input_port_defs(),
            Self::Transform(node) => node.input_port_defs(),
            Self::Crop(node) => node.input_port_defs(),
            Self::Resize(node) => node.input_port_defs(),
            Self::Shadow(node) => node.input_port_defs(),
            Self::WgslShader(node) => node.input_port_defs(),
            Self::Switch(node) => node.input_port_defs(),
            Self::MediaOutput(node) => node.input_port_defs(),
        }
    }

    fn input_ports(&self) -> Vec<PortRef> {
        self.input_ports()
    }

    fn output_port_defs(&self) -> &'static [OutputPortDef] {
        SINGLE_RASTER_OUTPUT
    }
}

impl PropertyEval for NodeKind {
    fn get_property(&self, id: &str) -> crate::Result<Option<PropertyExpression>> {
        match self {
            Self::MediaIn(node) => node.get_property(id),
            Self::Background(node) => node.get_property(id),
            Self::Text(node) => node.get_property(id),
            Self::Path(node) => node.get_property(id),
            Self::Shape(node) => node.get_property(id),
            Self::Boolean(node) => node.get_property(id),
            Self::Merge(node) => node.get_property(id),
            Self::RasterMultiMerge(node) => node.get_property(id),
            Self::AlphaPremultiply(node) => node.get_property(id),
            Self::Blur(node) => node.get_property(id),
            Self::ChannelShuffle(node) => node.get_property(id),
            Self::ColorGrade(node) => node.get_property(id),
            Self::Curves(node) => node.get_property(id),
            Self::Exposure(node) => node.get_property(id),
            Self::HueSaturation(node) => node.get_property(id),
            Self::Levels(node) => node.get_property(id),
            Self::Memo(node) => node.get_property(id),
            Self::Opacity(node) => node.get_property(id),
            Self::TimeRemap(node) => node.get_property(id),
            Self::Transform(node) => node.get_property(id),
            Self::Crop(node) => node.get_property(id),
            Self::Resize(node) => node.get_property(id),
            Self::Shadow(node) => node.get_property(id),
            Self::WgslShader(node) => node.get_property(id),
            Self::Switch(node) => node.get_property(id),
            Self::MediaOutput(node) => node.get_property(id),
        }
    }
}
