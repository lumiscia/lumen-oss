use std::collections::BTreeMap;

use crate::{
    audio::AudioTimeline,
    error::{GraphValidationError, LumenError},
    expr::ExpressionValue,
    graph::Graph,
    node::{ClipPropValue, NodeKind, compositing::clip::Clip},
};

#[derive(Debug, Clone, lumen_macros::StructuralHash)]
pub struct TimelineSettings {
    pub fps: f32,
    pub duration_frames: u32,
}

impl TimelineSettings {
    pub fn time_seconds(&self, frame: u32) -> f64 {
        if self.fps <= 0.0 {
            return 0.0;
        }
        frame as f64 / self.fps as f64
    }
}

#[derive(Debug, Clone, lumen_macros::StructuralHash)]
pub struct RenderSettings {
    pub width: u32,
    pub height: u32,
    pub background_color: [u8; 4],
}

#[derive(Debug, Clone, Default)]
pub struct CompositionMetadata {
    pub name: Option<String>,
}

/// A validated composition that can no longer change. It is fingerprinted once when built, so
/// renderers recognize it across frames without reading all of it again. Edit it through
/// [`Composition::into_builder`].
#[derive(Debug)]
pub struct Composition {
    parts: CompositionBuilder,
    fingerprint: u64,
}

/// The parts of a composition, assembled or edited before [`CompositionBuilder::build`] validates
/// them into a [`Composition`].
#[derive(Debug, lumen_macros::StructuralHash)]
pub struct CompositionBuilder {
    pub graph: Graph,
    pub timeline: TimelineSettings,
    pub render_settings: RenderSettings,
    #[structural_hash(skip)] // Display metadata does not affect rendering.
    pub metadata: Option<CompositionMetadata>,
    #[structural_hash(skip)] // Audio is independent of compiled video plans and GPU caches.
    pub audio: Option<AudioTimeline>,
    /// Templates that `clip` nodes instantiate, by name.
    pub compositions: BTreeMap<String, CompositionTemplate>,
}

/// A reusable composition with its own canvas, timeline and props. Its node ids are its own, so
/// they never collide with the root's or another template's.
#[derive(Debug, lumen_macros::StructuralHash)]
pub struct CompositionTemplate {
    pub graph: Graph,
    pub timeline: TimelineSettings,
    pub render_settings: RenderSettings,
    pub props: BTreeMap<String, PropDef>,
}

/// A prop a template declares, which `prop('name')` reads inside it.
#[derive(Debug, Clone, lumen_macros::StructuralHash)]
pub struct PropDef {
    pub kind: PropKind,
    /// The value an instance that doesn't set the prop gets.
    pub default: ExpressionValue,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, lumen_macros::StructuralHash)]
pub enum PropKind {
    String,
    Number,
    Color,
    Vec2,
}

impl PropKind {
    pub fn parse(name: &str) -> Option<Self> {
        match name {
            "string" => Some(Self::String),
            "number" => Some(Self::Number),
            "color" => Some(Self::Color),
            "vec2" => Some(Self::Vec2),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::String => "string",
            Self::Number => "number",
            Self::Color => "color",
            Self::Vec2 => "vec2",
        }
    }

    /// The value of this kind a prop without a default holds.
    pub fn zero(self) -> ExpressionValue {
        match self {
            Self::String => ExpressionValue::String(String::new()),
            Self::Number => ExpressionValue::Number(0.0),
            Self::Color => ExpressionValue::Color([0.0; 4]),
            Self::Vec2 => ExpressionValue::Vec2((0.0, 0.0)),
        }
    }

    /// `value` when it is of this kind; numbers and vec2 values never convert to strings.
    pub fn check(self, value: ExpressionValue) -> Result<ExpressionValue, String> {
        let matches = matches!(
            (self, &value),
            (Self::String, ExpressionValue::String(_))
                | (Self::Number, ExpressionValue::Number(_))
                | (Self::Color, ExpressionValue::Color(_))
                | (Self::Vec2, ExpressionValue::Vec2(_))
        );
        if matches {
            Ok(value)
        } else {
            Err(format!(
                "expected a {}, got {}",
                self.name(),
                value.type_name()
            ))
        }
    }

