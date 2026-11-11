//! `model_3d`'s vertex colours, second texture coordinates, texture transforms, morph targets,
//! lights and cameras, each drawn as what it stands for would be.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::scene::{CENTER, light, merge, model, ortho, render_3d_with};
use crate::support::{Case, Expect, Scene, TestMedia, check, gpu, render};

const FLOAT: u32 = 5126;
const UNSIGNED_BYTE: u32 = 5121;
const UNSIGNED_SHORT: u32 = 5123;

/// A glTF document and its binary chunk: one mesh, placed by node 0 of the default scene.
struct Gltf {
    json: Value,
    bin: Vec<u8>,
}

impl Gltf {
    fn new() -> Self {
        Self {
            json: json!({
                "asset": { "version": "2.0" },
                "extensionsUsed": ["KHR_materials_unlit"],
                "scene": 0,
                "scenes": [{ "nodes": [0] }],
                "nodes": [{ "mesh": 0 }],
                "meshes": [{ "primitives": [] }],
                "accessors": [],
                "bufferViews": [],
                "buffers": [{ "byteLength": 0 }],
            }),
            bin: Vec::new(),
        }
    }

    fn view(&mut self, bytes: &[u8]) -> usize {
        self.bin.resize(self.bin.len().next_multiple_of(4), 0);
        let views = self.json["bufferViews"].as_array_mut().unwrap();
        views.push(json!({ "buffer": 0, "byteOffset": self.bin.len(), "byteLength": bytes.len() }));
        self.bin.extend_from_slice(bytes);
        views.len() - 1
    }

    fn accessor(&mut self, component: u32, kind: &str, count: usize, bytes: &[u8]) -> usize {
        let view = self.view(bytes);
        let accessors = self.json["accessors"].as_array_mut().unwrap();
        accessors.push(json!({
            "bufferView": view, "componentType": component, "type": kind, "count": count
        }));
        accessors.len() - 1
    }

    fn floats(&mut self, kind: &str, values: &[f32]) -> usize {
        let per = match kind {
            "SCALAR" => 1,
            "VEC2" => 2,
            "VEC3" => 3,
            _ => 4,
        };
        let bytes: Vec<u8> = values.iter().flat_map(|v| v.to_le_bytes()).collect();
        self.accessor(FLOAT, kind, values.len() / per, &bytes)
    }

