use glam::{Mat4, Quat, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Filter, Sampler, Wrap};

/// A glTF document and its binary chunk, assembled into a `.glb` by [`Glb::bytes`].
struct Glb {
    json: Value,
    bin: Vec<u8>,
}

fn floats(values: &[f32]) -> Vec<u8> {
    values
        .iter()
        .flat_map(|value| value.to_le_bytes())
        .collect()
}

const FLOAT: u32 = 5126;
const UNSIGNED_SHORT: u32 = 5123;
const UNSIGNED_BYTE: u32 = 5121;

impl Glb {
    /// One triangle in the XY plane, placed by node 0 of the default scene.
    fn triangle() -> Self {
        let mut glb = Self {
            json: json!({
                "asset": { "version": "2.0" },
                "scene": 0,
                "scenes": [{ "nodes": [0] }],
                "nodes": [{ "mesh": 0 }],
                "meshes": [{ "primitives": [{ "attributes": {} }] }],
                "accessors": [],
                "bufferViews": [],
                "buffers": [{ "byteLength": 0 }],
            }),
            bin: Vec::new(),
        };
        let positions = glb.accessor(
            FLOAT,
            "VEC3",
            3,
            &floats(&[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0]),
        );
        let indices = glb.accessor(
            UNSIGNED_SHORT,
            "SCALAR",
            3,
            &[0u16, 1, 2].map(u16::to_le_bytes).concat(),
        );
        glb.json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] = json!(positions);
        glb.json["meshes"][0]["primitives"][0]["indices"] = json!(indices);
        glb
    }

    /// Appends `bytes` to the binary chunk as a new buffer view, returning its index.
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

    fn bytes(&self) -> Vec<u8> {
        let mut json = self.json.clone();
        json["buffers"][0]["byteLength"] = json!(self.bin.len());
        let mut json = serde_json::to_vec(&json).unwrap();
        json.resize(json.len().next_multiple_of(4), b' ');
        let mut bin = self.bin.clone();
        bin.resize(bin.len().next_multiple_of(4), 0);
        let mut out = b"glTF".to_vec();
        out.extend(2u32.to_le_bytes());
        out.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(b"JSON");
        out.extend(json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(b"BIN\0");
        out.extend(bin);
        out
    }

    fn read(&self) -> Result<Model, ModelError> {
        Model::from_glb(&self.bytes())
    }
}

fn close(a: Vec3, b: Vec3) -> bool {
    (a - b).length() < 1e-4
}

/// Where each primitive's origin lands when `scene` is posed with `playback`.
fn origins(scene: &ModelScene, playback: Option<Playback>) -> Vec<Vec3> {
    scene
        .pose(playback)
        .primitives
        .iter()
        .map(|posed| posed.world.transform_point3(Vec3::ZERO))
        .collect()
}

#[test]
fn a_triangle_reads_with_flat_normals_and_planar_uvs() {
    let model = Glb::triangle().read().expect("a valid glb");
    let geometry = &model.geometry;
    assert_eq!(
        (geometry.vertices.len(), &geometry.indices[..]),
        (3, &[0, 1, 2][..])
    );
    assert!(geometry.skin.is_empty());
    let uvs: Vec<[f32; 2]> = geometry.vertices.iter().map(|vertex| vertex.uv).collect();
    assert_eq!(uvs, [[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]]);
    let pose = model.scene.pose(None);
    assert_eq!(pose.primitives.len(), 1);
    let posed = pose.primitives[0];
    assert!(posed.primitive.flat_normals && !posed.primitive.skinned);
    assert_eq!(posed.world, Mat4::IDENTITY);
    assert_eq!(posed.primitive.material, None);
    assert!(close(posed.primitive.center, Vec3::new(0.5, 0.5, 0.0)));
}

