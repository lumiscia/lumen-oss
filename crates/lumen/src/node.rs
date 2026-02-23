//! Node type system, ports, and evaluation contracts for graph execution.

use std::{collections::HashMap, fmt};

use crate::{
	error::{LumenError, PropertyError, RenderError},
	raster::RasterFrame,
	render::RenderContext,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct NodeId(pub u64);

impl NodeId {
	pub const fn new(value: u64) -> Self {
		Self(value)
	}
}

impl fmt::Display for NodeId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.0)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct TrackId(pub u64);

impl TrackId {
	pub const fn new(value: u64) -> Self {
		Self(value)
	}
}

impl fmt::Display for TrackId {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(f, "{}", self.0)
	}
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PortKind {
	RasterFrame = 0,
	Surface = 1,
	Vector = 2,
}

#[derive(Debug, Clone)]
pub enum PortValue {
	RasterFrame(RasterFrame),
	Vector(VectorData),
}

#[derive(Debug, Clone)]
pub enum VectorData {
	Empty,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InputPortDef {
	pub name: &'static str,
	pub kind: PortKind,
	pub optional: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputPortDef {
	pub name: &'static str,
	pub kind: PortKind,
}

#[derive(Debug, Clone, Default)]
pub struct NodeInputs {
	ports: HashMap<String, PortValue>,
}

impl NodeInputs {
	pub fn new() -> Self {
		Self::default()
	}

	pub fn insert(&mut self, name: impl Into<String>, value: PortValue) {
		self.ports.insert(name.into(), value);
	}

	pub fn get_raster(&self, name: &str) -> Result<&RasterFrame, LumenError> {
		match self.ports.get(name) {
			Some(PortValue::RasterFrame(frame)) => Ok(frame),
			Some(_) => Err(PropertyError::InvalidType {
				node_id: NodeId(0),
				property_path: name.to_string(),
				expected: "RasterFrame",
				actual: "non-raster",
			}
			.into()),
			None => Err(PropertyError::MissingProperty {
				node_id: NodeId(0),
				property_path: name.to_string(),
			}
			.into()),
		}
	}

	pub fn get_raster_optional(&self, name: &str) -> Result<Option<&RasterFrame>, LumenError> {
		match self.ports.get(name) {
			Some(PortValue::RasterFrame(frame)) => Ok(Some(frame)),
			Some(_) => Err(PropertyError::InvalidType {
				node_id: NodeId(0),
				property_path: name.to_string(),
				expected: "RasterFrame",
				actual: "non-raster",
			}
			.into()),
			None => Ok(None),
		}
	}

	pub fn get_vector(&self, name: &str) -> Result<&VectorData, LumenError> {
		match self.ports.get(name) {
			Some(PortValue::Vector(vector)) => Ok(vector),
			Some(_) => Err(PropertyError::InvalidType {
				node_id: NodeId(0),
				property_path: name.to_string(),
				expected: "Vector",
				actual: "non-vector",
			}
			.into()),
			None => Err(PropertyError::MissingProperty {
				node_id: NodeId(0),
				property_path: name.to_string(),
			}
			.into()),
		}
	}
}

pub trait NodeEval: Send + Sync {
	fn input_port_defs(&self) -> &'static [InputPortDef];
	fn output_port_defs(&self) -> &'static [OutputPortDef];
	fn evaluate(&self, inputs: &NodeInputs, ctx: &mut RenderContext) -> Result<PortValue, LumenError>;
}

#[derive(Debug, Clone, PartialEq)]
pub enum PropertyValue {
	Float(f64),
	Int(i64),
	Bool(bool),
	Color([u8; 4]),
	String(String),
	Vector2(f64, f64),
	Map(HashMap<String, PropertyValue>),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
#[non_exhaustive]
pub enum BlendMode {
	Normal = 0,
	Multiply = 1,
	Screen = 2,
	Overlay = 3,
	Darken = 4,
	Lighten = 5,
}

impl From<BlendMode> for skia_safe::BlendMode {
	fn from(value: BlendMode) -> Self {
		match value {
			BlendMode::Normal => Self::SrcOver,
			BlendMode::Multiply => Self::Multiply,
			BlendMode::Screen => Self::Screen,
			BlendMode::Overlay => Self::Overlay,
			BlendMode::Darken => Self::Darken,
			BlendMode::Lighten => Self::Lighten,
		}
	}
}

#[derive(Debug, Clone)]
pub struct Node {
	pub id: NodeId,
	pub kind: NodeKind,
}

impl Node {
	pub fn new(id: NodeId, kind: NodeKind) -> Self {
		Self { id, kind }
	}
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Shape;
#[derive(Debug, Clone, Copy, Default)]
pub struct ShapeRenderer;
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaIn;
#[derive(Debug, Clone, Copy, Default)]
pub struct SolidColor;
#[derive(Debug, Clone, Copy, Default)]
pub struct Text;
#[derive(Debug, Clone, Copy, Default)]
pub struct Transform;
#[derive(Debug, Clone, Copy, Default)]
pub struct Crop;
#[derive(Debug, Clone, Copy, Default)]
pub struct Resize;
#[derive(Debug, Clone, Copy, Default)]
pub struct Blur;
#[derive(Debug, Clone, Copy, Default)]
pub struct Shadow;
#[derive(Debug, Clone, Copy, Default)]
pub struct Boolean;
#[derive(Debug, Clone, Copy, Default)]
pub struct Merge;
#[derive(Debug, Clone, Copy, Default)]
pub struct Switch;
#[derive(Debug, Clone, Copy, Default)]
pub struct FrameHold;
#[derive(Debug, Clone, Copy, Default)]
pub struct MediaOutput;
#[derive(Debug, Clone, Copy, Default)]
pub struct Memo;

#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum NodeKind {
	Shape(Shape),
	ShapeRenderer(ShapeRenderer),
	MediaIn(MediaIn),
	SolidColor(SolidColor),
	Text(Text),
	Transform(Transform),
	Crop(Crop),
	Resize(Resize),
	Blur(Blur),
	Shadow(Shadow),
	Boolean(Boolean),
	Merge(Merge),
	Switch(Switch),
	FrameHold(FrameHold),
	MediaOutput(MediaOutput),
	Memo(Memo),
}

impl NodeKind {
	pub fn kind_name(&self) -> &'static str {
		match self {
			Self::Shape(_) => "Shape",
			Self::ShapeRenderer(_) => "ShapeRenderer",
			Self::MediaIn(_) => "MediaIn",
			Self::SolidColor(_) => "SolidColor",
			Self::Text(_) => "Text",
			Self::Transform(_) => "Transform",
			Self::Crop(_) => "Crop",
			Self::Resize(_) => "Resize",
			Self::Blur(_) => "Blur",
			Self::Shadow(_) => "Shadow",
			Self::Boolean(_) => "Boolean",
			Self::Merge(_) => "Merge",
			Self::Switch(_) => "Switch",
			Self::FrameHold(_) => "FrameHold",
			Self::MediaOutput(_) => "MediaOutput",
			Self::Memo(_) => "Memo",
		}
	}

	pub fn input_port_defs(&self) -> &'static [InputPortDef] {
		match self {
			Self::Shape(node) => node.input_port_defs(),
			Self::ShapeRenderer(node) => node.input_port_defs(),
			Self::MediaIn(node) => node.input_port_defs(),
			Self::SolidColor(node) => node.input_port_defs(),
			Self::Text(node) => node.input_port_defs(),
			Self::Transform(node) => node.input_port_defs(),
			Self::Crop(node) => node.input_port_defs(),
			Self::Resize(node) => node.input_port_defs(),
			Self::Blur(node) => node.input_port_defs(),
			Self::Shadow(node) => node.input_port_defs(),
			Self::Boolean(node) => node.input_port_defs(),
			Self::Merge(node) => node.input_port_defs(),
			Self::Switch(node) => node.input_port_defs(),
			Self::FrameHold(node) => node.input_port_defs(),
			Self::MediaOutput(node) => node.input_port_defs(),
			Self::Memo(node) => node.input_port_defs(),
		}
	}

	pub fn output_port_defs(&self) -> &'static [OutputPortDef] {
		match self {
			Self::Shape(node) => node.output_port_defs(),
			Self::ShapeRenderer(node) => node.output_port_defs(),
			Self::MediaIn(node) => node.output_port_defs(),
			Self::SolidColor(node) => node.output_port_defs(),
			Self::Text(node) => node.output_port_defs(),
			Self::Transform(node) => node.output_port_defs(),
			Self::Crop(node) => node.output_port_defs(),
			Self::Resize(node) => node.output_port_defs(),
			Self::Blur(node) => node.output_port_defs(),
			Self::Shadow(node) => node.output_port_defs(),
			Self::Boolean(node) => node.output_port_defs(),
			Self::Merge(node) => node.output_port_defs(),
			Self::Switch(node) => node.output_port_defs(),
			Self::FrameHold(node) => node.output_port_defs(),
			Self::MediaOutput(node) => node.output_port_defs(),
			Self::Memo(node) => node.output_port_defs(),
		}
	}

