//! What a composition needs from its host before it renders: the media its nodes and audio
//! clips name, and the font families its text uses.

use std::{collections::HashMap, fmt};

use serde::Serialize;

use crate::{
    composition::{Composition, Scene},
    node::{MediaSource, MediaValue, Node, NodeId, NodeKind},
};

use super::AssetKind;

/// A param of a node, in the root composition or in a template.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub struct NodeParamAt {
    /// The template the node is in, absent for the root.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub template: Option<String>,
    pub node: NodeId,
    /// The param's path in the node's `params`, such as `source` or `content[1].font_family`.
    pub param: String,
}

impl fmt::Display for NodeParamAt {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(template) = &self.template {
            write!(f, "composition `{template}`, ")?;
        }
        write!(f, "node {} `{}`", self.node, self.param)
    }
}

/// Where a composition names a source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum MediaLocation {
    Node(NodeParamAt),
    /// The `source_id` of a clip of an audio timeline, in the root or in a template.
    #[serde(rename_all = "camelCase")]
    AudioClip {
        #[serde(skip_serializing_if = "Option::is_none")]
        template: Option<String>,
        track: String,
        clip: String,
        /// The clip's position in the timeline's `clips`.
        index: usize,
    },
}

impl fmt::Display for MediaLocation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Node(at) => at.fmt(f),
            Self::AudioClip {
                template,
                track,
                clip,
                ..
            } => {
                if let Some(template) = template {
                    write!(f, "composition `{template}`, ")?;
                }
                write!(f, "audio clip `{clip}` on track `{track}`")
            }
        }
    }
}

/// One place a source is used, and the kinds of media it reads there.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub struct MediaUse {
    pub at: MediaLocation,
    pub kinds: Vec<AssetKind>,
}

/// A source the composition names as written (a path, a URL or an alias, or `asset:<id>`), with
/// each place it is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub struct MediaDependency {
    pub source: String,
    /// The kinds of media the uses read, without repeats, in the order the uses first name them.
    pub kinds: Vec<AssetKind>,
    pub uses: Vec<MediaUse>,
}

/// A media param set by an expression, whose source is known only when a frame evaluates it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub struct DynamicMediaUse {
    #[serde(flatten)]
    pub at: NodeParamAt,
    pub kinds: Vec<AssetKind>,
}

/// A font family a text node names as a literal. A family is not a media source: the host
/// resolves it by name, falling back to its own fonts, unless it is written `asset:<id>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub struct FontDependency {
    pub family: String,
    pub uses: Vec<NodeParamAt>,
}

/// Everything a composition needs before it renders, found without rendering it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[cfg_attr(feature = "metadata", derive(schemars::JsonSchema))]
pub struct MediaInventory {
    /// Each distinct source, in the order the root's nodes, the templates' nodes and then the
    /// audio first name it.
    pub media: Vec<MediaDependency>,
    pub dynamic: Vec<DynamicMediaUse>,
    pub fonts: Vec<FontDependency>,
}

/// One place the composition names media or a font, as the walk finds it.
pub(crate) enum Found<'a> {
    Media {
        source: &'a str,
        kinds: Vec<AssetKind>,
        at: MediaLocation,
    },
    Dynamic {
        kinds: Vec<AssetKind>,
        at: NodeParamAt,
    },
    Font {
        family: &'a str,
        at: NodeParamAt,
    },
}