#[test]
fn nodes_place_their_children_in_their_own_space() {
    let mut glb = Glb::triangle();
    glb.json["nodes"] = json!([
        { "translation": [1, 0, 0], "children": [1, 2] },
        { "mesh": 0, "rotation": [0, 0.70710677, 0, 0.70710677], "scale": [2, 2, 2] },
        { "mesh": 0, "matrix": [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 3, 0, 1] },
    ]);
    let model = glb.read().unwrap();
    let pose = model.scene.pose(None);
    // Scaled by 2 and turned a quarter about Y, +X lands on -Z.
    let turned = pose.primitives[0].world.transform_point3(Vec3::X);
    assert!(close(turned, Vec3::new(1.0, 0.0, -2.0)), "{turned}");
    let moved = pose.primitives[1].world.transform_point3(Vec3::ZERO);
    assert!(close(moved, Vec3::new(1.0, 3.0, 0.0)), "{moved}");
}

#[test]
fn materials_read_each_alpha_mode() {
    let mut glb = Glb::triangle();
    glb.json["materials"] = json!([
        { "name": "solid" },
        { "name": "cut", "alphaMode": "MASK", "alphaCutoff": 0.3 },
        { "name": "cut at half", "alphaMode": "MASK" },
        { "name": "glass", "alphaMode": "BLEND", "doubleSided": true,
          "pbrMetallicRoughness": { "baseColorFactor": [1, 1, 1, 0.5] } },
        { "name": "lamp", "emissiveFactor": [1, 0.5, 0],
          "extensions": { "KHR_materials_unlit": {},
                          "KHR_materials_emissive_strength": { "emissiveStrength": 4 } } },
    ]);
    glb.json["extensionsUsed"] = json!(["KHR_materials_unlit", "KHR_materials_emissive_strength"]);
    glb.json["meshes"][0]["primitives"][0]["material"] = json!(3);
    let model = glb.read().unwrap();
    let scene = &model.scene;
    let blending: Vec<Blending> = scene.materials().iter().map(|m| m.blending).collect();
    assert_eq!(
        blending,
        [
            Blending::Opaque,
            Blending::Mask { cutoff: 0.3 },
            Blending::Mask { cutoff: 0.5 },
            Blending::Blend,
            Blending::Opaque,
        ]
    );
    let glass = &scene.materials()[3];
    assert!(glass.double_sided && glass.base_color[3] == 0.5);
    let lamp = &scene.materials()[4];
    assert!(lamp.unlit && lamp.emissive == [4.0, 2.0, 0.0]);
    assert_eq!(scene.pose(None).primitives[0].primitive.material, Some(3));

    assert_eq!(scene.material("glass"), Ok(3));
    assert_eq!(
        scene.material("screen"),
        Err(
            "the model has no material `screen`; it has `solid`, `cut`, `cut at half`, `glass`, `lamp`"
                .to_string()
        )
    );
}

/// A PNG of `width` x `height` pixels of one colour.
fn png(width: u32, height: u32, rgba: [u8; 4]) -> Vec<u8> {
    use ::image::ImageEncoder;
    let pixels: Vec<u8> = (0..width * height).flat_map(|_| rgba).collect();
    let mut out = Vec::new();
    ::image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&pixels, width, height, ::image::ExtendedColorType::Rgba8)
        .unwrap();
    out
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = !0u32;
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = if crc & 1 == 1 {
                (crc >> 1) ^ 0xedb8_8320
            } else {
                crc >> 1
            };
        }
    }
    !crc
}

/// `glb` with a material whose base colour and normal maps show `image`, the normal map through
/// the second UV set.
fn textured(mut glb: Glb, image: &[u8]) -> Glb {
    let view = glb.view(image);
    glb.json["images"] = json!([{ "bufferView": view, "mimeType": "image/png" }]);
    glb.json["samplers"] =
        json!([{ "wrapS": 33071, "wrapT": 33648, "magFilter": 9728, "minFilter": 9729 }]);
    glb.json["textures"] = json!([{ "source": 0, "sampler": 0 }]);
    glb.json["materials"] = json!([{
        "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } },
        "normalTexture": { "index": 0, "texCoord": 1 },
        "emissiveTexture": { "index": 0, "extensions": { "KHR_texture_transform": { "scale": [2, 2] } } },
    }]);
    glb.json["extensionsUsed"] = json!(["KHR_texture_transform"]);
    glb
}