	pub fn evaluate(
		&self,
		inputs: &NodeInputs,
		ctx: &mut RenderContext,
	) -> Result<PortValue, LumenError> {
		match self {
			Self::Shape(node) => node.evaluate(inputs, ctx),
			Self::ShapeRenderer(node) => node.evaluate(inputs, ctx),
			Self::MediaIn(node) => node.evaluate(inputs, ctx),
			Self::SolidColor(node) => node.evaluate(inputs, ctx),
			Self::Text(node) => node.evaluate(inputs, ctx),
			Self::Transform(node) => node.evaluate(inputs, ctx),
			Self::Crop(node) => node.evaluate(inputs, ctx),
			Self::Resize(node) => node.evaluate(inputs, ctx),
			Self::Blur(node) => node.evaluate(inputs, ctx),
			Self::Shadow(node) => node.evaluate(inputs, ctx),
			Self::Boolean(node) => node.evaluate(inputs, ctx),
			Self::Merge(node) => node.evaluate(inputs, ctx),
			Self::Switch(node) => node.evaluate(inputs, ctx),
			Self::FrameHold(node) => node.evaluate(inputs, ctx),
			Self::MediaOutput(node) => node.evaluate(inputs, ctx),
			Self::Memo(node) => node.evaluate(inputs, ctx),
		}
	}
}