    /// Reads a JSON literal of this kind: a string, a number, a `[r, g, b(, a)]` array or
    /// `#rrggbb(aa)` string, or a `[x, y]` array.
    pub fn literal(self, value: &serde_json::Value) -> Result<ExpressionValue, String> {
        let invalid = || format!("expected a {}, got {value}", self.name());
        match self {
            Self::String => value
                .as_str()
                .map(|text| ExpressionValue::String(text.to_string()))
                .ok_or_else(invalid),
            Self::Number => value
                .as_f64()
                .map(ExpressionValue::Number)
                .ok_or_else(invalid),
            Self::Color => crate::json::parse_color(value)
                .map(ExpressionValue::color_from_u8)
                .ok_or_else(invalid),
            Self::Vec2 => match value.as_array().map(Vec::as_slice) {
                Some([x, y]) => x
                    .as_f64()
                    .zip(y.as_f64())
                    .map(ExpressionValue::Vec2)
                    .ok_or_else(invalid),
                _ => Err(invalid()),
            },
        }
    }
}

impl Composition {
    pub fn graph(&self) -> &Graph {
        &self.parts.graph
    }

    pub fn timeline(&self) -> &TimelineSettings {
        &self.parts.timeline
    }

    pub fn render_settings(&self) -> &RenderSettings {
        &self.parts.render_settings
    }

    pub fn metadata(&self) -> Option<&CompositionMetadata> {
        self.parts.metadata.as_ref()
    }

    pub fn audio(&self) -> Option<&AudioTimeline> {
        self.parts.audio.as_ref()
    }

    /// Templates that `clip` nodes instantiate, by name.
    pub fn compositions(&self) -> &BTreeMap<String, CompositionTemplate> {
        &self.parts.compositions
    }

    /// Identifies the composition's content: equal for compositions built from equal parts.
    pub(crate) fn fingerprint(&self) -> u64 {
        self.fingerprint
    }

    /// The composition's parts, to be edited and built again.
    pub fn into_builder(self) -> CompositionBuilder {
        self.parts
    }
}

impl CompositionBuilder {
    pub fn new(graph: Graph, timeline: TimelineSettings, render_settings: RenderSettings) -> Self {
        Self {
            graph,
            timeline,
            render_settings,
            metadata: None,
            audio: None,
            compositions: BTreeMap::new(),
        }
    }

    /// Validates the composition's structure: its graphs, the clips that instantiate templates,
    /// that no template instantiates itself, and its audio timeline.
    pub fn build(self) -> Result<Composition, Vec<LumenError>> {
        self.validate()?;
        Ok(Composition {
            fingerprint: fingerprint(&self),
            parts: self,
        })
    }

    fn validate(&self) -> Result<(), Vec<LumenError>> {
        let mut errors = Vec::new();

        match self.graph.validate() {
            Ok(_) => {}
            Err(graph_errors) => errors.extend(graph_errors),
        }
        for (name, template) in &self.compositions {
            if let Err(graph_errors) = template.graph.validate() {
                errors.extend(graph_errors.into_iter().map(|error| {
                    LumenError::from(GraphValidationError::InTemplate {
                        template: name.clone(),
                        error: Box::new(error),
                    })
                }));
            }
        }
        let graphs = std::iter::once((None, &self.graph)).chain(
            self.compositions
                .iter()
                .map(|(name, template)| (Some(name), &template.graph)),
        );
        for (template, graph) in graphs {
            let mut clips = graph.nodes.values().filter_map(as_clip).collect::<Vec<_>>();
            clips.sort_by_key(|clip| clip.id);
            for clip in clips {
                if let Err(details) = self.check_clip(clip) {
                    errors.push(
                        GraphValidationError::InvalidClip {
                            template: template.cloned(),
                            node_id: clip.id,
                            details,
                        }
                        .into(),
                    );
                }
            }
        }
        if let Some(audio) = &self.audio {
            errors.extend(
                audio
                    .problems()
                    .into_iter()
                    .map(|details| GraphValidationError::InvalidAudio { details }.into()),
            );
        }
        if let Some(templates) = self.template_cycle() {
            errors.push(GraphValidationError::TemplateCycle { templates }.into());
        }

        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }

    /// Checks a clip against the template it names: the template exists, the clip doesn't also
    /// take a source, its props are declared with literals of the right kind, and a literal
    /// `duration` is not negative other than -1.
    fn check_clip(&self, clip: &Clip) -> Result<(), String> {
        if let crate::node::Deferred::Value(duration) = clip.params.duration
            && duration < 0.0
            && duration != crate::node::compositing::clip::PLAY_TO_END
        {
            return Err(format!(
                "`duration` must be at least 0, or -1 to play to the end, got {duration}"
            ));
        }
        let Some(name) = clip.template_name() else {
            return match clip.props().0.keys().next() {
                Some(prop) => Err(format!(
                    "prop `{prop}` is set, but the clip plays its `source` rather than a composition"
                )),
                None => Ok(()),
            };
        };
        let Some(template) = self.compositions.get(name) else {
            let defined = self
                .compositions
                .keys()
                .map(|name| format!("`{name}`"))
                .collect::<Vec<_>>();
            return Err(format!(
                "composition `{name}` is not defined; `compositions` has {}",
                if defined.is_empty() {
                    "none".to_string()
                } else {
                    defined.join(", ")
                }
            ));
        };
        if clip.source.is_some() {
            return Err(
                "a clip plays either a `composition` or its `source` input, not both".to_string(),
            );
        }
        for (prop, value) in &clip.props().0 {
            let Some(def) = template.props.get(prop) else {
                return Err(format!(
                    "composition `{name}` has no prop `{prop}`; {}",
                    declared_props(&template.props)
                ));
            };
            if let ClipPropValue::Literal(value) = value {
                def.kind
                    .literal(value)
                    .map_err(|error| format!("prop `{prop}` of `{name}`: {error}"))?;
            }
        }
        Ok(())
    }

    /// A chain of templates that instantiates itself, ending with the template it returns to.
    /// Each template is searched once.
    fn template_cycle(&self) -> Option<Vec<String>> {
        let mut path = Vec::new();
        let mut done = std::collections::BTreeSet::new();
        self.compositions
            .keys()
            .find_map(|name| self.cycle_from(name, &mut path, &mut done))
    }

    fn cycle_from<'a>(
        &'a self,
        name: &'a str,
        path: &mut Vec<&'a str>,
        done: &mut std::collections::BTreeSet<&'a str>,
    ) -> Option<Vec<String>> {
        if let Some(start) = path.iter().position(|visiting| *visiting == name) {
            return Some(
                path[start..]
                    .iter()
                    .chain([&name])
                    .map(|name| name.to_string())
                    .collect(),
            );
        }
        if done.contains(name) {
            return None;
        }
        // An undefined template is reported by `check_clip`.
        let template = self.compositions.get(name)?;
        let mut used = template
            .graph
            .nodes
            .values()
            .filter_map(|node| as_clip(node)?.template_name())
            .collect::<Vec<_>>();
        used.sort_unstable();
        used.dedup();
        path.push(name);
        let cycle = used
            .into_iter()
            .find_map(|next| self.cycle_from(next, path, done));
        path.pop();
        done.insert(name);
        cycle
    }
}

/// Identifies the exact content of a composition and its templates: their canvases, timelines,
/// props, and every node's inputs, params, literal values and expression sources.
fn fingerprint(composition: &CompositionBuilder) -> u64 {
    crate::fingerprint::fingerprint(composition)
}

fn as_clip(node: &NodeKind) -> Option<&Clip> {
    match node {
        NodeKind::Clip(clip) => Some(clip),
        _ => None,
    }
}

/// The props a template declares, for error messages.
pub(crate) fn declared_props(props: &BTreeMap<String, PropDef>) -> String {
    if props.is_empty() {
        return "the template declares no props".to_string();
    }
    let names = props
        .keys()
        .map(|name| format!("`{name}`"))
        .collect::<Vec<_>>()
        .join(", ");
    format!("the template declares {names}")
}

#[cfg(test)]
mod fingerprint_tests {
    use super::*;
    use serde_json::{Value, json};