#[test]
fn textures_decode_with_mips_in_the_colour_space_of_their_slots() {
    let model = textured(Glb::triangle(), &png(4, 2, [255, 128, 0, 255]))
        .read()
        .unwrap();
    let material = &model.scene.materials()[0];
    let base = material.maps.base_color.expect("a base colour map");
    assert_eq!(
        base.sampler,
        Sampler {
            wrap_u: Wrap::ClampToEdge,
            wrap_v: Wrap::MirroredRepeat,
            mag: Filter::Nearest,
            min: Filter::Linear,
            mipmap: None,
        }
    );
    // The second UV set isn't read, so its map is left out; the optional texture transform is
    // ignored and the emissive map shares the base colour's sRGB texture.
    assert_eq!(material.maps.normal, None);
    assert_eq!(
        material.maps.emissive.map(|map| map.texture),
        Some(base.texture)
    );
    assert_eq!(model.textures.len(), 1);
    let texture = &model.textures[0];
    assert!(texture.srgb);
    let sizes: Vec<usize> = texture.levels.iter().map(Vec::len).collect();
    assert_eq!(sizes, [4 * 2 * 4, 2 * 4, 4]);
    assert_eq!(texture.levels[2], [255, 128, 0, 255]);
}

#[test]
fn an_image_declaring_a_huge_size_is_rejected_before_decoding() {
    let mut image = png(1, 1, [0; 4]);
    image[16..24].copy_from_slice(&[100_000u32.to_be_bytes(), 100_000u32.to_be_bytes()].concat());
    let crc = crc32(&image[12..29]);
    image[29..33].copy_from_slice(&crc.to_be_bytes());
    let error = textured(Glb::triangle(), &image).read().unwrap_err();
    assert_eq!(
        error,
        ModelError::TooLarge(
            "the model has more pixels across one image than the 8192 it may".to_string()
        )
    );
}

/// A triangle placed by three nodes, each moved along X by one clip's channel with `interpolation`
/// over keys at 0, 1 and 2 seconds: 0, 10 and 20 for the step and linear channels, and for the
/// cubic one the same values with an out-tangent of 4 at the first key and others of 0.
fn animated() -> ModelScene {
    let mut glb = Glb::triangle();
    glb.json["nodes"] = json!([{ "mesh": 0 }, { "mesh": 0 }, { "mesh": 0 }]);
    glb.json["scenes"] = json!([{ "nodes": [0, 1, 2] }]);
    let times = glb.accessor(FLOAT, "SCALAR", 3, &floats(&[0.0, 1.0, 2.0]));
    let x = |values: &[f32]| -> Vec<f32> { values.iter().flat_map(|&x| [x, 0.0, 0.0]).collect() };
    let linear = glb.accessor(FLOAT, "VEC3", 3, &floats(&x(&[0.0, 10.0, 20.0])));
    let cubic = glb.accessor(
        FLOAT,
        "VEC3",
        9,
        &floats(&x(&[0.0, 0.0, 4.0, 0.0, 10.0, 0.0, 0.0, 20.0, 0.0])),
    );
    let sampler = |interpolation: &str, output| json!({ "input": times, "output": output, "interpolation": interpolation });
    let channel =
        |node: usize| json!({ "sampler": node, "target": { "node": node, "path": "translation" } });
    glb.json["animations"] = json!([{
        "name": "slide",
        "samplers": [sampler("STEP", linear), sampler("LINEAR", linear), sampler("CUBICSPLINE", cubic)],
        "channels": [channel(0), channel(1), channel(2)],
    }]);
    glb.read().unwrap().scene
}