impl Composition {
    /// The media and font families this composition names, with where each is used. Sources
    /// that are set to the same string appear once, with all their uses; a source an expression
    /// computes is listed in `dynamic` instead. Unset (empty) sources are left out.
    ///
    /// The kinds are those a node's `#[media(...)]` declares, narrowed to the one its selector
    /// names.
    pub fn media_dependencies(&self) -> MediaInventory {
        let mut inventory = MediaInventory::default();
        let mut media: HashMap<&str, usize> = HashMap::new();
        let mut fonts: HashMap<&str, usize> = HashMap::new();
        walk(self, &mut |found| match found {
            Found::Media { source, kinds, at } => {
                let index = *media.entry(source).or_insert_with(|| {
                    inventory.media.push(MediaDependency {
                        source: source.to_string(),
                        kinds: Vec::new(),
                        uses: Vec::new(),
                    });
                    inventory.media.len() - 1
                });
                let dependency = &mut inventory.media[index];
                for kind in &kinds {
                    if !dependency.kinds.contains(kind) {
                        dependency.kinds.push(*kind);
                    }
                }
                dependency.uses.push(MediaUse { at, kinds });
            }
            Found::Dynamic { kinds, at } => inventory.dynamic.push(DynamicMediaUse { at, kinds }),
            Found::Font { family, at } => {
                let index = *fonts.entry(family).or_insert_with(|| {
                    inventory.fonts.push(FontDependency {
                        family: family.to_string(),
                        uses: Vec::new(),
                    });
                    inventory.fonts.len() - 1
                });
                inventory.fonts[index].uses.push(at);
            }
        });
        inventory
    }
}

/// Calls `f` with each place `composition` names media or a font: the nodes of the root and then
/// of each template, in id order, and then the audio timelines.
pub(crate) fn walk<'a>(composition: &'a Composition, f: &mut dyn FnMut(Found<'a>)) {
    let scenes = std::iter::once((None, composition.scene())).chain(
        composition
            .compositions()
            .iter()
            .map(|(name, template)| (Some(name.as_str()), &template.scene)),
    );
    for (template, scene) in scenes {
        walk_nodes(template, scene, f);
    }
    let timelines = std::iter::once((None, composition.audio())).chain(
        composition
            .compositions()
            .iter()
            .map(|(name, template)| (Some(name.as_str()), template.audio.as_ref())),
    );
    for (template, audio) in timelines {
        for (index, clip) in audio
            .map_or(&[][..], |audio| &audio.clips)
            .iter()
            .enumerate()
        {
            clip.for_each_media_source(&mut |source| {
                if let MediaValue::Literal(value) = source.value
                    && !value.is_empty()
                {
                    f(Found::Media {
                        source: value,
                        kinds: source.kinds,
                        at: MediaLocation::AudioClip {
                            template: template.map(str::to_string),
                            track: clip.track_id.clone(),
                            clip: clip.id.clone(),
                            index,
                        },
                    });
                }
            });
        }
    }
}