    fn scene() -> Value {
        let nodes = json!([
            { "id": 1, "type": "text", "params": { "content": [{
                "text": "Hello", "baseline_shift": 2,
                "font_features": { "liga": 1, "tnum": "=frame / 2" },
                "underline": { "style": "solid", "thickness": "=frame / 2" },
                "color": { "type": "linear_gradient", "stops": [
                    { "offset": 0, "color": "#000000" },
                    { "offset": 1, "color": "#ffffff" }
                ] }
            }] } },
            { "id": 2, "type": "background" },
            { "id": 3, "type": "media_output" }
        ]);
        json!({
            "timeline": { "fps": 30, "duration_frames": 90 },
            "render_settings": { "width": 640, "height": 360 },
            "nodes": nodes,
            "connections": [{ "from_node": 1, "to_node": 3, "to_port": "source" }],
            "compositions": {
                "card": {
                    "render_settings": { "width": 640, "height": 360 },
                    "timeline": { "fps": 30, "duration_frames": 90 },
                    "nodes": nodes,
                    "connections": [{ "from_node": 1, "to_node": 3, "to_port": "source" }],
                    "props": { "title": { "type": "string", "default": "Card" } }
                }
            }
        })
    }

    fn parse(value: &Value) -> Composition {
        crate::json::parse_value(value).unwrap()
    }

    #[test]
    fn equal_parts_have_equal_fingerprints_regardless_of_node_map_order() {
        let value = scene();
        let expected = parse(&value).fingerprint();
        assert_eq!(expected, parse(&value).fingerprint());
        let mut builder = parse(&value).into_builder();
        for graph in std::iter::once(&mut builder.graph).chain(
            builder
                .compositions
                .values_mut()
                .map(|template| &mut template.graph),
        ) {
            let mut nodes = graph.nodes.drain().collect::<Vec<_>>();
            nodes.sort_unstable_by_key(|(id, _)| std::cmp::Reverse(*id));
            graph.nodes = std::collections::HashMap::with_capacity(128);
            graph.nodes.extend(nodes);
        }
        assert_eq!(expected, builder.build().unwrap().fingerprint());
    }

    #[test]
    fn nested_text_fields_and_expression_sources_change_the_fingerprint() {
        let value = scene();
        let expected = parse(&value).fingerprint();
        for (field, replacement) in [
            ("baseline_shift", json!(3)),
            ("font_features/tnum", json!("=frame / 3")),
            ("underline/style", json!("wavy")),
            // Same evaluated value, different source text.
            ("underline/thickness", json!("=frame / 2 + 0")),
            ("color/stops/1/offset", json!(0.75)),
        ] {
            let mut changed = value.clone();
            let path = format!("/nodes/0/params/content/0/{field}");
            *changed.pointer_mut(&path).unwrap() = replacement;
            assert_ne!(expected, parse(&changed).fingerprint(), "{path}");
        }
    }

    #[test]
    fn composition_function_bodies_change_the_fingerprint() {
        let mut value = scene();
        value["functions"] = json!({ "thickness()": "frame / 2" });
        *value
            .pointer_mut("/nodes/0/params/content/0/underline/thickness")
            .unwrap() = json!("=thickness()");
        let expected = parse(&value).fingerprint();
        // The call's source is unchanged; only the body it expands to differs.
        value["functions"]["thickness()"] = json!("frame / 3");
        assert_ne!(expected, parse(&value).fingerprint());
    }

    #[test]
    fn rewiring_an_input_changes_the_fingerprint() {
        let value = scene();
        let expected = parse(&value).fingerprint();
        let mut rewired = value.clone();
        rewired["connections"][0]["from_node"] = json!(2);
        assert_ne!(expected, parse(&rewired).fingerprint());
    }

    #[test]
    fn canvas_timeline_and_template_content_change_the_fingerprint() {
        let value = scene();
        let expected = parse(&value).fingerprint();
        for (path, replacement) in [
            ("/render_settings/width", json!(800)),
            ("/timeline/fps", json!(60)),
            (
                "/compositions/card/render_settings",
                json!({ "width": 800, "height": 360 }),
            ),
            (
                "/compositions/card/timeline",
                json!({ "fps": 60, "duration_frames": 90 }),
            ),
            ("/compositions/card/props/title/default", json!("New title")),
            (
                "/compositions/card/nodes/0/params/content/0/text",
                json!("New text"),
            ),
        ] {
            let mut changed = value.clone();
            *changed.pointer_mut(path).unwrap() = replacement;
            assert_ne!(expected, parse(&changed).fingerprint(), "{path}");
        }
    }
}