#[test]
fn clips_sample_each_interpolation_at_and_between_keys_and_past_the_end() {
    let scene = animated();
    let clip = scene.animation("slide").unwrap().unwrap();
    let at = |seconds: f32, looped: bool| -> Vec<f32> {
        origins(
            &scene,
            Some(Playback {
                clip,
                seconds,
                looped,
            }),
        )
        .iter()
        .map(|origin| origin.x)
        .collect()
    };
    let near = |got: Vec<f32>, expected: [f32; 3]| {
        let close = got
            .iter()
            .zip(expected)
            .all(|(got, expected)| (got - expected).abs() < 1e-4);
        assert!(close, "{got:?}, not {expected:?}");
    };
    near(at(0.0, false), [0.0, 0.0, 0.0]);
    near(at(1.0, false), [10.0, 10.0, 10.0]);
    // Halfway, the cubic channel's out-tangent adds 4 x (t^3 - 2t^2 + t) = 0.5.
    near(at(0.5, false), [0.0, 5.0, 5.5]);
    // A quarter of the way to the next key, Hermite easing has moved 0.15625 of the way.
    near(at(1.25, false), [10.0, 12.5, 11.5625]);
    near(at(3.0, false), [20.0, 20.0, 20.0]);
    near(at(-1.0, false), [0.0, 0.0, 0.0]);
    // Looped, 2.5 seconds into a 2-second clip is 0.5 seconds in.
    near(at(2.5, true), [0.0, 5.0, 5.5]);
    near(at(f32::NAN, false), [0.0, 0.0, 0.0]);
    // Without a clip, nodes rest where they are.
    assert_eq!(origins(&scene, None), [Vec3::ZERO; 3]);
}

#[test]
fn clips_are_found_by_name_or_index() {
    let scene = animated();
    assert_eq!(scene.animation(""), Ok(None));
    assert_eq!(scene.animation("slide"), Ok(Some(0)));
    assert_eq!(scene.animation("0"), Ok(Some(0)));
    assert_eq!(
        scene.animation("1"),
        Err("the model has no animation `1`; it has 0 `slide`".to_string())
    );
}

/// A triangle skinned to a chain of two joints a unit apart along Y, with a clip that moves the
/// first joint 2 along X at 1 second and turns the second a quarter about Z.
fn skinned() -> Glb {
    let mut glb = Glb::triangle();
    glb.json["nodes"] = json!([
        { "mesh": 0, "skin": 0, "translation": [5, 5, 5] },
        { "children": [2] },
        { "translation": [0, 1, 0] },
    ]);
    glb.json["scenes"] = json!([{ "nodes": [0, 1] }]);
    let joints = glb.accessor(
        UNSIGNED_BYTE,
        "VEC4",
        3,
        &[0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0],
    );
    let weights = glb.accessor(
        FLOAT,
        "VEC4",
        3,
        &floats(&[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0]),
    );
    let attributes = &mut glb.json["meshes"][0]["primitives"][0]["attributes"];
    attributes["JOINTS_0"] = json!(joints);
    attributes["WEIGHTS_0"] = json!(weights);
    let inverse_binds = [Mat4::IDENTITY, Mat4::from_translation(Vec3::NEG_Y)]
        .map(|matrix| floats(&matrix.to_cols_array()))
        .concat();
    let inverse_binds = glb.accessor(FLOAT, "MAT4", 2, &inverse_binds);
    glb.json["skins"] = json!([{ "joints": [1, 2], "inverseBindMatrices": inverse_binds }]);
    let times = glb.accessor(FLOAT, "SCALAR", 2, &floats(&[0.0, 1.0]));
    let moves = glb.accessor(FLOAT, "VEC3", 2, &floats(&[0.0, 0.0, 0.0, 2.0, 0.0, 0.0]));
    let turn = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
    let turns = glb.accessor(
        FLOAT,
        "VEC4",
        2,
        &floats(&[Quat::IDENTITY.to_array(), turn.to_array()].concat()),
    );
    glb.json["animations"] = json!([{
        "samplers": [{ "input": times, "output": moves }, { "input": times, "output": turns }],
        "channels": [
            { "sampler": 0, "target": { "node": 1, "path": "translation" } },
            { "sampler": 1, "target": { "node": 2, "path": "rotation" } },
        ],
    }]);
    glb
}

