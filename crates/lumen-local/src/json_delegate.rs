use std::{collections::HashMap, convert::TryFrom};

use anyhow::{Context, Result, anyhow};
use lumen::{
    Project,
    clip::{
        ClipGeometry, ClipMeta, ClipType,
        media::{ImageClip, ImageFit, LoopMode, VideoClip},
        shape::ShapeClip,
        style::{
            BaseStyle, Easing, EllipseStyle, Fill, Keyframe, RectStyle, Sequence, StyleExpression,
            StyleProperty, StyleValue, TextAlign, TextDecoration, TextOverflow, TextStyle,
            TransformStyle, VerticalAlign,
        },
        text::TextClip,
    },
    scene::{BlendMode, Layer, Scene},
    time::Rational,
};
use serde::Deserialize;

#[derive(Debug)]
pub struct ProjectBundle {
    pub project: Project,
    pub background: [u8; 4],
    pub image_sources: HashMap<String, String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonProject {
    pub canvas: JsonCanvas,
    pub timeline: JsonTimeline,
    #[serde(default)]
    pub sources: Vec<JsonSource>,
    #[serde(default)]
    pub layers: Vec<JsonLayer>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonCanvas {
    pub width: u32,
    pub height: u32,
    #[serde(default = "default_background")]
    pub background: [u8; 4],
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonTimeline {
    pub fps: JsonRational,
    #[serde(alias = "duration_frames")]
    pub total_frames: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonRational {
    pub num: u32,
    pub den: u32,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonSource {
    pub id: String,
    pub media: JsonSourceMedia,
    pub kind: JsonSourceKind,
    pub path: Option<String>,
    pub url: Option<String>,
    pub filter: Option<String>,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JsonSourceMedia {
    Video,
    Image,
    Audio,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum JsonSourceKind {
    File,
    Url,
    Generator,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonLayer {
    pub id: String,
    #[serde(default)]
    pub z_index: i32,
    #[serde(default = "default_float_one")]
    pub opacity: JsonFloatProperty,
    #[serde(default)]
    pub items: Vec<JsonLayerItem>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonLayerItem {
    #[serde(default = "default_item_kind")]
    pub kind: String,
    pub id: String,
    pub start_frame: u32,
    pub duration_frames: u32,
    #[serde(default = "default_float_one")]
    pub opacity: JsonFloatProperty,
    #[serde(default)]
    pub transform: JsonTransform,
    pub content: JsonClipContent,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(rename_all = "snake_case")]
pub struct JsonTransform {
    #[serde(default = "default_float_zero")]
    pub x: JsonFloatProperty,
    #[serde(default = "default_float_zero")]
    pub y: JsonFloatProperty,
    #[serde(default = "default_float_width")]
    pub width: JsonFloatProperty,
    #[serde(default = "default_float_height")]
    pub height: JsonFloatProperty,
    #[serde(default = "default_float_zero")]
    pub rotation_degrees: JsonFloatProperty,
}

impl Default for JsonTransform {
    fn default() -> Self {
        Self {
            x: default_float_zero(),
            y: default_float_zero(),
            width: default_float_width(),
            height: default_float_height(),
            rotation_degrees: default_float_zero(),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum JsonClipContent {
    Shape {
        shape: JsonShapeKind,
        fill: Option<[u8; 4]>,
        radius: Option<JsonFloatProperty>,
    },
    Text {
        text: String,
        font_size: Option<JsonFloatProperty>,
        font_weight: Option<JsonU32Property>,
        align: Option<JsonTextAlign>,
        color: Option<[u8; 4]>,
        line_height: Option<JsonFloatProperty>,
    },
    Image {
        source: String,
        fit: Option<JsonFitMode>,
    },
    Video {
        source: String,
        fit: Option<JsonFitMode>,
        pipeline: Option<JsonVideoPipeline>,
    },
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonShapeKind {
    Rectangle,
    Ellipse,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonFitMode {
    Cover,
    Contain,
    Fill,
    None,
}

#[derive(Debug, Deserialize, Clone, Copy)]
#[serde(rename_all = "snake_case")]
pub enum JsonTextAlign {
    Left,
    Center,
    Right,
    Justify,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct JsonVideoPipeline {
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default)]
    pub reverse: bool,
    pub trim: Option<JsonTrimRange>,
    pub looping: Option<JsonLooping>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
pub struct JsonTrimRange {
    pub start_frame: u32,
    pub end_frame: u32,
}

#[derive(Debug, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub struct JsonLooping {
    #[serde(default)]
    pub mode: JsonLoopMode,
    pub count: Option<u32>,
}

#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum JsonLoopMode {
    #[default]
    None,
    Infinite,
    Repeat,
    PingPong,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum JsonFloatProperty {
    Literal(f32),
    Expression(String),
    ExpressionObject { expr: String },
    Sequence { keyframes: Vec<JsonFloatKeyframe> },
    Keyframes(Vec<JsonFloatKeyframe>),
}

#[derive(Debug, Deserialize, Clone)]
pub struct JsonFloatKeyframe {
    pub frame: u32,
    pub value: JsonFloatValue,
    #[serde(default)]
    pub easing: JsonEasing,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum JsonFloatValue {
    Literal(f32),
    Expression(String),
    ExpressionObject { expr: String },
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum JsonU32Property {
    Literal(u32),
    Expression(String),
    ExpressionObject { expr: String },
    Sequence { keyframes: Vec<JsonU32Keyframe> },
    Keyframes(Vec<JsonU32Keyframe>),
}

#[derive(Debug, Deserialize, Clone)]
pub struct JsonU32Keyframe {
    pub frame: u32,
    pub value: JsonU32Value,
    #[serde(default)]
    pub easing: JsonEasing,
}

#[derive(Debug, Deserialize, Clone)]
#[serde(untagged)]
pub enum JsonU32Value {
    Literal(u32),
    Expression(String),
    ExpressionObject { expr: String },
}

#[derive(Debug, Deserialize, Clone, Copy, Default)]
#[serde(rename_all = "snake_case")]
pub enum JsonEasing {
    #[default]
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

impl JsonProject {
    pub fn into_bundle(self) -> Result<ProjectBundle> {
        let frame_rate = Rational::new(self.timeline.fps.num, self.timeline.fps.den);
        if frame_rate.den == 0 {
            return Err(anyhow!(
                "timeline.fps.den must be greater than 0, got {}",
                frame_rate.den
            ));
        }
        if self.timeline.total_frames == 0 {
            return Err(anyhow!("timeline.total_frames must be greater than 0"));
        }

        let mut image_sources = HashMap::new();
        for source in &self.sources {
            if source.media != JsonSourceMedia::Image {
                continue;
            }

            match source.kind {
                JsonSourceKind::File => {
                    let path = source.path.as_ref().ok_or_else(|| {
                        anyhow!("source `{}` is file image but has no `path`", source.id)
                    })?;
                    image_sources.insert(source.id.clone(), path.clone());
                }
                JsonSourceKind::Url => {
                    let url = source.url.as_ref().ok_or_else(|| {
                        anyhow!("source `{}` is url image but has no `url`", source.id)
                    })?;
                    image_sources.insert(source.id.clone(), url.clone());
                }
                JsonSourceKind::Generator => {
                    let _ = source.filter.as_ref().ok_or_else(|| {
                        anyhow!(
                            "source `{}` is generator image but has no `filter`",
                            source.id
                        )
                    })?;
                }
            }
        }

        let mut layers = self.layers;
        layers.sort_by_key(|layer| layer.z_index);
        let layers = layers
            .into_iter()
            .map(|layer| layer.try_into_layer(frame_rate))
            .collect::<Result<Vec<_>>>()?;

        let project = Scene {
            width: self.canvas.width,
            height: self.canvas.height,
            frame_rate,
            duration_frames: self.timeline.total_frames,
            layers,
        };

        Ok(ProjectBundle {
            project,
            background: self.canvas.background,
            image_sources,
        })
    }
}

impl JsonLayer {
    fn try_into_layer(self, frame_rate: Rational) -> Result<Layer> {
        let clips = self
            .items
            .into_iter()
            .map(|item| item.try_into_clip(frame_rate))
            .collect::<Result<Vec<_>>>()?;

        Ok(Layer {
            id: self.id,
            clips,
            blend_mode: BlendMode::Normal,
            opacity: self.opacity.into_style_property(),
            visible: true,
        })
    }
}

impl JsonLayerItem {
    fn try_into_clip(self, frame_rate: Rational) -> Result<ClipType> {
        if self.kind != "clip" {
            return Err(anyhow!(
                "layer item `{}` uses unsupported kind `{}`",
                self.id,
                self.kind
            ));
        }

        let meta = clip_meta(self.id.clone(), self.start_frame, self.duration_frames)?;
        let geometry = clip_geometry(&self.transform);
        let base_style = base_style(self.opacity, self.transform.rotation_degrees.clone());

        match self.content {
            JsonClipContent::Shape {
                shape,
                fill,
                radius,
            } => {
                let fill = fill.map(to_fill);
                match shape {
                    JsonShapeKind::Rectangle => {
                        let corner = radius
                            .unwrap_or_else(default_float_zero)
                            .into_style_property();
                        let style = RectStyle {
                            base: base_style,
                            width: self.transform.width.clone().into_style_property(),
                            height: self.transform.height.clone().into_style_property(),
                            corner_radius: [corner.clone(), corner.clone(), corner.clone(), corner],
                            fill,
                            stroke: None,
                        };
                        Ok(ClipType::Shape(ShapeClip::rectangle(meta, geometry, style)))
                    }
                    JsonShapeKind::Ellipse => {
                        let style = EllipseStyle {
                            base: base_style,
                            width: self.transform.width.clone().into_style_property(),
                            height: self.transform.height.clone().into_style_property(),
                            fill,
                            stroke: None,
                        };
                        Ok(ClipType::Shape(ShapeClip::ellipse(meta, geometry, style)))
                    }
                }
            }
            JsonClipContent::Text {
                text,
                font_size,
                font_weight,
                align,
                color,
                line_height,
            } => {
                let color = color.unwrap_or([255, 255, 255, 255]);
                let style = TextStyle {
                    base: base_style,
                    font_family: "Inter".to_string(),
                    font_size: font_size
                        .unwrap_or_else(default_text_font_size)
                        .into_style_property(),
                    font_weight: font_weight
                        .unwrap_or_else(default_text_font_weight)
                        .into_style_property(),
                    font_style: skia_safe::font_style::Slant::Upright,
                    color: [
                        literal(color[0]),
                        literal(color[1]),
                        literal(color[2]),
                        literal(color[3]),
                    ],
                    line_height: line_height
                        .unwrap_or_else(default_text_line_height)
                        .into_style_property(),
                    letter_spacing: literal(0.0),
                    text_align: to_text_align(align.unwrap_or(JsonTextAlign::Left)),
                    vertical_align: VerticalAlign::Top,
                    max_width: Some(self.transform.width.clone().into_style_property()),
                    max_lines: None,
                    overflow: TextOverflow::Clip,
                    decoration: TextDecoration::None,
                };
                Ok(ClipType::Text(TextClip::new(meta, geometry, text, style)))
            }
            JsonClipContent::Image { source, fit } => {
                let fit = fit.map(to_image_fit).unwrap_or(ImageFit::Cover);
                Ok(ClipType::Image(
                    ImageClip::new(meta, source, fit, base_style).with_geometry(geometry),
                ))
            }
            JsonClipContent::Video {
                source,
                fit,
                pipeline,
            } => {
                let fit = fit.map(to_image_fit).unwrap_or(ImageFit::Cover);
                let mut clip =
                    VideoClip::new(meta, source, fit, base_style).with_geometry(geometry);
                if let Some(pipeline) = pipeline {
                    let speed = if pipeline.reverse {
                        -pipeline.speed.abs().max(0.01)
                    } else {
                        pipeline.speed.max(0.01)
                    };
                    clip = clip.with_speed(speed);
                    clip = clip.with_loop_mode(to_loop_mode(pipeline.looping));
                    if let Some(trim) = pipeline.trim {
                        let fps = frame_rate.as_f32();
                        if fps <= 0.0 {
                            return Err(anyhow!("invalid fps for video trim conversion"));
                        }
                        let start = trim.start_frame as f32 / fps;
                        let end = trim.end_frame as f32 / fps;
                        if end > start {
                            clip = clip.with_trim(Some(start..end));
                        }
                    }
                }
                Ok(ClipType::Video(clip))
            }
        }
    }
}

impl JsonFloatProperty {
    fn into_style_property(self) -> StyleProperty<f32> {
        match self {
            Self::Literal(value) => literal(value),
            Self::Expression(expr) => expression(expr),
            Self::ExpressionObject { expr } => expression(expr),
            Self::Sequence { keyframes } | Self::Keyframes(keyframes) => {
                StyleProperty::Sequence(Sequence::new(
                    keyframes
                        .into_iter()
                        .map(JsonFloatKeyframe::into_keyframe)
                        .collect(),
                ))
            }
        }
    }
}

impl JsonFloatKeyframe {
    fn into_keyframe(self) -> Keyframe<f32> {
        Keyframe::new(self.frame, self.value.into_style_value()).with_easing(self.easing.into())
    }
}

impl JsonFloatValue {
    fn into_style_value(self) -> StyleValue<f32> {
        match self {
            Self::Literal(value) => StyleValue::Literal(value),
            Self::Expression(expr) => StyleValue::Expression(StyleExpression::new(expr)),
            Self::ExpressionObject { expr } => StyleValue::Expression(StyleExpression::new(expr)),
        }
    }
}

impl JsonU32Property {
    fn into_style_property(self) -> StyleProperty<u32> {
        match self {
            Self::Literal(value) => literal(value),
            Self::Expression(expr) => {
                StyleProperty::Value(StyleValue::Expression(StyleExpression::new(expr)))
            }
            Self::ExpressionObject { expr } => {
                StyleProperty::Value(StyleValue::Expression(StyleExpression::new(expr)))
            }
            Self::Sequence { keyframes } | Self::Keyframes(keyframes) => {
                StyleProperty::Sequence(Sequence::new(
                    keyframes
                        .into_iter()
                        .map(JsonU32Keyframe::into_keyframe)
                        .collect(),
                ))
            }
        }
    }
}

impl JsonU32Keyframe {
    fn into_keyframe(self) -> Keyframe<u32> {
        Keyframe::new(self.frame, self.value.into_style_value()).with_easing(self.easing.into())
    }
}

impl JsonU32Value {
    fn into_style_value(self) -> StyleValue<u32> {
        match self {
            Self::Literal(value) => StyleValue::Literal(value),
            Self::Expression(expr) => StyleValue::Expression(StyleExpression::new(expr)),
            Self::ExpressionObject { expr } => StyleValue::Expression(StyleExpression::new(expr)),
        }
    }
}

impl From<JsonEasing> for Easing {
    fn from(value: JsonEasing) -> Self {
        match value {
            JsonEasing::Linear => Easing::Linear,
            JsonEasing::EaseIn => Easing::EaseIn,
            JsonEasing::EaseOut => Easing::EaseOut,
            JsonEasing::EaseInOut => Easing::EaseInOut,
        }
    }
}

fn clip_meta(id: String, start_frame: u32, duration_frames: u32) -> Result<ClipMeta> {
    if duration_frames == 0 {
        return Err(anyhow!(
            "clip `{id}` has duration_frames=0; duration must be at least 1"
        ));
    }
    let end_frame = start_frame
        .checked_add(duration_frames - 1)
        .with_context(|| format!("clip `{id}` frame range overflow"))?;
    Ok(ClipMeta {
        id: Some(id),
        start_frame,
        end_frame,
    })
}

fn clip_geometry(transform: &JsonTransform) -> ClipGeometry {
    ClipGeometry {
        x: transform.x.clone().into_style_property(),
        y: transform.y.clone().into_style_property(),
        width: transform.width.clone().into_style_property(),
        height: transform.height.clone().into_style_property(),
        anchor_x: literal(0.0),
        anchor_y: literal(0.0),
    }
}

fn base_style(opacity: JsonFloatProperty, rotation_degrees: JsonFloatProperty) -> BaseStyle {
    BaseStyle {
        visible: literal(true),
        opacity: opacity.into_style_property(),
        blend_mode: skia_safe::BlendMode::SrcOver,
        blur: literal(0.0),
        shadows: Vec::new(),
        clip_radius: [literal(0.0), literal(0.0), literal(0.0), literal(0.0)],
        transform: TransformStyle {
            translate: [literal(0.0), literal(0.0)],
            scale: [literal(1.0), literal(1.0)],
            rotation: rotation_degrees.into_style_property(),
            skew: [literal(0.0), literal(0.0)],
            origin: [literal(0.0), literal(0.0)],
        },
        alignment: [literal(0.0), literal(0.0)],
        mask: None,
    }
}

fn to_fill(color: [u8; 4]) -> Fill {
    Fill::Solid {
        color: [
            literal(color[0]),
            literal(color[1]),
            literal(color[2]),
            literal(color[3]),
        ],
    }
}

fn to_text_align(value: JsonTextAlign) -> TextAlign {
    match value {
        JsonTextAlign::Left => TextAlign::Left,
        JsonTextAlign::Center => TextAlign::Center,
        JsonTextAlign::Right => TextAlign::Right,
        JsonTextAlign::Justify => TextAlign::Justify,
    }
}

fn to_image_fit(value: JsonFitMode) -> ImageFit {
    match value {
        JsonFitMode::Cover => ImageFit::Cover,
        JsonFitMode::Contain => ImageFit::Contain,
        JsonFitMode::Fill => ImageFit::Fill,
        JsonFitMode::None => ImageFit::None,
    }
}

fn to_loop_mode(looping: Option<JsonLooping>) -> LoopMode {
    let Some(looping) = looping else {
        return LoopMode::None;
    };

    match looping.mode {
        JsonLoopMode::None => LoopMode::None,
        JsonLoopMode::Infinite => LoopMode::Repeat,
        JsonLoopMode::Repeat => {
            if looping.count.unwrap_or(2) > 1 {
                LoopMode::Repeat
            } else {
                LoopMode::None
            }
        }
        JsonLoopMode::PingPong => LoopMode::PingPong,
    }
}

fn literal<T>(value: T) -> StyleProperty<T> {
    StyleProperty::Value(StyleValue::Literal(value))
}

fn expression(expr: String) -> StyleProperty<f32> {
    StyleProperty::Value(StyleValue::Expression(StyleExpression::new(expr)))
}

fn default_item_kind() -> String {
    "clip".to_string()
}

fn default_float_zero() -> JsonFloatProperty {
    JsonFloatProperty::Literal(0.0)
}

fn default_float_one() -> JsonFloatProperty {
    JsonFloatProperty::Literal(1.0)
}

fn default_float_width() -> JsonFloatProperty {
    JsonFloatProperty::Literal(100.0)
}

fn default_float_height() -> JsonFloatProperty {
    JsonFloatProperty::Literal(100.0)
}

fn default_text_font_size() -> JsonFloatProperty {
    JsonFloatProperty::Literal(48.0)
}

fn default_text_font_weight() -> JsonU32Property {
    JsonU32Property::Literal(600)
}

fn default_text_line_height() -> JsonFloatProperty {
    JsonFloatProperty::Literal(1.2)
}

fn default_background() -> [u8; 4] {
    [0, 0, 0, 255]
}

fn default_speed() -> f32 {
    1.0
}

impl TryFrom<JsonProject> for ProjectBundle {
    type Error = anyhow::Error;

    fn try_from(value: JsonProject) -> Result<Self, Self::Error> {
        value.into_bundle()
    }
}

#[cfg(test)]
mod tests {
    use lumen::clip::{
        ClipType,
        style::{StyleProperty, StyleValue},
    };

    use super::{JsonProject, ProjectBundle};

    #[test]
    fn parses_and_converts_shape_text_project() {
        let raw = r#"
		{
		  "canvas": { "width": 640, "height": 360, "background": [8, 12, 20, 255] },
		  "timeline": { "fps": { "num": 30, "den": 1 }, "total_frames": 90 },
		  "layers": [
		    {
		      "id": "layer_shapes",
		      "z_index": 2,
		      "items": [
		        {
		          "id": "shape_1",
		          "start_frame": 0,
		          "duration_frames": 90,
		          "opacity": 0.8,
		          "transform": { "x": 120, "y": 80, "width": 300, "height": 140, "rotation_degrees": 15 },
		          "content": {
		            "type": "shape",
		            "shape": "rectangle",
		            "fill": [120, 40, 240, 230],
		            "radius": 18
		          }
		        }
		      ]
		    },
		    {
		      "id": "layer_text",
		      "z_index": 5,
		      "items": [
		        {
		          "id": "text_1",
		          "start_frame": 0,
		          "duration_frames": 90,
		          "transform": { "x": 60, "y": 240, "width": 520, "height": 90 },
		          "content": {
		            "type": "text",
		            "text": "delegate conversion works",
		            "font_size": 42,
		            "align": "left",
		            "color": [250, 252, 255, 255]
		          }
		        }
		      ]
		    }
		  ]
		}
		"#;

        let delegate: JsonProject = serde_json::from_str(raw).expect("delegate JSON parse");
        let bundle: ProjectBundle = delegate.try_into().expect("delegate conversion");

        assert_eq!(bundle.project.width, 640);
        assert_eq!(bundle.project.height, 360);
        assert_eq!(bundle.project.duration_frames, 90);
        assert_eq!(bundle.project.layers.len(), 2);
        assert!(bundle.image_sources.is_empty());
    }

    #[test]
    fn parses_expression_and_keyframe_properties() {
        let raw = r#"
		{
		  "canvas": { "width": 640, "height": 360 },
		  "timeline": { "fps": { "num": 30, "den": 1 }, "total_frames": 120 },
		  "layers": [
		    {
		      "id": "layer_expr",
		      "opacity": {
		        "keyframes": [
		          { "frame": 0, "value": 0.1 },
		          { "frame": 60, "value": "clamp(0.9, 0.0, 1.0)", "easing": "ease_in_out" }
		        ]
		      },
		      "items": [
		        {
		          "id": "shape_expr",
		          "start_frame": 0,
		          "duration_frames": 120,
		          "transform": {
		            "x": "lerp(20, 220, 0.5)",
		            "y": 80,
		            "width": { "keyframes": [{ "frame": 0, "value": 120 }, { "frame": 60, "value": 300 }] },
		            "height": 100
		          },
		          "content": {
		            "type": "shape",
		            "shape": "rectangle"
		          }
		        }
		      ]
		    }
		  ]
		}
		"#;

        let delegate: JsonProject = serde_json::from_str(raw).expect("delegate JSON parse");
        let bundle: ProjectBundle = delegate.try_into().expect("delegate conversion");
        let layer = &bundle.project.layers[0];
        assert!(matches!(layer.opacity, StyleProperty::Sequence(_)));

        let clip = layer.clips.first().expect("clip present");
        let ClipType::Shape(shape_clip) = clip else {
            panic!("expected shape clip");
        };
        assert!(matches!(
            shape_clip.geometry.x,
            StyleProperty::Value(StyleValue::Expression(_))
        ));
        assert!(matches!(
            shape_clip.geometry.width,
            StyleProperty::Sequence(_)
        ));
    }

    #[test]
    fn conversion_rejects_zero_duration_clip() {
        let raw = r#"
		{
		  "canvas": { "width": 320, "height": 180 },
		  "timeline": { "fps": { "num": 30, "den": 1 }, "total_frames": 10 },
		  "layers": [
		    {
		      "id": "layer_1",
		      "items": [
		        {
		          "id": "bad_clip",
		          "start_frame": 0,
		          "duration_frames": 0,
		          "content": { "type": "shape", "shape": "rectangle" }
		        }
		      ]
		    }
		  ]
		}
		"#;

        let delegate: JsonProject = serde_json::from_str(raw).expect("delegate JSON parse");
        let error = ProjectBundle::try_from(delegate).expect_err("conversion should fail");
        assert!(error.to_string().contains("duration_frames=0"));
    }
}