const NO_INPUTS: [InputPortDef; 0] = [];
const VECTOR_INPUT: [InputPortDef; 1] = [InputPortDef {
	name: "vector",
	kind: PortKind::Vector,
	optional: false,
}];
const SINGLE_RASTER_INPUT: [InputPortDef; 1] = [InputPortDef {
	name: "source",
	kind: PortKind::RasterFrame,
	optional: false,
}];
const MERGE_INPUTS: [InputPortDef; 3] = [
	InputPortDef {
		name: "base",
		kind: PortKind::RasterFrame,
		optional: false,
	},
	InputPortDef {
		name: "overlay",
		kind: PortKind::RasterFrame,
		optional: false,
	},
	InputPortDef {
		name: "mask",
		kind: PortKind::RasterFrame,
		optional: true,
	},
];
const BOOLEAN_INPUTS: [InputPortDef; 2] = [
	InputPortDef {
		name: "source",
		kind: PortKind::RasterFrame,
		optional: false,
	},
	InputPortDef {
		name: "mask",
		kind: PortKind::RasterFrame,
		optional: true,
	},
];
const RASTER_OUTPUT: [OutputPortDef; 1] = [OutputPortDef {
	name: "output",
	kind: PortKind::RasterFrame,
}];
const VECTOR_OUTPUT: [OutputPortDef; 1] = [OutputPortDef {
	name: "output",
	kind: PortKind::Vector,
}];

macro_rules! impl_stub_eval {
	($ty:ty, $inputs:expr, $outputs:expr, $name:expr) => {
		impl NodeEval for $ty {
			fn input_port_defs(&self) -> &'static [InputPortDef] {
				&$inputs
			}

			fn output_port_defs(&self) -> &'static [OutputPortDef] {
				&$outputs
			}

			fn evaluate(
				&self,
				_inputs: &NodeInputs,
				ctx: &mut RenderContext,
			) -> Result<PortValue, LumenError> {
				Err(RenderError::NodeEvaluation {
					frame: ctx.frame,
					node_id: NodeId(0),
					node_kind: $name,
					details: "node implementation not available yet".to_string(),
				}
				.into())
			}
		}
	};
}

impl_stub_eval!(Shape, NO_INPUTS, VECTOR_OUTPUT, "Shape");
impl_stub_eval!(ShapeRenderer, VECTOR_INPUT, RASTER_OUTPUT, "ShapeRenderer");
impl_stub_eval!(MediaIn, NO_INPUTS, RASTER_OUTPUT, "MediaIn");
impl_stub_eval!(SolidColor, NO_INPUTS, RASTER_OUTPUT, "SolidColor");
impl_stub_eval!(Text, NO_INPUTS, RASTER_OUTPUT, "Text");
impl_stub_eval!(Transform, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "Transform");
impl_stub_eval!(Crop, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "Crop");
impl_stub_eval!(Resize, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "Resize");
impl_stub_eval!(Blur, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "Blur");
impl_stub_eval!(Shadow, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "Shadow");
impl_stub_eval!(Boolean, BOOLEAN_INPUTS, RASTER_OUTPUT, "Boolean");
impl_stub_eval!(Merge, MERGE_INPUTS, RASTER_OUTPUT, "Merge");
impl_stub_eval!(Switch, NO_INPUTS, RASTER_OUTPUT, "Switch");
impl_stub_eval!(FrameHold, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "FrameHold");
impl_stub_eval!(MediaOutput, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "MediaOutput");
impl_stub_eval!(Memo, SINGLE_RASTER_INPUT, RASTER_OUTPUT, "Memo");