#[test]
fn a_skinned_primitive_follows_its_joints_and_ignores_its_nodes_transform() {
    let model = skinned().read().unwrap();
    let skin: Vec<SkinVertex> = model.geometry.skin.clone();
    assert_eq!(skin.len(), 3);
    assert_eq!(
        (skin[2].joints, skin[2].weights[..2].to_vec()),
        ([1, 0, 0, 0], vec![32768, 32768])
    );
    let scene = &model.scene;
    let rest = scene.pose(None);
    assert_eq!(rest.primitives[0].first_joint, Some(0));
    assert!(rest.primitives[0].primitive.skinned);
    assert!(
        rest.joints
            .iter()
            .all(|joint| joint.abs_diff_eq(Mat4::IDENTITY, 1e-6))
    );

    let clip = scene.animation("0").unwrap().unwrap();
    let posed = scene.pose(Some(Playback {
        clip,
        seconds: 1.0,
        looped: false,
    }));
    // A point a unit past the second joint turns about it to sit a unit along -X of it, and the
    // whole chain has moved 2 along X.
    let tip = posed.joints[1].transform_point3(Vec3::new(0.0, 2.0, 0.0));
    assert!(close(tip, Vec3::new(1.0, 1.0, 0.0)), "{tip}");
    let root = posed.joints[0].transform_point3(Vec3::ZERO);
    assert!(close(root, Vec3::new(2.0, 0.0, 0.0)), "{root}");
}

#[test]
fn a_skinned_vertex_leaning_on_a_joint_its_skin_lacks_is_an_error() {
    let mut glb = skinned();
    glb.json["skins"][0]["joints"] = json!([1]);
    assert_eq!(
        glb.read().unwrap_err(),
        ModelError::Invalid("a mesh uses a joint its skin lacks".to_string())
    );
}

#[test]
fn models_needing_what_this_reader_lacks_are_rejected_naming_it() {
    let mut external = Glb::triangle();
    external.json["buffers"][0]["uri"] = json!("triangle.bin");
    let mut required = Glb::triangle();
    required.json["extensionsUsed"] = json!(["KHR_draco_mesh_compression"]);
    required.json["extensionsRequired"] = json!(["KHR_draco_mesh_compression"]);
    let mut optional = Glb::triangle();
    optional.json["extensionsUsed"] = json!(["KHR_draco_mesh_compression"]);
    for (glb, expected) in [
        (
            external,
            "the model refers to a file outside itself, `triangle.bin`; pack everything into one .glb",
        ),
        (
            required,
            "the model requires the glTF extension KHR_draco_mesh_compression, which isn't supported",
        ),
    ] {
        assert_eq!(
            glb.read().unwrap_err(),
            ModelError::Unsupported(expected.to_string())
        );
    }
    optional
        .read()
        .expect("an optional extension is left unread");
}

type Edit = Box<dyn FnOnce(&mut Value)>;

/// The error [`Glb::triangle`] gives once `edit` changes its JSON.
fn error_with(edit: impl FnOnce(&mut Value)) -> ModelError {
    let mut glb = Glb::triangle();
    edit(&mut glb.json);
    glb.read().unwrap_err()
}

