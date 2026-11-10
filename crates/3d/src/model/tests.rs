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
    let pixels: Vec<u8> = (0..width * height).flat_map(|_| rgba).collect();
    png_of(width, height, &pixels)
}

/// A PNG of `width` x `height` RGBA8 `pixels`, rows top first.
fn png_of(width: u32, height: u32, pixels: &[u8]) -> Vec<u8> {
    use ::image::ImageEncoder;
    let mut out = Vec::new();
    ::image::codecs::png::PngEncoder::new(&mut out)
        .write_image(pixels, width, height, ::image::ExtendedColorType::Rgba8)
        .unwrap();
    out
}

/// A PNG chunk of `kind` holding `data`, whose checksum is right only when `valid`.
fn png_chunk(kind: &[u8; 4], data: &[u8], valid: bool) -> Vec<u8> {
    let mut chunk = (data.len() as u32).to_be_bytes().to_vec();
    chunk.extend(kind);
    chunk.extend(data);
    let crc = crc32(&chunk[4..]) ^ u32::from(!valid);
    chunk.extend(crc.to_be_bytes());
    chunk
}

/// `image`, a PNG, with its header's size replaced by `width` x `height`.
fn resized_header(mut image: Vec<u8>, width: u32, height: u32) -> Vec<u8> {
    image[16..24].copy_from_slice(&[width.to_be_bytes(), height.to_be_bytes()].concat());
    let crc = crc32(&image[12..29]);
    image[29..33].copy_from_slice(&crc.to_be_bytes());
    image
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
    let image = resized_header(png(1, 1, [0; 4]), 100_000, 100_000);
    let error = textured(Glb::triangle(), &image).read().unwrap_err();
    assert_eq!(
        error,
        ModelError::TooLarge(
            "the model has more pixels across one image than the 8192 it may".to_string()
        )
    );
}

#[test]
fn every_mip_level_counts_toward_the_texture_pixel_limit_before_decoding() {
    // Two 8192 x 8192 images are 134 M pixels, under the limit, and 179 M with their mips.
    let mut glb = textured(Glb::triangle(), &[]);
    let views =
        [0u8, 1].map(|colour| glb.view(&resized_header(png(1, 1, [colour; 4]), 8192, 8192)));
    glb.json["images"] = json!([{ "bufferView": views[0] }, { "bufferView": views[1] }]);
    glb.json["textures"] = json!([{ "source": 0 }, { "source": 1 }]);
    glb.json["materials"] = json!([{
        "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } },
        "emissiveTexture": { "index": 1 },
    }]);
    assert_eq!(
        glb.read().unwrap_err(),
        ModelError::TooLarge(
            "the model has more texture pixels than the 160000000 it may".to_string()
        )
    );
}

#[test]
fn images_sharing_bytes_decode_once() {
    // A hundred 1200 x 1200 textures would be past the texture pixel limit; one isn't.
    let mut glb = textured(Glb::triangle(), &png(1200, 1200, [0, 0, 255, 255]));
    let image = glb.json["images"][0].clone();
    glb.json["images"] = Value::Array(vec![image; 100]);
    glb.json["textures"] = (0..100).map(|source| json!({ "source": source })).collect();
    glb.json["materials"] = (0..100)
        .map(|index| json!({ "pbrMetallicRoughness": { "baseColorTexture": { "index": index } } }))
        .collect();
    let model = glb.read().unwrap();
    assert_eq!(model.textures.len(), 1);
    let maps: Vec<_> = model
        .scene
        .materials()
        .iter()
        .map(|m| m.maps.base_color.map(|map| map.texture))
        .collect();
    assert!(maps.iter().all(|&texture| texture == Some(0)));
}