fn walk_nodes<'a>(template: Option<&str>, scene: &'a Scene, f: &mut dyn FnMut(Found<'a>)) {
    let mut nodes: Vec<&NodeKind> = scene.graph.nodes.values().collect();
    nodes.sort_by_key(|node| node.id());
    let at = |node: NodeId, param: &str| NodeParamAt {
        template: template.map(str::to_string),
        node,
        param: param.to_string(),
    };
    for node in nodes {
        let id = node.id();
        node.for_each_media_source(&mut |MediaSource {
                                             param,
                                             kinds,
                                             value,
                                         }| match value {
            MediaValue::Literal(source) if !source.is_empty() => f(Found::Media {
                source,
                kinds,
                at: MediaLocation::Node(at(id, param)),
            }),
            MediaValue::Literal(_) => {}
            MediaValue::Expression(_) => f(Found::Dynamic {
                kinds,
                at: at(id, param),
            }),
        });
        let families = match node {
            NodeKind::Text(text) => crate::node::source::text::literal_font_families(text),
            NodeKind::Text3d(text) => text.literal_font_families(),
            _ => Vec::new(),
        };
        for (param, family) in families {
            if !family.is_empty() {
                f(Found::Font {
                    family,
                    at: at(id, &param),
                });
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::{Value, json};

    use super::*;
    use crate::json::parse_value;

    fn composition(nodes: Value, extra: Value) -> Composition {
        let mut nodes = nodes;
        nodes.as_array_mut().unwrap().extend([
            json!({ "id": 100, "type": "media_output" }),
            json!({ "id": 101, "type": "background" }),
        ]);
        let mut document = json!({
            "timeline": { "duration_frames": 1 },
            "render_settings": { "width": 16, "height": 16 },
            "nodes": nodes,
            "connections": [{ "from_node": 101, "to_node": 100, "to_port": "source" }],
        });
        for (key, value) in extra.as_object().into_iter().flatten() {
            document[key] = value.clone();
        }
        parse_value(&document).unwrap_or_else(|error| panic!("{error:#}"))
    }

    fn media_in(id: u64, kind: &str, source: &str) -> Value {
        json!({ "id": id, "type": "media_in", "params": { "kind": kind, "source": source } })
    }

    fn at_node(template: Option<&str>, node: u64, param: &str) -> MediaLocation {
        MediaLocation::Node(NodeParamAt {
            template: template.map(str::to_string),
            node: NodeId::new(node),
            param: param.to_string(),
        })
    }

    #[test]
    fn a_source_used_twice_is_listed_once_with_both_uses() {
        let inventory = composition(
            json!([
                media_in(1, "image", "logo.png"),
                media_in(2, "video", "clip.mp4"),
                media_in(3, "image", "logo.png"),
                media_in(4, "video", "logo.png"),
                media_in(5, "image", ""),
            ]),
            json!({}),
        )
        .media_dependencies();

        let sources: Vec<_> = inventory
            .media
            .iter()
            .map(|dependency| dependency.source.as_str())
            .collect();
        assert_eq!(sources, ["logo.png", "clip.mp4"]);
        let logo = &inventory.media[0];
        assert_eq!(logo.kinds, [AssetKind::Image, AssetKind::Video]);
        assert_eq!(
            logo.uses,
            [
                MediaUse {
                    at: at_node(None, 1, "source"),
                    kinds: vec![AssetKind::Image]
                },
                MediaUse {
                    at: at_node(None, 3, "source"),
                    kinds: vec![AssetKind::Image]
                },
                MediaUse {
                    at: at_node(None, 4, "source"),
                    kinds: vec![AssetKind::Video]
                },
            ]
        );
        assert!(inventory.dynamic.is_empty());
    }

    #[test]
    fn a_kind_set_by_an_expression_accepts_images_and_videos() {
        let inventory = composition(
            json!([media_in(1, "=if(frame < 5, 0, 1)", "asset:clip")]),
            json!({}),
        )
        .media_dependencies();

        assert_eq!(
            inventory.media[0].uses[0].kinds,
            [AssetKind::Image, AssetKind::Video]
        );
    }

    #[test]
    fn a_source_an_expression_computes_is_dynamic() {
        let inventory = composition(
            json!([media_in(1, "image", "=if(frame < 5, 'a.png', 'b.png')")]),
            json!({}),
        )
        .media_dependencies();

        assert!(inventory.media.is_empty());
        assert_eq!(
            inventory.dynamic,
            [DynamicMediaUse {
                at: NodeParamAt {
                    template: None,
                    node: NodeId::new(1),
                    param: "source".to_string()
                },
                kinds: vec![AssetKind::Image],
            }]
        );
    }

    #[test]
    fn templates_name_their_media_under_the_template() {
        let inventory = composition(
            json!([media_in(1, "image", "logo.png")]),
            json!({
                "compositions": {
                    "card": {
                        "nodes": [
                            media_in(1, "image", "logo.png"),
                            media_in(2, "video", "=if(frame < 5, 'a.mp4', 'b.mp4')"),
                            { "id": 3, "type": "text", "params": { "font_family": "Display" } },
                            { "id": 4, "type": "media_output" }
                        ],
                        "connections": [{ "from_node": 3, "to_node": 4, "to_port": "source" }]
                    }
                }
            }),
        )
        .media_dependencies();

        assert_eq!(
            inventory.media[0]
                .uses
                .iter()
                .map(|usage| usage.at.clone())
                .collect::<Vec<_>>(),
            [
                at_node(None, 1, "source"),
                at_node(Some("card"), 1, "source")
            ]
        );
        assert_eq!(inventory.dynamic.len(), 1);
        assert_eq!(inventory.dynamic[0].at.template.as_deref(), Some("card"));
        assert_eq!(inventory.fonts.len(), 1);
        assert_eq!(inventory.fonts[0].uses[0].template.as_deref(), Some("card"));
    }

    #[test]
    fn audio_clips_are_media_of_the_audio_kind() {
        let inventory = composition(
            json!([media_in(1, "image", "voice.wav")]),
            json!({
                "audio": {
                    "tracks": [{ "id": "t" }],
                    "clips": [
                        { "id": "a", "source_id": "voice.wav", "track_id": "t", "start_ms": 0, "duration_ms": 10 },
                        { "id": "b", "source_id": "asset:song", "track_id": "t", "start_ms": 0, "duration_ms": 10 }
                    ]
                }
            }),
        )
        .media_dependencies();

        let voice = &inventory.media[0];
        assert_eq!(voice.kinds, [AssetKind::Image, AssetKind::Audio]);
        assert_eq!(
            voice.uses[1].at,
            MediaLocation::AudioClip {
                template: None,
                track: "t".to_string(),
                clip: "a".to_string(),
                index: 0
            }
        );
        assert_eq!(inventory.media[1].source, "asset:song");
        assert_eq!(inventory.media[1].kinds, [AssetKind::Audio]);
    }

    #[test]
    fn font_families_are_listed_apart_from_media() {
        let inventory = composition(
            json!([{ "id": 1, "type": "text", "params": {
                "font_family": "Display",
                "content": [{ "text": "a", "font_family": "asset:brand" }, { "text": "b" }]
            } }]),
            json!({}),
        )
        .media_dependencies();

        assert!(inventory.media.is_empty());
        let families: Vec<_> = inventory
            .fonts
            .iter()
            .map(|font| (font.family.as_str(), font.uses[0].param.as_str()))
            .collect();
        assert_eq!(
            families,
            [
                ("Display", "font_family"),
                ("asset:brand", "content[0].font_family")
            ]
        );
    }

    #[test]
    fn the_inventory_serializes_as_the_bindings_return_it() {
        let inventory = composition(
            json!([media_in(1, "video", "clip.mp4")]),
            json!({ "audio": {
                "tracks": [{ "id": "t" }],
                "clips": [{ "id": "a", "source_id": "song.mp3", "track_id": "t", "start_ms": 0, "duration_ms": 10 }]
            } }),
        )
        .media_dependencies();

        assert_eq!(
            serde_json::to_value(&inventory).unwrap(),
            json!({
                "media": [
                    {
                        "source": "clip.mp4",
                        "kinds": ["video"],
                        "uses": [{
                            "at": { "type": "node", "node": 1, "param": "source" },
                            "kinds": ["video"]
                        }]
                    },
                    {
                        "source": "song.mp3",
                        "kinds": ["audio"],
                        "uses": [{
                            "at": { "type": "audioClip", "track": "t", "clip": "a", "index": 0 },
                            "kinds": ["audio"]
                        }]
                    }
                ],
                "dynamic": [],
                "fonts": []
            })
        );
    }

    #[cfg(feature = "3d")]
    #[test]
    fn models_and_environment_maps_are_media_of_their_own_kinds() {
        let inventory = composition(
            json!([
                { "id": 1, "type": "model_3d", "params": { "source": "asset:robot" } },
                { "id": 2, "type": "render_3d", "params": { "environment_map": "sky.hdr" } },
                { "id": 3, "type": "model_3d", "params": { "source": "=if(frame < 5, 'a.glb', 'b.glb')" } },
            ]),
            json!({ "connections": [
                { "from_node": 101, "to_node": 100, "to_port": "source" },
                { "from_node": 1, "to_node": 2, "to_port": "scene" }
            ] }),
        )
        .media_dependencies();

        let kinds: Vec<_> = inventory
            .media
            .iter()
            .map(|dependency| (dependency.source.as_str(), dependency.kinds.clone()))
            .collect();
        assert_eq!(
            kinds,
            [
                ("asset:robot", vec![AssetKind::Model]),
                ("sky.hdr", vec![AssetKind::Environment])
            ]
        );
        assert_eq!(inventory.dynamic.len(), 1);
        assert_eq!(inventory.dynamic[0].kinds, [AssetKind::Model]);
    }

    /// Every node declares the params it reads media from with `#[media(...)]`, so a node
    /// built from JSON reports each one.
    #[cfg(feature = "metadata")]
    #[test]
    fn every_declared_media_param_is_visited() {
        let mut declared = 0;
        for schema in NodeKind::schemas() {
            for property in schema
                .properties
                .iter()
                .filter(|property| property.media.is_some())
            {
                let media = property.media.unwrap();
                assert_eq!(media.param, property.id);
                let mut params = serde_json::Map::new();
                params.insert(property.id.to_string(), json!("a.bin"));
                let node = NodeKind::from_json(schema.kind, NodeId::new(1), Some(&params))
                    .unwrap_or_else(|error| panic!("{}: {error:?}", schema.kind));

                let mut visited = Vec::new();
                node.for_each_media_source(&mut |source| {
                    visited.push((source.param, source.value.clone(), source.kinds.clone()));
                });
                let (_, value, kinds) = visited
                    .iter()
                    .find(|(param, ..)| *param == property.id)
                    .unwrap_or_else(|| panic!("{}.{} is not visited", schema.kind, property.id));
                assert_eq!(*value, MediaValue::Literal("a.bin"));
                // A selector left at its default picks the first kind, since it is the default
                // option; none picks all of them.
                if media.selector.is_none() {
                    assert_eq!(kinds.as_slice(), media.kinds);
                } else {
                    assert_eq!(kinds.as_slice(), &media.kinds[..1]);
                }
                declared += 1;
            }
        }
        assert_eq!(
            declared, 4,
            "media_in.source, model_3d.source, render_3d.environment_map, color_grade.lut_source"
        );
    }

    /// A param that looks like it names a file must declare `#[media(...)]`, or be listed here
    /// with why it doesn't.
    #[cfg(feature = "metadata")]
    #[test]
    fn params_that_look_like_sources_declare_media_or_say_why_not() {
        const NOT_MEDIA: [(&str, &str, &str); 2] = [
            (
                "color_grade",
                "lut",
                "inline LUT stops or `identity`, not a file",
            ),
            (
                "render_3d",
                "environment",
                "the brightness of the environment light",
            ),
        ];
        const NAMES: [&str; 12] = [
            "source",
            "src",
            "url",
            "uri",
            "path",
            "file",
            "environment",
            "model",
            "image",
            "video",
            "audio",
            "lut",
        ];
        const SUFFIXES: [&str; 5] = ["_source", "_src", "_url", "_uri", "_file"];
        for schema in NodeKind::schemas() {
            for property in &schema.properties {
                let looks_like_a_source = NAMES.contains(&property.id)
                    || SUFFIXES.iter().any(|suffix| property.id.ends_with(suffix));
                let excused = NOT_MEDIA
                    .iter()
                    .any(|(kind, id, _)| *kind == schema.kind && *id == property.id);
                assert!(
                    !looks_like_a_source || property.media.is_some() || excused,
                    "`{}.{}` looks like a media source: declare `#[media(...)]` on it, or add it to NOT_MEDIA with the reason it is not one",
                    schema.kind,
                    property.id
                );
                // The schema's `source_id` role and `#[media(...)]` mark the same params.
                assert_eq!(
                    property.constraints.role == Some("source_id"),
                    property.media.is_some(),
                    "`{}.{}` has one of `role = \"source_id\"` and `#[media(...)]` without the other",
                    schema.kind,
                    property.id
                );
            }
        }
    }

    #[test]
    fn a_template_name_too_long_to_repeat_is_rejected() {
        let document = |name: &str| {
            serde_json::json!({
                "timeline": { "duration_frames": 1 },
                "render_settings": { "width": 1, "height": 1 },
                "nodes": [],
                "connections": [],
                "compositions": { name: { "nodes": [], "connections": [] } },
            })
        };
        let error = crate::json::parse_value(&document(&"a".repeat(257)))
            .map(|_| ())
            .unwrap_err()
            .to_string();
        assert!(error.contains("257 bytes long; the most is 256"), "{error}");
    }
}