#[test]
fn malformed_containers_and_out_of_range_data_are_errors() {
    let bytes = Glb::triangle().bytes();
    for length in 0..bytes.len() {
        assert!(
            Model::from_glb(&bytes[..length]).is_err(),
            "truncated to {length}"
        );
    }
    let mut magic = bytes.clone();
    magic[0] = b'x';
    assert!(matches!(
        Model::from_glb(&magic),
        Err(ModelError::Invalid(_))
    ));
    let mut chunk = bytes.clone();
    chunk[12..16].copy_from_slice(&u32::MAX.to_le_bytes());
    assert_eq!(
        Model::from_glb(&chunk).unwrap_err(),
        ModelError::Invalid("a chunk reaches past the end of the file".to_string())
    );

    let cases: [(&str, Edit); 7] = [
        (
            "an accessor reaches past the end of its buffer view",
            Box::new(|json| json["accessors"][0]["byteOffset"] = json!(4)),
        ),
        (
            "a buffer view reaches past the end of its buffer",
            Box::new(|json| json["bufferViews"][1]["byteLength"] = json!(1000)),
        ),
        (
            "a primitive has an index past its vertices",
            Box::new(|json| json["accessors"][0]["count"] = json!(2)),
        ),
        (
            "an accessor index is out of range",
            Box::new(|json| json["meshes"][0]["primitives"][0]["indices"] = json!(7)),
        ),
        (
            "the model's nodes don't form a tree: a node has two parents, or is its own ancestor",
            Box::new(|json| {
                json["nodes"] = json!([{ "mesh": 0, "children": [1] }, { "children": [0] }]);
            }),
        ),
        (
            "the model's nodes don't form a tree: a node has two parents, or is its own ancestor",
            Box::new(|json| json["nodes"] = json!([{ "children": [1, 1] }, { "mesh": 0 }])),
        ),
        (
            "node 0 has a child the model lacks",
            Box::new(|json| json["nodes"][0]["children"] = json!([9])),
        ),
    ];
    for (expected, edit) in cases {
        assert_eq!(error_with(edit), ModelError::Invalid(expected.to_string()));
    }
}

#[test]
fn counts_past_each_limit_are_errors_before_anything_that_size_is_read() {
    let empty_objects = |count: usize| Value::Array(vec![json!({}); count]);
    let too_large = |what: &str, limit: &str| {
        ModelError::TooLarge(format!("the model has more {what} than the {limit} it may"))
    };
    let cases: [(ModelError, Edit); 6] = [
        (
            too_large("vertices", "4000000"),
            Box::new(|json| json["accessors"][0]["count"] = json!(4_000_001)),
        ),
        (
            too_large("indices", "12000000"),
            Box::new(|json| json["accessors"][1]["count"] = json!(12_000_003)),
        ),
        (
            too_large("nodes", "100000"),
            Box::new(move |json| json["nodes"] = empty_objects(100_001)),
        ),
        (
            too_large("JSON objects and strings", "2000000"),
            Box::new(move |json| json["nodes"] = empty_objects(2_000_001)),
        ),
        (
            too_large("joints in one skin", "1024"),
            Box::new(|json| json["skins"] = json!([{ "joints": vec![0; 1025] }])),
        ),
        (
            too_large("primitives placed in its scene", "100000"),
            Box::new(|json| {
                let primitive = json["meshes"][0]["primitives"][0].clone();
                json["meshes"][0]["primitives"] = json!([primitive, primitive]);
                json["nodes"] = Value::Array(vec![json!({ "mesh": 0 }); 50_001]);
                json["scenes"] = json!([{ "nodes": (0..50_001).collect::<Vec<_>>() }]);
            }),
        ),
    ];
    for (expected, edit) in cases {
        assert_eq!(error_with(edit), expected);
    }
    let huge_input = error_with(|json| {
        json["animations"] = json!([{
            "samplers": [{ "input": 0, "output": 0 }],
            "channels": [{ "sampler": 0, "target": { "node": 0, "path": "translation" } }],
        }]);
        json["accessors"][0]["count"] = json!(3);
        json["accessors"].as_array_mut().unwrap().push(
            json!({ "bufferView": 0, "componentType": FLOAT, "type": "SCALAR", "count": 48_000_001u64 }),
        );
        json["animations"][0]["samplers"][0]["input"] = json!(2);
    });
    assert_eq!(
        huge_input,
        too_large("components in one accessor", "48000000")
    );
}

#[test]
fn corrupted_files_return_without_panicking() {
    let bytes = skinned().bytes();
    let mut seed = 0x2545_f491_u32;
    for _ in 0..3000 {
        let mut corrupt = bytes.clone();
        for _ in 0..3 {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let at = (seed >> 8) as usize % corrupt.len();
            corrupt[at] = (seed >> 24) as u8;
        }
        // A corrupt file may still be a readable model; only that it returns matters.
        if let Ok(model) = Model::from_glb(&corrupt) {
            model.scene.pose(Some(Playback {
                clip: 0,
                seconds: 0.5,
                looped: true,
            }));
        }
    }
}