#[test]
fn a_png_decodes_from_its_still_image_chunks_alone() {
    // Each chunk beside the still image has a bad checksum, which the decoder would refuse.
    let image = png(1, 1, [10, 20, 30, 255]);
    let (header, rest) = image.split_at(33);
    let iend = rest.len() - 12;
    let mut edited = header.to_vec();
    for kind in [b"iCCP", b"gAMA", b"sRGB", b"cICP", b"acTL", b"fcTL"] {
        edited.extend(png_chunk(kind, b"profile\0\0\x78\x9c", false));
    }
    edited.extend(&rest[..iend]);
    edited.extend(png_chunk(b"fdAT", &[0; 16], false));
    edited.extend(png_chunk(b"tEXt", b"Comment\0hello", false));
    edited.extend(&rest[iend..]);
    let model = textured(Glb::triangle(), &edited).read().unwrap();
    assert_eq!(model.textures[0].levels[0], [10, 20, 30, 255]);
}

#[test]
fn an_odd_sides_last_texel_counts_toward_its_mip() {
    let pixels = [[0, 0, 0, 255], [0, 0, 0, 255], [255, 255, 255, 255]].concat();
    let model = textured(Glb::triangle(), &png_of(3, 1, &pixels))
        .read()
        .unwrap();
    // A third of white in linear light is 156 in sRGB.
    assert_eq!(model.textures[0].levels[1], [156, 156, 156, 255]);
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
    glb.json["skins"][0] = json!({ "joints": [1] });
    assert_eq!(
        glb.read().unwrap_err(),
        ModelError::Invalid("a mesh uses a joint its skin lacks".to_string())
    );
}

#[test]
fn skins_read_identity_binds_when_absent_and_check_their_joints() {
    let mut absent = skinned();
    absent.json["skins"][0] = json!({ "joints": [1, 2] });
    let model = absent.read().unwrap();
    assert_eq!(
        model.scene.pose(None).joints,
        [Mat4::IDENTITY, Mat4::from_translation(Vec3::Y)]
    );

    let cases: [(&str, Edit); 3] = [
        (
            "a skin has 1 inverse bind matrices for its 2 joints",
            Box::new(|json| json["accessors"][4]["count"] = json!(1)),
        ),
        (
            "skin 0 uses node 1 as a joint, which isn't in the scene",
            Box::new(|json| json["scenes"] = json!([{ "nodes": [0] }])),
        ),
        (
            "a skin names a node the model lacks",
            Box::new(|json| json["skins"][0]["skeleton"] = json!(9)),
        ),
    ];
    for (expected, edit) in cases {
        let mut glb = skinned();
        edit(&mut glb.json);
        assert_eq!(
            glb.read().unwrap_err(),
            ModelError::Invalid(expected.to_string())
        );
    }
}

/// Every matrix `scene` poses at `seconds` into its first clip.
fn posed_matrices(scene: &ModelScene, seconds: f32) -> Vec<Mat4> {
    let pose = scene.pose(Some(Playback {
        clip: 0,
        seconds,
        looped: true,
    }));
    pose.primitives
        .iter()
        .map(|posed| posed.world)
        .chain(pose.joints)
        .collect()
}