    /// Adds a square facing +Z with the left, bottom, right and top `edges`, showing the whole of
    /// its textures top left at top left, in `material`.
    fn quad(&mut self, edges: [f32; 4], material: usize) {
        let [left, bottom, right, top] = edges;
        let position = self.floats(
            "VEC3",
            &[
                left, top, 0.0, right, top, 0.0, left, bottom, 0.0, right, bottom, 0.0,
            ],
        );
        let normal = self.floats("VEC3", &[0.0, 0.0, 1.0].repeat(4));
        let uv = self.floats("VEC2", &[0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        let indices: Vec<u8> = [0u16, 2, 1, 1, 2, 3]
            .iter()
            .flat_map(|i| i.to_le_bytes())
            .collect();
        let indices = self.accessor(UNSIGNED_SHORT, "SCALAR", 6, &indices);
        self.json["meshes"][0]["primitives"]
            .as_array_mut()
            .unwrap()
            .push(json!({
                "attributes": { "POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv },
                "indices": indices,
                "material": material,
            }));
    }

    /// The last primitive added.
    fn primitive(&mut self) -> &mut Value {
        self.json["meshes"][0]["primitives"]
            .as_array_mut()
            .unwrap()
            .last_mut()
            .unwrap()
    }

    fn glb(&self) -> Vec<u8> {
        let mut document = self.json.clone();
        let mut bin = self.bin.clone();
        bin.resize(bin.len().next_multiple_of(4), 0);
        document["buffers"][0]["byteLength"] = json!(bin.len());
        let mut json = serde_json::to_vec(&document).unwrap();
        json.resize(json.len().next_multiple_of(4), b' ');
        let mut glb = b"glTF".to_vec();
        glb.extend(2u32.to_le_bytes());
        glb.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        glb.extend((json.len() as u32).to_le_bytes());
        glb.extend(b"JSON");
        glb.extend(json);
        glb.extend((bin.len() as u32).to_le_bytes());
        glb.extend(b"BIN\0");
        glb.extend(bin);
        glb
    }
}

/// A unit square at the origin in `material`.
fn square(materials: Value) -> Gltf {
    let mut gltf = Gltf::new();
    gltf.json["materials"] = materials;
    gltf.quad([-0.5, -0.5, 0.5, 0.5], 0);
    gltf
}

fn unlit(color: [f32; 4]) -> Value {
    json!({
        "pbrMetallicRoughness": { "baseColorFactor": color },
        "extensions": { "KHR_materials_unlit": {} },
    })
}

/// A rough dielectric, which shows how light falls on it.
fn matte() -> Value {
    json!({ "pbrMetallicRoughness": { "metallicFactor": 0, "roughnessFactor": 1 } })
}

/// `models`, each a `model_3d`'s params, seen by an orthographic camera 4 units tall, with `others`
/// added to the scene.
fn view(render: Value, models: &[Value], others: Vec<Value>) -> Scene {
    let mut nodes: Vec<Value> = models
        .iter()
        .enumerate()
        .map(|(index, params)| model(index as u32 + 1, params.clone()))
        .collect();
    nodes.push(ortho(20, json!({})));
    nodes.extend(others);
    let ids: Vec<u32> = nodes
        .iter()
        .map(|node| node["id"].as_u64().unwrap() as u32)
        .collect();
    nodes.push(merge(30, &ids, json!({})));
    render_3d_with(render, nodes)
}

fn source(name: &str) -> Value {
    json!({ "source": name })
}

/// An uncompressed PNG of `rgba`, `width` texels across.
fn png(width: u32, height: u32, rgba: &[u8]) -> Vec<u8> {
    let crc = |bytes: &[u8]| {
        let mut crc = !0u32;
        for &byte in bytes {
            crc ^= u32::from(byte);
            for _ in 0..8 {
                crc = (crc >> 1) ^ (0xedb8_8320 & (crc & 1).wrapping_neg());
            }
        }
        !crc
    };
    let chunk = |kind: &[u8; 4], data: &[u8]| {
        let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
        chunk.extend(kind);
        chunk.extend(data);
        chunk.extend(crc(&chunk[4..]).to_be_bytes());
        chunk
    };
    let mut raw = Vec::new();
    for row in rgba.chunks(width as usize * 4) {
        raw.push(0);
        raw.extend(row);
    }
    let (a, b) = raw.iter().fold((1u32, 0u32), |(a, b), &byte| {
        let a = (a + u32::from(byte)) % 65521;
        (a, (b + a) % 65521)
    });
    let mut zlib = vec![0x78, 0x01];
    let blocks: Vec<&[u8]> = raw.chunks(65535).collect();
    for (index, block) in blocks.iter().enumerate() {
        zlib.push(u8::from(index + 1 == blocks.len()));
        let length = block.len() as u16;
        zlib.extend(length.to_le_bytes());
        zlib.extend((!length).to_le_bytes());
        zlib.extend(*block);
    }
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut header = width.to_be_bytes().to_vec();
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]);
    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    png.extend(chunk(b"IHDR", &header));
    png.extend(chunk(b"IDAT", &zlib));
    png.extend(chunk(b"IEND", &[]));
    png
}

const RED: [u8; 4] = [255, 0, 0, 255];
const GREEN: [u8; 4] = [0, 255, 0, 255];
const BLUE: [u8; 4] = [0, 0, 255, 255];
const WHITE: [u8; 4] = [255, 255, 255, 255];

#[test]
fn vertex_colours_multiply_the_base_colour_in_linear_light() {
    let coloured = |kind: &str, component: u32, bytes: &[u8], material: Value| {
        let mut gltf = square(json!([material]));
        let count = 4;
        let colors = gltf.accessor(component, kind, count, bytes);
        gltf.primitive()["attributes"]["COLOR_0"] = json!(colors);
        if component != FLOAT {
            gltf.json["accessors"][colors]["normalized"] = json!(true);
        }
        gltf.glb()
    };
    let grey = [128u8, 128, 128, 255].repeat(4);
    // 128 of 255 in linear light.
    let level = 128.0 / 255.0;
    let red: Vec<u8> = [1.0f32, 0.0, 0.0]
        .repeat(4)
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let half_alpha: Vec<u8> = [65535u16, 65535, 65535, 32768]
        .repeat(4)
        .iter()
        .flat_map(|v| v.to_le_bytes())
        .collect();
    let blended = json!({
        "pbrMetallicRoughness": { "baseColorFactor": [0, 1, 0, 1] },
        "alphaMode": "BLEND",
        "extensions": { "KHR_materials_unlit": {} },
    });
    let media = TestMedia::default()
        .model(
            "grey-unlit.glb",
            coloured("VEC4", UNSIGNED_BYTE, &grey, unlit([1.0; 4])),
        )
        .model(
            "grey-lit.glb",
            coloured("VEC4", UNSIGNED_BYTE, &grey, matte()),
        )
        .model("red.glb", coloured("VEC3", FLOAT, &red, unlit([1.0; 4])))
        .model(
            "half.glb",
            coloured("VEC4", UNSIGNED_SHORT, &half_alpha, blended),
        )
        .model(
            "factor-unlit.glb",
            square(json!([unlit([level, level, level, 1.0])])).glb(),
        )
        .model(
            "factor-lit.glb",
            square(json!([{ "pbrMetallicRoughness": {
                "baseColorFactor": [level, level, level, 1], "metallicFactor": 0,
                "roughnessFactor": 1 } }]))
            .glb(),
        );
    let plain = |name: &str| view(json!({}), &[source(name)], vec![]);
    check(vec![
        Case::new(
            "unlit, as a base colour factor of the same linear value",
            plain("grey-unlit.glb"),
            Expect::Matches(plain("factor-unlit.glb").into()),
        )
        .media(media.clone()),
        Case::new(
            "lit, as a base colour factor of the same linear value",
            plain("grey-lit.glb"),
            Expect::Matches(plain("factor-lit.glb").into()),
        )
        .media(media.clone()),
        Case::new(
            "three float components, with alpha 1",
            plain("red.glb"),
            Expect::Probes(vec![(CENTER, CENTER, RED)]),
        )
        .media(media.clone()),
        Case::new(
            "alpha blends a blended material",
            plain("half.glb"),
            Expect::Probes(vec![(CENTER, CENTER, [0, 128, 0, 128]), (2, 2, [0; 4])]),
        )
        .media(media),
    ]);
}

#[test]
fn maps_read_the_texture_coordinates_and_transforms_their_slots_name() {
    // Red, green, blue and white quarters, top left first.
    let texture = png(2, 2, &[RED, GREEN, BLUE, WHITE].concat());
    let card = |info: Value| {
        let mut gltf = square(json!([{
            "pbrMetallicRoughness": { "baseColorTexture": info },
            "extensions": { "KHR_materials_unlit": {} },
        }]));
        gltf.json["extensionsUsed"] = json!(["KHR_materials_unlit", "KHR_texture_transform"]);
        // Every vertex reads the bottom left quarter through its first set, and the top left
        // through its second.
        let uv0 = gltf.floats("VEC2", &[0.25, 0.75].repeat(4));
        let uv1 = gltf.floats("VEC2", &[0.25, 0.25].repeat(4));
        gltf.primitive()["attributes"]["TEXCOORD_0"] = json!(uv0);
        gltf.primitive()["attributes"]["TEXCOORD_1"] = json!(uv1);
        let image = gltf.view(&texture);
        gltf.json["images"] = json!([{ "bufferView": image, "mimeType": "image/png" }]);
        gltf.json["samplers"] = json!([{ "magFilter": 9728, "minFilter": 9728 }]);
        gltf.json["textures"] = json!([{ "source": 0, "sampler": 0 }]);
        gltf.glb()
    };
    let transform = |transform: Value| json!({ "index": 0, "extensions": { "KHR_texture_transform": transform } });
    let rows = [
        ("the first set", json!({ "index": 0 }), BLUE),
        ("the second set", json!({ "index": 0, "texCoord": 1 }), RED),
        (
            "an offset",
            transform(json!({ "offset": [0.5, -0.5] })),
            GREEN,
        ),
        (
            "a quarter turn and an offset",
            transform(json!({ "rotation": std::f32::consts::FRAC_PI_2, "offset": [0, 1] })),
            WHITE,
        ),
        ("a scale", transform(json!({ "scale": [3, 1] })), WHITE),
        (
            "the transform's own set",
            transform(json!({ "texCoord": 1 })),
            RED,
        ),
    ];
    check(
        rows.into_iter()
            .map(|(name, info, color)| {
                Case::new(
                    name,
                    view(json!({}), &[source("card.glb")], vec![]),
                    Expect::Probes(vec![(CENTER, CENTER, color), (26, 38, color)]),
                )
                .media(TestMedia::default().model("card.glb", card(info)))
            })
            .collect(),
    );
}

/// A unit square whose one morph target moves it 1 along +X, with `weights` for its mesh, and a
/// clip `move` taking its weight from 0 to 1 over a second.
fn sliding(weights: Option<f32>, material: Value) -> Gltf {
    let mut gltf = square(json!([material]));
    let along = gltf.floats("VEC3", &[1.0, 0.0, 0.0].repeat(4));
    gltf.primitive()["targets"] = json!([{ "POSITION": along }]);
    if let Some(weight) = weights {
        gltf.json["meshes"][0]["weights"] = json!([weight]);
    }
    let times = gltf.floats("SCALAR", &[0.0, 1.0]);
    let values = gltf.floats("SCALAR", &[0.0, 1.0]);
    gltf.json["animations"] = json!([{
        "name": "move",
        "samplers": [{ "input": times, "output": values }],
        "channels": [{ "sampler": 0, "target": { "node": 0, "path": "weights" } }],
    }]);
    gltf
}

/// A unit square in `material` moved by `offset`.
fn square_at(offset: [f32; 2], material: Value) -> Vec<u8> {
    let mut gltf = Gltf::new();
    gltf.json["materials"] = json!([material]);
    let [x, y] = offset;
    gltf.quad([x - 0.5, y - 0.5, x + 0.5, y + 0.5], 0);
    gltf.glb()
}

#[test]
fn morph_targets_move_vertices_by_their_weights() {
    let green = unlit([0.0, 1.0, 0.0, 1.0]);
    let mut own = sliding(Some(0.0), green.clone());
    own.json["nodes"][0]["weights"] = json!([0.5]);
    // Skinned by one joint a unit up, after the morph moves it.
    let mut skinned = sliding(Some(1.0), green.clone());
    let joints = skinned.accessor(UNSIGNED_BYTE, "VEC4", 4, &[0u8; 16]);
    let weights = skinned.floats("VEC4", &[1.0, 0.0, 0.0, 0.0].repeat(4));
    skinned.primitive()["attributes"]["JOINTS_0"] = json!(joints);
    skinned.primitive()["attributes"]["WEIGHTS_0"] = json!(weights);
    skinned.json["skins"] = json!([{ "joints": [1] }]);
    skinned.json["nodes"] = json!([{ "mesh": 0, "skin": 0 }, { "translation": [0, 1, 0] }]);
    skinned.json["scenes"] = json!([{ "nodes": [0, 1] }]);
    let media = TestMedia::default()
        .model("rest.glb", sliding(Some(0.0), green.clone()).glb())
        .model("moved.glb", sliding(Some(1.0), green.clone()).glb())
        .model("unweighted.glb", sliding(None, green.clone()).glb())
        .model("own.glb", own.glb())
        .model("skinned.glb", skinned.glb())
        .model("at-0.glb", square_at([0.0, 0.0], green.clone()))
        .model("at-half.glb", square_at([0.5, 0.0], green.clone()))
        .model("at-1.glb", square_at([1.0, 0.0], green.clone()))
        .model("at-1-1.glb", square_at([1.0, 1.0], green));
    let plain = |params: Value| view(json!({}), &[params], vec![]).frames(4).fps(4.0);
    let case = |name: &str, params: Value, reference: &str| {
        Case::new(
            name,
            plain(params),
            Expect::Matches(plain(source(reference)).into()),
        )
        .media(media.clone())
    };
    check(vec![
        case("at weight 0, at rest", source("rest.glb"), "at-0.glb"),
        case(
            "without weights, at rest",
            source("unweighted.glb"),
            "at-0.glb",
        ),
        case(
            "at its mesh's weight 1, moved",
            source("moved.glb"),
            "at-1.glb",
        ),
        case("at its node's own weight", source("own.glb"), "at-half.glb"),
        case(
            "half way through its clip",
            json!({ "source": "rest.glb", "animation": "move", "animation_loop": false }),
            "at-half.glb",
        )
        .frames([2.0]),
        case(
            "past the end of its clip",
            json!({ "source": "rest.glb", "animation": "move", "animation_loop": false,
                    "animation_offset": 5 }),
            "at-1.glb",
        ),
        case(
            "morphed, then moved by its joint",
            source("skinned.glb"),
            "at-1-1.glb",
        ),
    ]);
}

#[test]
fn morphed_normals_light_a_surface_as_vertex_normals_tilted_the_same_way_do() {
    let lean = std::f32::consts::FRAC_1_SQRT_2;
    let mut morphed = square(json!([matte()]));
    let tilt = morphed.floats("VEC3", &[lean, 0.0, lean - 1.0].repeat(4));
    morphed.primitive()["targets"] = json!([{ "NORMAL": tilt }]);
    morphed.json["meshes"][0]["weights"] = json!([1.0]);
    let mut tilted = square(json!([matte()]));
    let normal = tilted.floats("VEC3", &[lean, 0.0, lean].repeat(4));
    tilted.primitive()["attributes"]["NORMAL"] = json!(normal);
    let media = TestMedia::default()
        .model("morphed.glb", morphed.glb())
        .model("tilted.glb", tilted.glb())
        .model("flat.glb", square(json!([matte()])).glb());
    let lit = |name: &str| {
        view(
            json!({ "environment": 0 }),
            &[source(name)],
            vec![light(10, json!({ "rotation": [0, 40, 0] }))],
        )
    };
    let flat = crate::scene::frame_of(lit("flat.glb"), &media);
    check(vec![
        Case::new(
            "the morphed normal",
            lit("morphed.glb"),
            Expect::Matches(lit("tilted.glb").into()),
        )
        .media(media.clone()),
        Case::new(
            "and the light tells it from the unmorphed one",
            lit("morphed.glb"),
            Expect::Custom(Box::new(move |frames| {
                let (morphed, flat) = (frames[0].pixel(CENTER, CENTER), flat.pixel(CENTER, CENTER));
                assert!(
                    morphed[0].abs_diff(flat[0]) > 20,
                    "morphed {morphed:?}, flat {flat:?}"
                );
            })),
        )
        .media(media),
    ]);
}

#[test]
fn morphed_normals_of_different_lengths_and_directions_light_as_their_unit_normals_do() {
    // At weight 0.5 the left edge's normal is (1, 0, 1) and the right's (0, 0, 1).
    let mut morphed = square(json!([matte()]));
    let left = [2.0, 0.0, 0.0];
    let right = [0.0, 0.0, 0.0];
    let tilt = morphed.floats("VEC3", &[left, right, left, right].concat());
    morphed.primitive()["targets"] = json!([{ "NORMAL": tilt }]);
    morphed.json["meshes"][0]["weights"] = json!([0.5]);
    let lean = std::f32::consts::FRAC_1_SQRT_2;
    let mut unit = square(json!([matte()]));
    let (left, right) = ([lean, 0.0, lean], [0.0, 0.0, 1.0]);
    let normals = unit.floats("VEC3", &[left, right, left, right].concat());
    unit.primitive()["attributes"]["NORMAL"] = json!(normals);
    let media = TestMedia::default()
        .model("morphed.glb", morphed.glb())
        .model("unit.glb", unit.glb());
    let lit = |name: &str| {
        view(
            json!({ "environment": 0 }),
            &[source(name)],
            vec![light(10, json!({ "rotation": [0, 60, 0] }))],
        )
    };
    check(vec![
        Case::new(
            "the morphed normals",
            lit("morphed.glb"),
            Expect::Matches(lit("unit.glb").into()),
        )
        .media(media),
    ]);
}

/// The nodes of a ground 6 units across, seen from straight above by a camera that leaves out
/// everything over it, lit from straight above by a light casting sharp shadows, with `model`.
fn shadowed_ground(model: Value) -> Scene {
    crate::support::scene(
        crate::scene::SIZE,
        json!([
            { "id": 50, "type": "shape_3d", "params": {
                "shape": "plane", "size": [6, 1, 6], "roughness": 1 } },
            { "id": 51, "type": "camera_3d", "params": {
                "projection": "orthographic", "ortho_height": 6, "position": [0, 4, 0],
                "rotation": [-90, 0, 0], "near": 3.95, "far": 10 } },
            { "id": 60, "type": "light_3d", "params": {
                "light": "directional", "rotation": [-90, 0, 0], "cast_shadows": true,
                "shadow_softness": 0 } },
            model,
            { "id": 90, "type": "merge_3d", "in": { "objects": [50, 51, 60, 1] } },
            { "id": 100, "type": "render_3d", "params": { "environment": 0 },
              "in": { "scene": 90 } },
        ]),
    )
}

/// A level square half a unit up, which its one morph target raises 1 towards the light, and a
/// clip `move` taking the target's weight from 0 to 4 over a second, with 2 for the mesh.
fn rising_square() -> Gltf {
    let mut morphed = square(json!([matte()]));
    level(&mut morphed, 0.5);
    let along = morphed.floats("VEC3", &[0.0, 0.0, 1.0].repeat(4));
    morphed.primitive()["targets"] = json!([{ "POSITION": along }]);
    morphed.json["meshes"][0]["weights"] = json!([2.0]);
    let times = morphed.floats("SCALAR", &[0.0, 1.0]);
    let values = morphed.floats("SCALAR", &[0.0, 4.0]);
    morphed.json["animations"] = json!([{
        "name": "move",
        "samplers": [{ "input": times, "output": values }],
        "channels": [{ "sampler": 0, "target": { "node": 0, "path": "weights" } }],
    }]);
    morphed
}

/// Lays a square flat, `y` up.
fn level(gltf: &mut Gltf, y: f32) {
    gltf.json["nodes"][0]["translation"] = json!([0, y, 0]);
    gltf.json["nodes"][0]["rotation"] = json!([-0.70710677, 0, 0, 0.70710677]);
}

#[test]
fn a_model_weighted_past_1_casts_its_whole_shadow() {
    let morphed = rising_square();
    let mut moved = square(json!([matte()]));
    level(&mut moved, 2.5);
    let media = TestMedia::default()
        .model("morphed.glb", morphed.glb())
        .model("moved.glb", moved.glb());
    let ground = |params: Value| {
        shadowed_ground(json!({ "id": 1, "type": "model_3d", "params": params }))
            .frames(4)
            .fps(4.0)
    };
    let moved = || ground(source("moved.glb"));
    check(vec![
        Case::new(
            "at its mesh's weight 2",
            ground(source("morphed.glb")),
            Expect::Matches(moved().into()),
        )
        .media(media.clone()),
        Case::new(
            "half way through a clip to weight 4",
            ground(
                json!({ "source": "morphed.glb", "animation": "move", "animation_loop": false }),
            ),
            Expect::Matches(moved().into()),
        )
        .media(media)
        .frames([2.0]),
    ]);
}

#[test]
fn a_model_with_still_weights_is_kept_and_one_playing_a_weight_clip_is_drawn_each_frame() {
    let gpu = gpu();
    let media = TestMedia::default().model(
        "slide.glb",
        sliding(Some(1.0), unlit([0.0, 1.0, 0.0, 1.0])).glb(),
    );
    let frames = |params: Value| {
        let composition: Composition = view(json!({}), &[params], vec![]).frames(4).fps(4.0).into();
        let mut renderer = gpu.renderer();
        let frames: Vec<Vec<u8>> = (0..4)
            .map(|frame| render(&mut renderer, &composition, f64::from(frame), &media).rgba)
            .collect();
        (frames, renderer.skipped_pass_count())
    };
    let (still, skipped) = frames(source("slide.glb"));
    assert!(still.iter().all(|frame| *frame == still[0]));
    assert!(
        skipped >= 3 * 2,
        "a still model is kept, skipping {skipped} passes"
    );
    let (sliding, skipped) = frames(json!({ "source": "slide.glb", "animation": "move" }));
    assert_eq!(skipped, 0, "a model playing a clip renders every frame");
    for pair in sliding.windows(2) {
        assert!(pair[0] != pair[1], "the square slides");
    }
}

/// The `count` frames of `composition` at its frame rate, with the passes each recorded, on a
/// renderer that keeps what doesn't change, or on one that draws every frame in full.
fn run_frames(
    composition: &Composition,
    media: &TestMedia,
    count: u32,
    keeping: bool,
) -> (Vec<Vec<u8>>, Vec<usize>) {
    let gpu = gpu();
    let mut renderer = if keeping {
        gpu.renderer()
    } else {
        gpu.reference()
    };
    (0..count)
        .map(|frame| {
            let frame = render(&mut renderer, composition, f64::from(frame), media);
            (frame.rgba, renderer.recorded_pass_count())
        })
        .unzip()
}

/// Checks that a clip of one second at 4 frames a second, playing from frame 0, is drawn as it
/// is without keeping anything, and that from frame 6 on its last pose is kept: pixel for pixel
/// the frame it ended on, and recording no passes.
fn assert_the_last_pose_is_kept(composition: &Composition, media: &TestMedia) {
    let (kept, passes) = run_frames(composition, media, 10, true);
    let (drawn, _) = run_frames(composition, media, 10, false);
    for (frame, (kept, drawn)) in kept.iter().zip(&drawn).enumerate() {
        assert!(kept == drawn, "frame {frame} differs when kept");
    }
    for frame in 6..10 {
        assert!(kept[frame] == kept[4], "the pose holds at frame {frame}");
        assert_eq!(passes[frame], 0, "frame {frame} records {passes:?}");
    }
}

#[test]
fn a_morph_clip_that_has_ended_is_kept_as_its_weights_bounds_and_shadow_no_longer_change() {
    // The weight climbs to 4, which widens the model's bounds and so its shadow, and stays there.
    let media = TestMedia::default().model("morphed.glb", rising_square().glb());
    let ended = json!({ "source": "morphed.glb", "animation": "move", "animation_loop": false });
    let composition: Composition =
        shadowed_ground(json!({ "id": 1, "type": "model_3d", "params": ended }))
            .frames(10)
            .fps(4.0)
            .into();
    assert_the_last_pose_is_kept(&composition, &media);
}

#[test]
fn a_morph_clip_that_has_ended_is_kept_as_the_shape_its_weights_left() {
    // A square a quarter unit to the right for each unit of weight, which a clip takes from 0 to
    // 4: past its end the square is a unit to the right.
    let mut gltf = square(json!([unlit([0.0, 1.0, 0.0, 1.0])]));
    let along = gltf.floats("VEC3", &[0.25, 0.0, 0.0].repeat(4));
    gltf.primitive()["targets"] = json!([{ "POSITION": along }]);
    gltf.json["meshes"][0]["weights"] = json!([0.0]);
    let times = gltf.floats("SCALAR", &[0.0, 1.0]);
    let values = gltf.floats("SCALAR", &[0.0, 4.0]);
    gltf.json["animations"] = json!([{
        "name": "move",
        "samplers": [{ "input": times, "output": values }],
        "channels": [{ "sampler": 0, "target": { "node": 0, "path": "weights" } }],
    }]);
    let media = TestMedia::default().model("slide.glb", gltf.glb());
    let params = json!({ "source": "slide.glb", "animation": "move", "animation_loop": false });
    let composition: Composition = view(json!({}), &[params], vec![])
        .frames(10)
        .fps(4.0)
        .into();
    assert_the_last_pose_is_kept(&composition, &media);
    let (frames, _) = run_frames(&composition, &media, 10, true);
    assert!(
        frames[1] != frames[3],
        "the square slides while the clip plays"
    );
    assert!(
        frames[0] != frames[4],
        "and ends a unit from where it began"
    );
}

#[test]
fn a_clip_that_has_ended_keeps_the_lights_and_camera_it_moved() {
    // One node carries a point light and the camera, and a clip slides it 1 unit along X and back
    // 1 unit along Z over a second.
    let mut rig = square(json!([matte()]));
    rig.json["extensionsUsed"] = json!(["KHR_lights_punctual"]);
    rig.json["extensions"] = json!({ "KHR_lights_punctual": { "lights": [
        { "type": "point", "intensity": 20 }] } });
    rig.json["cameras"] = json!([{ "type": "perspective", "perspective": {
        "yfov": 50f64.to_radians(), "znear": 0.05 } }]);
    rig.json["nodes"] = json!([
        { "mesh": 0 },
        { "translation": [0, 0, 3], "camera": 0,
          "extensions": { "KHR_lights_punctual": { "light": 0 } } },
    ]);
    rig.json["scenes"] = json!([{ "nodes": [0, 1] }]);
    let times = rig.floats("SCALAR", &[0.0, 1.0]);
    let values = rig.floats("VEC3", &[0.0, 0.0, 3.0, 1.0, 0.0, 2.0]);
    rig.json["animations"] = json!([{
        "name": "move",
        "samplers": [{ "input": times, "output": values }],
        "channels": [{ "sampler": 0, "target": { "node": 1, "path": "translation" } }],
    }]);
    let media = TestMedia::default().model("rig.glb", rig.glb());
    let params = json!({ "source": "rig.glb", "animation": "move", "animation_loop": false,
                         "lights": true, "camera": "0" });
    let composition: Composition = view(json!({ "environment": 0 }), &[params], vec![])
        .frames(10)
        .fps(4.0)
        .into();
    assert_the_last_pose_is_kept(&composition, &media);
    let (frames, _) = run_frames(&composition, &media, 10, true);
    assert!(
        frames[1] != frames[3],
        "the light and camera move while the clip plays"
    );
}

/// A unit square facing +Z, and the model's `light` 2 units in front of it, facing it.
fn lamp(light: Value) -> Vec<u8> {
    let mut gltf = square(json!([matte()]));
    gltf.quad([-2.0, -2.0, 2.0, 2.0], 0);
    gltf.json["extensionsUsed"] = json!(["KHR_lights_punctual"]);
    gltf.json["extensions"] = json!({ "KHR_lights_punctual": { "lights": [light] } });
    gltf.json["nodes"] = json!([
        { "mesh": 0 },
        { "translation": [0, 0, 2], "extensions": { "KHR_lights_punctual": { "light": 0 } } },
    ]);
    gltf.json["scenes"] = json!([{ "nodes": [0, 1] }]);
    gltf.glb()
}

#[test]
fn a_models_lights_light_the_scene_in_the_units_the_extension_defines() {
    let media = TestMedia::default()
        .model(
            "point.glb",
            lamp(json!({ "type": "point", "intensity": 12 })),
        )
        .model(
            "spot.glb",
            lamp(
                json!({ "type": "spot", "intensity": 12, "color": [1, 0.5, 0.25],
                         "spot": { "innerConeAngle": 0.2, "outerConeAngle": 0.4 } }),
            ),
        )
        .model(
            "sun.glb",
            lamp(json!({ "type": "directional", "intensity": 2 })),
        );
    let lit = |name: &str, lights: bool, others: Vec<Value>| {
        view(
            json!({ "environment": 0 }),
            &[json!({ "source": name, "lights": lights })],
            others,
        )
    };
    let facing = |params: Value| {
        let mut params = params;
        params["rotation"] = json!([0, 0, 0]);
        light(10, params)
    };
    // 12 candela 2 metres away is 3 lux.
    let point = crate::scene::frame_of(lit("point.glb", true, vec![]), &media);
    let sun = crate::scene::frame_of(
        lit("point.glb", false, vec![facing(json!({ "intensity": 3 }))]),
        &media,
    );
    let unlit = crate::scene::frame_of(lit("point.glb", false, vec![]), &media);
    let (a, b) = (point.pixel(CENTER, CENTER), sun.pixel(CENTER, CENTER));
    assert!(
        (0..3).all(|c| a[c].abs_diff(b[c]) <= 1),
        "a point light of 12 cd at 2 m lights as 3 lux: {a:?}, {b:?}"
    );
    assert!(
        point.pixel(4, 4)[0] < a[0],
        "farther from the light is darker"
    );
    assert_ne!(
        unlit.pixel(CENTER, CENTER),
        a,
        "without `lights` the scene is lit as before"
    );
    let degrees = 0.8f64.to_degrees();
    check(vec![
        Case::new(
            "a spot light as a light_3d spot of the same cone",
            lit("spot.glb", true, vec![]),
            Expect::Matches(
                lit(
                    "spot.glb",
                    false,
                    vec![facing(json!({
                        "light": "spot", "intensity": 12, "position": [0, 0, 2],
                        "color": [255, 188, 137, 255], "spot_angle": degrees,
                        "spot_softness": 0.5,
                    }))],
                )
                .into(),
            ),
        )
        .media(media.clone())
        .tolerance(2),
        Case::new(
            "a directional light as a light_3d of the same intensity",
            lit("sun.glb", true, vec![]),
            Expect::Matches(lit("sun.glb", false, vec![facing(json!({ "intensity": 2 }))]).into()),
        )
        .media(media),
    ]);
}

#[test]
fn a_models_camera_views_the_scene_as_a_camera_3d_in_its_place_does() {
    let mut gltf = square(json!([matte()]));
    gltf.json["cameras"] = json!([
        { "type": "orthographic", "orthographic": { "xmag": 1, "ymag": 1, "znear": 0.1, "zfar": 50 } },
        { "type": "perspective", "name": "side",
          "perspective": { "yfov": 40f64.to_radians(), "znear": 0.05 } },
    ]);
    gltf.json["nodes"] = json!([
        { "mesh": 0 },
        { "translation": [1, 0.5, 5], "scale": [2, 2, 2], "camera": 1 },
    ]);
    gltf.json["scenes"] = json!([{ "nodes": [0, 1] }]);
    let media = TestMedia::default().model("set.glb", gltf.glb());
    // `before` comes before the model in the scene.
    let alone = |params: Value, before: Vec<Value>| {
        let mut nodes = before;
        nodes.push(model(1, params));
        let ids: Vec<u32> = nodes
            .iter()
            .map(|node| node["id"].as_u64().unwrap() as u32)
            .collect();
        nodes.push(merge(30, &ids, json!({})));
        render_3d_with(json!({}), nodes)
    };
    let camera_3d = json!({ "id": 2, "type": "camera_3d", "params": { "position": [1, 0.5, 5] } });
    let default_camera = json!({ "id": 2, "type": "camera_3d", "params": {} });
    check(vec![
        Case::new(
            "by name, without its node's scale",
            alone(json!({ "source": "set.glb", "camera": "side" }), vec![]),
            Expect::Matches(alone(source("set.glb"), vec![camera_3d.clone()]).into()),
        )
        .media(media.clone()),
        Case::new(
            "by index",
            alone(json!({ "source": "set.glb", "camera": "1" }), vec![]),
            Expect::Matches(alone(source("set.glb"), vec![camera_3d.clone()]).into()),
        )
        .media(media.clone()),
        Case::new(
            "after a camera_3d earlier in the scene, which wins",
            alone(
                json!({ "source": "set.glb", "camera": "side" }),
                vec![default_camera],
            ),
            Expect::Matches(alone(source("set.glb"), vec![]).into()),
        )
        .media(media),
    ]);
}

#[test]
fn points_and_lines_are_left_out_with_a_warning() {
    let mut gltf = square(json!([unlit([0.0, 1.0, 0.0, 1.0])]));
    let mut line = gltf.primitive().clone();
    line["mode"] = json!(1);
    gltf.json["meshes"][0]["primitives"]
        .as_array_mut()
        .unwrap()
        .push(line);
    let media = TestMedia::default().model("lines.glb", gltf.glb());
    let composition: Composition = view(json!({}), &[source("lines.glb")], vec![]).into();
    let mut renderer = gpu().renderer();
    let frame = render(&mut renderer, &composition, 0.0, &media);
    assert_eq!(frame.pixel(CENTER, CENTER), GREEN, "the triangles draw");
    let warnings = renderer.take_diagnostics();
    assert_eq!(warnings.len(), 1, "{warnings:?}");
    assert_eq!(warnings[0].code, "scene.model_primitives_skipped");
    assert_eq!(warnings[0].node, Some(lumen_engine::node::NodeId::new(1)));
    assert!(
        warnings[0]
            .message
            .contains("model `lines.glb` has primitives of points"),
        "{}",
        warnings[0].message
    );
}