#[test]
fn values_too_large_or_degenerate_never_reach_a_pose() {
    let cases: [(&str, Edit); 2] = [
        (
            "a material's factor isn't a finite number",
            Box::new(|json| json["materials"] = json!([{ "emissiveFactor": [1e39, 0, 0] }])),
        ),
        (
            "the model's JSON holds 4294967296, larger than any index or size in a model",
            Box::new(|json| json["nodes"][0]["mesh"] = json!(4_294_967_296u64)),
        ),
    ];
    for (expected, edit) in cases {
        assert_eq!(error_with(edit), ModelError::Invalid(expected.to_string()));
    }
    let mut normals = Glb::triangle();
    let normal = normals.accessor(FLOAT, "VEC3", 3, &floats(&[f32::NAN; 9]));
    normals.json["meshes"][0]["primitives"][0]["attributes"]["NORMAL"] = json!(normal);
    assert_eq!(
        normals.read().unwrap_err(),
        ModelError::Invalid("a vertex normal isn't a finite number".to_string())
    );

    // Positions near the largest float still give a finite centre and texture coordinates.
    let mut far = Glb::triangle();
    far.bin[..36].copy_from_slice(&floats(&[
        -3e38, -3e38, 0.0, 3e38, -3e38, 0.0, -3e38, 3e38, 0.0,
    ]));
    let model = far.read().unwrap();
    assert!(
        model.scene.pose(None).primitives[0]
            .primitive
            .center
            .is_finite()
    );
    assert!(
        model
            .geometry
            .vertices
            .iter()
            .all(|vertex| vertex.uv.iter().all(|uv| uv.is_finite()))
    );

    // A zero rotation key, linear and cubic, and scales whose product overflows.
    let mut glb = Glb::triangle();
    glb.json["nodes"] = json!([
        { "mesh": 0 },
        { "mesh": 0 },
        { "scale": [1e30, 1e30, 1e30], "children": [3] },
        { "mesh": 0, "scale": [1e30, 1e30, 1e30] },
    ]);
    glb.json["scenes"] = json!([{ "nodes": [0, 1, 2] }]);
    let times = glb.accessor(FLOAT, "SCALAR", 2, &floats(&[0.0, 1.0]));
    let zero = glb.accessor(FLOAT, "VEC4", 2, &floats(&[0.0; 8]));
    let zero_cubic = glb.accessor(FLOAT, "VEC4", 6, &floats(&[0.0; 24]));
    glb.json["animations"] = json!([{
        "samplers": [
            { "input": times, "output": zero },
            { "input": times, "output": zero_cubic, "interpolation": "CUBICSPLINE" },
        ],
        "channels": [
            { "sampler": 0, "target": { "node": 0, "path": "rotation" } },
            { "sampler": 1, "target": { "node": 1, "path": "rotation" } },
        ],
    }]);
    let scene = glb.read().unwrap().scene;
    for seconds in [0.0, 0.5, 1.0] {
        let matrices = posed_matrices(&scene, seconds);
        assert_eq!(matrices.len(), 2, "the overflowing primitive is left out");
        assert!(matrices.iter().all(Mat4::is_finite), "{matrices:?}");
    }
}

#[test]
fn a_nodes_matrix_is_kept_as_given_unless_it_is_animated() {
    let flat = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1];
    let mut still = Glb::triangle();
    still.json["nodes"][0]["matrix"] = json!(flat);
    let pose = still.read().unwrap().scene.pose(None);
    assert_eq!(
        pose.primitives[0].world,
        Mat4::from_scale(Vec3::new(1.0, 1.0, 0.0))
    );

    let mut animated = still;
    let times = animated.accessor(FLOAT, "SCALAR", 1, &floats(&[0.0]));
    let moves = animated.accessor(FLOAT, "VEC3", 1, &floats(&[0.0; 3]));
    animated.json["animations"] = json!([{
        "samplers": [{ "input": times, "output": moves }],
        "channels": [{ "sampler": 0, "target": { "node": 0, "path": "translation" } }],
    }]);
    assert_eq!(
        animated.read().unwrap_err(),
        ModelError::Invalid(
            "node 0 is animated, but its matrix doesn't come apart into a translation, rotation \
             and scale"
                .to_string()
        )
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
    let chunk_length = |length: u32| {
        let mut chunk = bytes.clone();
        chunk[12..16].copy_from_slice(&length.to_le_bytes());
        Model::from_glb(&chunk).unwrap_err()
    };
    let appended = |kind: &[u8; 4]| {
        let mut file = bytes.clone();
        file.extend(4u32.to_le_bytes());
        file.extend(kind);
        file.extend(b"{}  ");
        let length = file.len() as u32;
        file[8..12].copy_from_slice(&length.to_le_bytes());
        Model::from_glb(&file).unwrap_err()
    };
    for (error, expected) in [
        (
            chunk_length(0xffff_fffc),
            "a chunk reaches past the end of the file",
        ),
        (
            chunk_length(13),
            "a chunk's length isn't a multiple of four bytes",
        ),
        (appended(b"JSON"), "the file has two JSON chunks"),
        (appended(b"BIN\0"), "the file has two binary chunks"),
    ] {
        assert_eq!(error, ModelError::Invalid(expected.to_string()));
    }

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
    let cases: [(ModelError, Edit); 9] = [
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
            too_large("JSON values", "4000000"),
            Box::new(|json| json["extras"] = json!(vec![0; 4_000_001])),
        ),
        (
            too_large("materials", "10000"),
            Box::new(move |json| json["materials"] = empty_objects(10_001)),
        ),
        (
            too_large("channels in one animation", "100000"),
            Box::new(move |json| {
                json["animations"] = json!([{ "channels": empty_objects(100_001) }])
            }),
        ),
        (
            too_large("primitives", "100000"),
            Box::new(move |json| {
                json["meshes"] =
                    Value::Array(vec![json!({ "primitives": empty_objects(50_001) }); 2]);
            }),
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

    // A small file's accessors may decode to 16 MiB at most, however large its counts.
    let zeros = error_with(|json| {
        json["accessors"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "componentType": FLOAT, "type": "SCALAR", "count": 5_000_000 }));
        json["animations"] = json!([{
            "samplers": [{ "input": 2, "output": 2 }],
            "channels": [{ "sampler": 0, "target": { "node": 0, "path": "scale" } }],
        }]);
    });
    assert_eq!(
        zeros,
        ModelError::TooLarge(
            "the model's accessors decode to more than the 16777216 bytes a file of its size may"
                .to_string()
        )
    );
}

/// Where each number in `value` is, as JSON pointers.
fn number_paths(value: &Value, path: String, out: &mut Vec<String>) {
    match value {
        Value::Number(_) => out.push(path),
        Value::Array(items) => {
            for (index, item) in items.iter().enumerate() {
                number_paths(item, format!("{path}/{index}"), out);
            }
        }
        Value::Object(fields) => {
            for (key, item) in fields {
                number_paths(item, format!("{path}/{key}"), out);
            }
        }
        _ => {}
    }
}

#[test]
fn corrupted_files_return_without_panicking() {
    // Numbers at the edges of what counts, offsets, indices and component types may hold.
    let boundaries = [
        json!(0),
        json!(1),
        json!(-1),
        json!(3),
        json!(4),
        json!(255),
        json!(65_535),
        json!(4_294_967_295u64),
        json!(4_294_967_296u64),
        json!(9_007_199_254_740_993u64),
        json!(1e39),
        json!(0.5),
        json!(5120),
        json!(5125),
        json!(5126),
    ];
    let rich = textured(skinned(), &png(2, 2, [255, 0, 0, 255]));
    let mut seed = 0x2545_f491_u32;
    let mut next = || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        seed >> 8
    };
    // How many mutations end where: the pre-deserialisation scan, glTF deserialising, the reader
    // afterwards, or a model that reads.
    let mut split = [0usize; 4];
    for glb in [Glb::triangle(), rich] {
        let mut paths = Vec::new();
        number_paths(&glb.json, String::new(), &mut paths);
        for round in 0..1500 {
            let mut json = glb.json.clone();
            let mut bin = glb.bin.clone();
            for _ in 0..1 + next() % 2 {
                if round % 2 == 0 {
                    let path = &paths[next() as usize % paths.len()];
                    *json.pointer_mut(path).unwrap() =
                        boundaries[next() as usize % boundaries.len()].clone();
                } else {
                    let at = next() as usize % bin.len();
                    bin[at] = next() as u8;
                }
            }
            let corrupt = Glb { json, bin };
            match corrupt.read() {
                Err(ModelError::Invalid(message))
                    if message.starts_with("the model's JSON holds") =>
                {
                    split[0] += 1;
                }
                Err(ModelError::Invalid(message))
                    if message.starts_with("the model's JSON isn't valid glTF") =>
                {
                    split[1] += 1;
                }
                Err(_) => split[2] += 1,
                Ok(model) => {
                    split[3] += 1;
                    for seconds in [0.0, 0.3, 1.0, 7.5] {
                        assert!(
                            posed_matrices(&model.scene, seconds)
                                .iter()
                                .all(Mat4::is_finite)
                        );
                    }
                }
            }
        }
    }
    eprintln!(
        "mutations: {} scanned out, {} glTF parse errors, {} later errors, {} read",
        split[0], split[1], split[2], split[3]
    );
    assert!(split[2] > 300 && split[3] > 300, "{split:?}");
}
