use glam::{Mat4, Quat, Vec3};
use serde_json::{Value, json};

use super::*;
use crate::{Filter, MapUv, Sampler, Wrap};

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
    /// The `.glb` in `bytes`, whose binary chunk, if any, follows its JSON.
    fn parse(bytes: &[u8]) -> Self {
        let length = u32::from_le_bytes(bytes[12..16].try_into().unwrap()) as usize;
        Self {
            json: serde_json::from_slice(&bytes[20..20 + length]).unwrap(),
            bin: bytes.get(28 + length..).unwrap_or_default().to_vec(),
        }
    }

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
        self.bytes_with(|json| json)
    }

    /// The `.glb`, with `edit` applied to the text of its JSON.
    fn bytes_with(&self, edit: impl FnOnce(String) -> String) -> Vec<u8> {
        let mut json = self.json.clone();
        json["buffers"][0]["byteLength"] = json!(self.bin.len());
        let mut json = edit(serde_json::to_string(&json).unwrap()).into_bytes();
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

/// `glb` with a material whose base colour, normal and emissive maps show `image`, the normal map
/// through a third UV set and the emissive map scaled by a texture transform.
fn textured(mut glb: Glb, image: &[u8]) -> Glb {
    let view = glb.view(image);
    glb.json["images"] = json!([{ "bufferView": view, "mimeType": "image/png" }]);
    glb.json["samplers"] =
        json!([{ "wrapS": 33071, "wrapT": 33648, "magFilter": 9728, "minFilter": 9729 }]);
    glb.json["textures"] = json!([{ "source": 0, "sampler": 0 }]);
    glb.json["materials"] = json!([{
        "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } },
        "normalTexture": { "index": 0, "texCoord": 2 },
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
    // A third UV set isn't read, so its map is left out; the emissive map shares the base
    // colour's sRGB texture, scaled by its transform.
    assert_eq!(material.maps.normal, None);
    assert_eq!(
        material.maps.emissive.map(|map| map.texture),
        Some(base.texture)
    );
    assert_eq!(material.map_uvs[0], MapUv::default());
    assert_eq!(material.map_uvs[3], MapUv::new(0, [0.0; 2], 0.0, [2.0; 2]));
    assert_eq!(model.textures.len(), 1);
    let texture = &model.textures[0];
    assert!(texture.srgb);
    let sizes: Vec<usize> = texture.levels.iter().map(Vec::len).collect();
    assert_eq!(sizes, [4 * 2 * 4, 2 * 4, 4]);
    assert_eq!(texture.levels[2], [255, 128, 0, 255]);
}

/// The error for `details`, whose limit is the first number after "than the" in it. The size
/// over the limit is not compared, as most limits are met before it is known.
fn too_large_error(details: impl Into<String>) -> ModelError {
    let details = details.into();
    let limit = details
        .split("than the ")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .and_then(|number| number.parse().ok())
        .expect("the details name the limit");
    ModelError::TooLarge {
        details,
        limit,
        actual: None,
    }
}

/// `error` without how large the model was, which only some limits know.
fn ignoring_size(error: ModelError) -> ModelError {
    match error {
        ModelError::TooLarge { details, limit, .. } => ModelError::TooLarge {
            details,
            limit,
            actual: None,
        },
        error => error,
    }
}

#[test]
fn an_image_declaring_a_huge_size_is_rejected_before_decoding() {
    let image = resized_header(png(1, 1, [0; 4]), 100_000, 100_000);
    let error = textured(Glb::triangle(), &image).read().unwrap_err();
    assert_eq!(
        error,
        too_large_error(
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
        too_large_error("the model has more texture pixels than the 160000000 it may".to_string())
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
fn images_count_by_their_bytes_whatever_buffer_views_name_them() {
    // 128 buffer views over one PNG followed by 64 KiB the decoder never reaches.
    let views = |identical: bool| {
        let image = png(1, 1, [0, 255, 0, 255]);
        let length = image.len() + (64 << 10);
        let mut glb = textured(Glb::triangle(), &[image, vec![0; 64 << 10]].concat());
        let start = glb.json["bufferViews"][2]["byteOffset"].as_u64().unwrap();
        let shorter = |view: usize| if identical { 0 } else { 4 * view };
        glb.json["bufferViews"]
            .as_array_mut()
            .unwrap()
            .extend((0..128).map(|view| {
                json!({ "buffer": 0, "byteOffset": start, "byteLength": length - shorter(view) })
            }));
        glb.json["images"] = (3..131)
            .map(|view| json!({ "bufferView": view, "mimeType": "image/png" }))
            .collect();
        glb.json["textures"] = (0..128).map(|source| json!({ "source": source })).collect();
        glb.json["materials"] = (0..128)
            .map(|index| json!({ "pbrMetallicRoughness": { "baseColorTexture": { "index": index } } }))
            .collect();
        glb
    };
    let identical = views(true).read().unwrap();
    assert_eq!(identical.textures.len(), 1);
    let file = views(false).bytes().len();
    assert_eq!(
        ignoring_size(views(false).read().unwrap_err()),
        too_large_error(format!(
            "the model's images take more than the {file} bytes of its file"
        ))
    );
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

#[test]
fn info_lists_clips_and_materials_by_index_with_names_when_the_file_has_them() {
    let mut glb = Glb::triangle();
    let times = glb.accessor(FLOAT, "SCALAR", 2, &floats(&[0.0, 1.5]));
    let values = glb.accessor(FLOAT, "VEC3", 2, &floats(&[0.0; 6]));
    let clip = |name: Option<&str>| {
        let mut clip = json!({
            "samplers": [{ "input": times, "output": values }],
            "channels": [{ "sampler": 0, "target": { "node": 0, "path": "translation" } }],
        });
        if let Some(name) = name {
            clip["name"] = json!(name);
        }
        clip
    };
    glb.json["animations"] = json!([clip(Some("spin")), clip(None)]);
    glb.json["materials"] = json!([
        { "name": "body" },
        {},
        { "name": "screen", "pbrMetallicRoughness": { "baseColorTexture": { "index": 0 } } },
    ]);
    glb.json["textures"] = json!([{ "source": 0 }]);
    let view = glb.view(&png(1, 1, [255; 4]));
    glb.json["images"] = json!([{ "bufferView": view, "mimeType": "image/png" }]);

    let info = ModelInfo::from_glb(&glb.bytes()).unwrap();
    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        json!({
            "animations": [
                { "index": 0, "name": "spin", "duration": 1.5 },
                { "index": 1, "duration": 1.5 },
            ],
            "materials": [
                { "index": 0, "name": "body", "hasBaseColorTexture": false },
                { "index": 1, "hasBaseColorTexture": false },
                { "index": 2, "name": "screen", "hasBaseColorTexture": true },
            ],
            "cameras": [],
        })
    );
}

#[test]
fn info_of_a_model_with_neither_is_empty_and_of_a_bad_file_is_an_error() {
    let info = ModelInfo::from_glb(&Glb::triangle().bytes()).unwrap();
    assert_eq!(info, ModelInfo::default());
    assert_eq!(
        serde_json::to_value(&info).unwrap(),
        json!({ "animations": [], "materials": [], "cameras": [] })
    );
    assert!(ModelInfo::from_glb(&Glb::triangle().bytes()[..30]).is_err());
}

#[test]
fn info_reads_the_fixture_with_a_clip() {
    let info = ModelInfo::from_glb(GRID).unwrap();
    assert_eq!(info.animations.len(), 1);
    assert_eq!(info.animations[0].name.as_deref(), Some("bend"));
    assert!(info.animations[0].duration > 0.0);
}

#[test]
fn errors_list_at_most_twenty_names() {
    let mut glb = Glb::triangle();
    glb.json["materials"] = json!(
        (0..25)
            .map(|n| json!({ "name": format!("m{n}") }))
            .collect::<Vec<_>>()
    );
    let scene = glb.read().unwrap().scene;
    let error = scene.material("missing").unwrap_err();
    assert!(
        error.contains("`m19`") && !error.contains("`m20`"),
        "{error}"
    );
    assert!(error.ends_with(", and 5 more"), "{error}");
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
    // How serde_json reads a float too large for `f32` depends on the features it is built with:
    // as infinity, which the reader refuses, or as a number out of range.
    let too_large: Edit =
        Box::new(|json| json["materials"] = json!([{ "emissiveFactor": [1e39, 0, 0] }]));
    assert!(matches!(error_with(too_large), ModelError::Invalid(_)));
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
fn integers_past_32_bits_are_rejected_only_where_they_would_be_truncated() {
    let held = |value: u64| {
        format!(
            "the model's JSON holds {value} where an index or code belongs, which 32 bits can't hold"
        )
    };
    let cases: [(u64, Edit); 4] = [
        (
            4_294_967_296,
            Box::new(|json| json["nodes"][0]["mesh"] = json!(4_294_967_296u64)),
        ),
        (
            4_294_967_296,
            Box::new(|json| json["nodes"] = json!([{ "mesh": 0, "children": [4_294_967_296u64] }])),
        ),
        (
            4_294_967_296,
            Box::new(|json| {
                json["meshes"][0]["primitives"][0]["attributes"]["POSITION"] =
                    json!(4_294_967_296u64);
            }),
        ),
        (
            4_294_972_422,
            Box::new(|json| json["accessors"][0]["componentType"] = json!(4_294_972_422u64)),
        ),
    ];
    for (value, edit) in cases {
        assert_eq!(error_with(edit), ModelError::Invalid(held(value)));
    }
    // A float written without a decimal point, beside an index map whose key isn't an index.
    let mut glb = Glb::triangle();
    glb.json["accessors"][0]["max"] = json!([5_000_000_000u64, 1, 0]);
    glb.json["accessors"][0]["min"] = json!([0, 0, 0]);
    glb.json["extras"] = json!([{ "mesh": 5_000_000_000u64 }, "any value"]);
    glb.read().expect("a large float bound is not an index");
}

#[test]
fn deeply_nested_json_is_an_error() {
    let mut glb = Glb::triangle();
    glb.json["extras"] = json!("nested");
    let depth = 100_000;
    let nested = format!("{}{}", "[".repeat(depth), "]".repeat(depth));
    let bytes = glb.bytes_with(|json| json.replace("\"nested\"", &nested));
    let error = Model::from_glb(&bytes).unwrap_err();
    let ModelError::Invalid(message) = error else {
        panic!("{error:?}");
    };
    assert!(message.contains("recursion limit exceeded"), "{message}");
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
    required.json["extensionsUsed"] = json!(["KHR_materials_transmission"]);
    required.json["extensionsRequired"] = json!(["KHR_materials_transmission"]);
    let mut optional = Glb::triangle();
    optional.json["extensionsUsed"] = json!(["KHR_materials_transmission"]);
    let mut cases = vec![
        (
            external,
            "the model refers to a file outside itself, `triangle.bin`; pack everything into one .glb"
                .to_string(),
        ),
        (
            required,
            "the model requires the glTF extension KHR_materials_transmission, which isn't supported"
                .to_string(),
        ),
    ];
    // A compressed model whose decoder the crate is built without.
    for (fixture, extension, enabled) in COMPRESSED {
        if !enabled {
            cases.push((
                Glb::parse(fixture),
                format!("the model requires the glTF extension {extension}, which isn't supported"),
            ));
        }
    }
    for (glb, expected) in cases {
        assert_eq!(glb.read().unwrap_err(), ModelError::Unsupported(expected));
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
        too_large_error(format!("the model has more {what} than the {limit} it may"))
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
        too_large_error(
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

/// Writes two of the models these tests build to `fuzz/seeds`, for the fuzz targets.
#[test]
#[ignore = "writes the fuzz seeds"]
fn write_fuzz_seeds() {
    let rich = textured(skinned(), &png(2, 2, [255, 0, 0, 255]));
    for (name, glb) in [("triangle", Glb::triangle()), ("rich", rich)] {
        let path = format!("{}/fuzz/seeds/{name}.glb", env!("CARGO_MANIFEST_DIR"));
        std::fs::write(path, glb.bytes()).unwrap();
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
    // Each model, how many rounds to mutate it, and the range of its binary chunk to mutate: for
    // a compressed copy of the grid, its compressed payload.
    let mut models = vec![(Glb::triangle(), 1500, None), (rich, 1500, None)];
    for (fixture, extension, enabled) in COMPRESSED {
        if enabled {
            let glb = Glb::parse(fixture);
            let view = glb.json.pointer(&payload(&glb, extension)).unwrap();
            let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
            let length = view["byteLength"].as_u64().unwrap() as usize;
            models.push((glb, 100, Some(offset..offset + length)));
        }
    }
    for (glb, rounds, payload) in models {
        let mut paths = Vec::new();
        number_paths(&glb.json, String::new(), &mut paths);
        let payload = payload.unwrap_or(0..glb.bin.len());
        for round in 0..rounds {
            let mut json = glb.json.clone();
            let mut bin = glb.bin.clone();
            for _ in 0..1 + next() % 2 {
                if round % 2 == 0 {
                    let path = &paths[next() as usize % paths.len()];
                    *json.pointer_mut(path).unwrap() =
                        boundaries[next() as usize % boundaries.len()].clone();
                } else {
                    let at = payload.start + next() as usize % payload.len();
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

#[test]
fn each_joints_bounds_hold_the_vertices_it_moves_before_posing() {
    let model = skinned().read().unwrap();
    // The first joint moves the corner at the origin and shares the top one; the second moves
    // the corner along X and shares the top one.
    assert_eq!(
        model.scene.joint_bounds(&model.geometry),
        [SkinBounds {
            joints: vec![
                crate::Bounds::of([Vec3::ZERO, Vec3::Y]),
                crate::Bounds::of([Vec3::ZERO, Vec3::new(1.0, 1.0, 0.0)]),
            ],
            unmoved: crate::Bounds::EMPTY,
        }]
    );
    assert_eq!(model.scene.pose(None).skins, [(0, 0)]);

    // A vertex weighted to no joint is drawn where it is, so it counts apart from the joints.
    let mut glb = skinned();
    let weights = glb.accessor(
        FLOAT,
        "VEC4",
        3,
        &floats(&[1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.5, 0.0, 0.0]),
    );
    glb.json["meshes"][0]["primitives"][0]["attributes"]["WEIGHTS_0"] = json!(weights);
    let model = glb.read().unwrap();
    let bounds = model.scene.joint_bounds(&model.geometry);
    assert_eq!(bounds[0].unmoved, crate::Bounds::of([Vec3::X]));
    assert_eq!(
        bounds[0].joints[1],
        crate::Bounds::of([Vec3::Y]),
        "the joint the unweighted vertex names doesn't take it"
    );
}

/// `grid.glb`, from `tests/fixtures`: a skinned and animated grid with normals, texture
/// coordinates and a 16 x 16 texture, of which the other `grid-*.glb` there are compressed copies.
const GRID: &[u8] = include_bytes!("../../tests/fixtures/grid.glb");
/// Each compressed copy of [`GRID`], the extension it requires, and whether the crate is built to
/// read it.
const COMPRESSED: [(&[u8], &str, bool); 5] = [
    (
        include_bytes!("../../tests/fixtures/grid-draco.glb"),
        "KHR_draco_mesh_compression",
        cfg!(feature = "draco"),
    ),
    (
        include_bytes!("../../tests/fixtures/grid-meshopt.glb"),
        "EXT_meshopt_compression",
        cfg!(feature = "meshopt"),
    ),
    (
        include_bytes!("../../tests/fixtures/grid-etc1s.glb"),
        "KHR_texture_basisu",
        cfg!(feature = "ktx2"),
    ),
    (
        include_bytes!("../../tests/fixtures/grid-uastc.glb"),
        "KHR_texture_basisu",
        cfg!(feature = "ktx2"),
    ),
    (
        include_bytes!("../../tests/fixtures/grid-webp.glb"),
        "EXT_texture_webp",
        cfg!(feature = "webp"),
    ),
];

/// A corner of a drawn triangle: its position and normal in model space, and its texture
/// coordinates.
type Corner = [f32; 8];

/// Each triangle `model` draws with its first clip at `seconds`, or at rest, wound as it is drawn.
fn drawn_triangles(model: &Model, seconds: Option<f32>) -> Vec<[Corner; 3]> {
    let pose = model.scene.pose(seconds.map(|seconds| Playback {
        clip: 0,
        seconds,
        looped: false,
    }));
    let mut triangles = Vec::new();
    for posed in &pose.primitives {
        let corner = |index: u32| -> Corner {
            let vertex = model.geometry.vertices[index as usize];
            let matrix = match posed.first_joint {
                Some(first) => {
                    let skin = model.geometry.skin[index as usize];
                    (0..4).fold(Mat4::ZERO, |sum, i| {
                        let joint = pose.joints[first as usize + usize::from(skin.joints[i])];
                        sum + joint * (f32::from(skin.weights[i]) / 65535.0)
                    })
                }
                None => posed.world,
            };
            let position = matrix.transform_point3(vertex.position.into());
            let normal = matrix
                .transform_vector3(vertex.normal.into())
                .normalize_or_zero();
            [
                position.x,
                position.y,
                position.z,
                normal.x,
                normal.y,
                normal.z,
                vertex.uv[0],
                vertex.uv[1],
            ]
        };
        let primitive = posed.primitive;
        let indices = &model.geometry.indices[primitive.first_index as usize..]
            [..primitive.index_count as usize];
        triangles.extend(indices.chunks(3).map(|triangle| {
            [
                corner(triangle[0]),
                corner(triangle[1]),
                corner(triangle[2]),
            ]
        }));
    }
    triangles
}

/// Whether `a` and `b` draw the same triangles, in any order and starting from any corner, with
/// positions within `position` of each other, normals within 0.02 and texture coordinates within
/// 0.002.
fn same_triangles(a: &[[Corner; 3]], b: &[[Corner; 3]], position: f32) -> bool {
    let close = |a: &Corner, b: &Corner| {
        let tolerances = [position, position, position, 0.02, 0.02, 0.02, 0.002, 0.002];
        a.iter()
            .zip(b)
            .zip(tolerances)
            .all(|((a, b), tolerance)| (a - b).abs() <= tolerance)
    };
    a.len() == b.len()
        && a.iter().all(|triangle| {
            b.iter().any(|other| {
                (0..3).any(|turn| (0..3).all(|i| close(&triangle[i], &other[(i + turn) % 3])))
            })
        })
}

/// The mean difference of `a`'s and `b`'s bytes.
fn mean_difference(a: &[u8], b: &[u8]) -> f64 {
    assert_eq!(a.len(), b.len());
    let total: u64 = a
        .iter()
        .zip(b)
        .map(|(&a, &b)| u64::from(a.abs_diff(b)))
        .sum();
    total as f64 / a.len() as f64
}

/// Asserts `compressed` draws what `original` does, at rest and partway through its first clip,
/// with positions within `position`, and that its first texture differs on average by less than
/// `texture` of 255.
fn assert_reads_as(original: &Model, compressed: &Model, position: f32, texture: f64, name: &str) {
    for seconds in [None, Some(0.5)] {
        assert!(
            same_triangles(
                &drawn_triangles(original, seconds),
                &drawn_triangles(compressed, seconds),
                position
            ),
            "{name} at {seconds:?}"
        );
    }
    let (expected, decoded) = (&original.textures[0], &compressed.textures[0]);
    assert_eq!(
        (
            decoded.width,
            decoded.height,
            decoded.srgb,
            decoded.levels.len()
        ),
        (
            expected.width,
            expected.height,
            expected.srgb,
            expected.levels.len()
        ),
        "{name}"
    );
    let difference = mean_difference(&expected.levels[0], &decoded.levels[0]);
    assert!(difference < texture, "{name}: {difference}");
}

#[test]
fn compressed_models_read_as_their_originals_do() {
    let original = Model::from_glb(GRID).unwrap();
    for (fixture, extension, enabled) in COMPRESSED {
        if enabled {
            let compressed =
                Model::from_glb(fixture).unwrap_or_else(|error| panic!("{extension}: {error}"));
            // Positions quantized to 14 bits of the grid's extent of 2; ETC1S is the lossiest.
            assert_reads_as(&original, &compressed, 2e-3, 12.0, extension);
        }
    }
}

#[cfg(any(feature = "draco", all(feature = "meshopt", feature = "ktx2")))]
#[test]
fn compressed_foxes_read_as_the_fox_does() {
    // The fox is about 150 units long, its positions quantized to 14 bits of that.
    const FOX_TOLERANCE: f32 = 0.1;
    let fox = Model::from_glb(include_bytes!("../../../engine/tests/fixtures/Fox.glb")).unwrap();
    #[cfg(feature = "draco")]
    assert_reads_as(
        &fox,
        &Model::from_glb(include_bytes!(
            "../../../engine/tests/fixtures/Fox-draco.glb"
        ))
        .unwrap(),
        FOX_TOLERANCE,
        0.5,
        "Fox-draco",
    );
    #[cfg(all(feature = "meshopt", feature = "ktx2"))]
    assert_reads_as(
        &fox,
        &Model::from_glb(include_bytes!(
            "../../../engine/tests/fixtures/Fox-meshopt-ktx2.glb"
        ))
        .unwrap(),
        FOX_TOLERANCE,
        12.0,
        "Fox-meshopt-ktx2",
    );
}

/// The JSON pointer to the object giving the byte offset and length, in the binary chunk, of
/// `glb`'s first payload compressed for `extension`.
fn payload(glb: &Glb, extension: &str) -> String {
    let view = match extension {
        "KHR_draco_mesh_compression" => {
            &glb.json["meshes"][0]["primitives"][0]["extensions"][extension]["bufferView"]
        }
        "EXT_meshopt_compression" => return format!("/bufferViews/1/extensions/{extension}"),
        _ => &glb.json["images"][0]["bufferView"],
    };
    format!("/bufferViews/{view}")
}

#[test]
fn truncated_or_corrupted_compressed_payloads_are_errors() {
    for (fixture, extension, enabled) in COMPRESSED {
        if !enabled {
            continue;
        }
        let glb = Glb::parse(fixture);
        let at = payload(&glb, extension);
        let view = glb.json.pointer(&at).unwrap();
        let offset = view["byteOffset"].as_u64().unwrap_or(0) as usize;
        let length = view["byteLength"].as_u64().unwrap() as usize;
        let mut truncated = Glb {
            json: glb.json.clone(),
            bin: glb.bin.clone(),
        };
        truncated.json.pointer_mut(&at).unwrap()["byteLength"] = json!(length / 2);
        let mut corrupted = Glb {
            json: glb.json.clone(),
            bin: glb.bin.clone(),
        };
        corrupted.bin[offset + length / 2..offset + length].fill(0xFF);
        for (glb, how) in [(truncated, "truncated"), (corrupted, "corrupted")] {
            let error = glb.read().unwrap_err();
            assert!(
                matches!(error, ModelError::Invalid(_)),
                "{extension} {how}: {error:?}"
            );
        }
    }
}

/// An edit to the JSON and binary chunk of one of [`COMPRESSED`], by its index there, and the error
/// the edited file reads to.
type Case = (usize, fn(&mut Value, &mut Vec<u8>), ModelError);

/// Asserts each of `cases` whose decoder the crate is built with.
fn assert_cases(cases: Vec<Case>) {
    for (index, edit, expected) in cases {
        let (fixture, extension, enabled) = COMPRESSED[index];
        if enabled {
            let mut glb = Glb::parse(fixture);
            edit(&mut glb.json, &mut glb.bin);
            assert_eq!(
                ignoring_size(read_without_panicking(&glb.bytes()).unwrap_err()),
                expected,
                "{extension}"
            );
        }
    }
}

const MESHOPT: &str = "EXT_meshopt_compression";
/// The level index and header fields of `grid-*.glb`'s KTX2 image, which starts its binary chunk.
const KTX2_LEVEL_0_UNCOMPRESSED: usize = 96;
const KTX2_WIDTH: usize = 20;
/// The canvas size of `grid-webp.glb`'s image, less one, as two 24-bit fields.
const WEBP_CANVAS: usize = 24;

#[test]
fn compressed_data_disagreeing_with_its_declared_counts_is_an_error() {
    let draco = || {
        invalid(
            "a Draco-compressed primitive decodes to a different number of vertices or indices than its accessors declare",
        )
    };
    assert_cases(vec![
        // One vertex more, and one triangle more, than the Draco stream holds.
        (
            0,
            |json, _| json["accessors"][1]["count"] = json!(82),
            draco(),
        ),
        (
            0,
            |json, _| json["accessors"][0]["count"] = json!(387),
            draco(),
        ),
        // A meshopt view's count disagreeing with its length, and with its data.
        (
            1,
            |json, _| json["bufferViews"][1]["extensions"][MESHOPT]["count"] = json!(80),
            invalid(
                "a meshopt-compressed buffer view decodes to a different length than its buffer view has",
            ),
        ),
        (
            1,
            |json, _| {
                json["bufferViews"][1]["extensions"][MESHOPT]["count"] = json!(40);
                json["bufferViews"][1]["byteLength"] = json!(40 * 8);
            },
            invalid("a meshopt-compressed buffer view in the model is corrupt: decode error -3"),
        ),
        // A UASTC level declaring more blocks than its size has.
        (
            3,
            |_, bin| bin[KTX2_LEVEL_0_UNCOMPRESSED] = 255,
            invalid("a KTX2 image in the model is corrupt: a level's length doesn't fit its size"),
        ),
    ]);
}

#[test]
fn compressed_data_past_its_budget_is_rejected_before_decoding() {
    let budget = |what: &str| {
        too_large_error(format!(
            "the model's {what} decode to more than the 16777216 bytes a file of its size may"
        ))
    };
    let pixels = |bytes: usize| {
        too_large_error(format!(
            "the model's KTX2 and WebP images decode to more than the 8388608 pixels their \
             {bytes} bytes may"
        ))
    };
    assert_cases(vec![
        (
            0,
            |json, _| {
                for accessor in 1..6 {
                    json["accessors"][accessor]["count"] = json!(300_000);
                }
            },
            budget("Draco meshes"),
        ),
        (
            1,
            |json, _| {
                json["bufferViews"][1]["extensions"][MESHOPT]["count"] = json!(2_200_000);
                json["bufferViews"][1]["byteLength"] = json!(2_200_000 * 8);
            },
            budget("meshopt buffer views"),
        ),
        (
            2,
            |_, bin| bin[KTX2_WIDTH..][..8].copy_from_slice(&[0, 32, 0, 0, 0, 32, 0, 0]),
            pixels(671),
        ),
        (
            4,
            |_, bin| {
                bin[WEBP_CANVAS..][..6].copy_from_slice(&[0x3F, 0x1F, 0, 0x3F, 0x1F, 0]);
                let frame = webp_frame(bin);
                bin[frame..][..4].copy_from_slice(&[0x40, 0x1F, 0x40, 0x1F]);
            },
            pixels(182),
        ),
    ]);
}

/// Where the width and height of the lossy frame of `grid-webp.glb`'s image are in `bin`.
fn webp_frame(bin: &[u8]) -> usize {
    bin.windows(4).position(|kind| kind == b"VP8 ").unwrap() + 8 + 6
}

/// A Zstandard frame of `blocks` RLE blocks of `block` bytes each, with a window of
/// `2^window_log` bytes.
fn rle_frame(window_log: u8, blocks: usize, block: u32) -> Vec<u8> {
    let mut frame = 0xFD2F_B528u32.to_le_bytes().to_vec();
    frame.extend([0, (window_log - 10) << 3]);
    for index in 0..blocks {
        let last = u32::from(index + 1 == blocks);
        frame.extend(&(block << 3 | 1 << 1 | last).to_le_bytes()[..3]);
        frame.push(0x55);
    }
    frame
}

/// `grid-uastc.glb` with its full-size level replaced by `frame`.
fn uastc_level(frame: Vec<u8>) -> Glb {
    let mut glb = Glb::parse(COMPRESSED[3].0);
    let level = u64::from_le_bytes(glb.bin[80..88].try_into().unwrap()) as usize;
    glb.bin[level..][..frame.len()].copy_from_slice(&frame);
    glb.bin[88..96].copy_from_slice(&(frame.len() as u64).to_le_bytes());
    glb
}

/// `grid-etc1s.glb` with its image declaring 2048 x 2048 pixels, and with a copy of that image
/// as its metallic-roughness texture when `twice`.
fn large_etc1s(twice: bool) -> Glb {
    let mut glb = Glb::parse(COMPRESSED[2].0);
    glb.bin[KTX2_WIDTH..][..8].copy_from_slice(&[0, 8, 0, 0, 0, 8, 0, 0]);
    if twice {
        let length = glb.json["bufferViews"][0]["byteLength"].as_u64().unwrap() as usize;
        let image = glb.bin[..length].to_vec();
        let view = glb.view(&image);
        glb.json["images"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "bufferView": view, "mimeType": "image/ktx2" }));
        glb.json["textures"]
            .as_array_mut()
            .unwrap()
            .push(json!({ "extensions": { "KHR_texture_basisu": { "source": 1 } } }));
        glb.json["materials"][0]["pbrMetallicRoughness"]["metallicRoughnessTexture"] =
            json!({ "index": 1 });
    }
    glb
}

/// Reads `bytes`, asserting that nothing panics on the way, caught or not.
fn read_without_panicking(bytes: &[u8]) -> Result<Model, ModelError> {
    thread_local!(static PANICKED: std::cell::Cell<bool> = const { std::cell::Cell::new(false) });
    static HOOK: std::sync::Once = std::sync::Once::new();
    HOOK.call_once(|| {
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            PANICKED.set(true);
            previous(info);
        }));
    });
    PANICKED.set(false);
    let model = Model::from_glb(bytes);
    assert!(!PANICKED.get(), "reading the model panicked");
    model
}

#[test]
fn zstandard_levels_are_bounded_by_their_declared_length() {
    if !cfg!(feature = "ktx2") {
        return;
    }
    // A 1 GiB window, and 1 KiB blocks running far past the level's 256 bytes.
    assert_eq!(
        ignoring_size(
            read_without_panicking(&uastc_level(rle_frame(30, 20, 128 << 10)).bytes()).unwrap_err()
        ),
        too_large_error(
            "a KTX2 image in the model has a level of 256 bytes whose Zstandard window is \
             1073741824 bytes, more than the 1024 it may be"
                .to_string()
        )
    );
    assert_eq!(
        read_without_panicking(&uastc_level(rle_frame(10, 20, 1 << 10)).bytes()).unwrap_err(),
        invalid(
            "a KTX2 image in the model is corrupt: a level inflates to another length than it declares"
        )
    );
    // The level's own length, inflated from a frame of 1 KiB window, reads.
    let mut exact = rle_frame(10, 1, 256);
    exact[5] = 0;
    let model = read_without_panicking(&uastc_level(exact).bytes()).unwrap();
    assert_eq!(model.textures[0].width, 16);
}

#[test]
fn webp_images_are_one_still_image_the_size_of_their_canvas() {
    if !cfg!(feature = "webp") {
        return;
    }
    let mut inner = Glb::parse(COMPRESSED[4].0);
    let frame = webp_frame(&inner.bin);
    inner.bin[frame..][..4].copy_from_slice(&[0xFF, 0x3F, 0xFF, 0x3F]);
    let mut animated = Glb::parse(COMPRESSED[4].0);
    animated.bin[WEBP_CANVAS - 4] |= 0x02;
    for (glb, expected) in [
        (
            inner,
            invalid(
                "a WebP image in the model is corrupt: its image is another size than its canvas",
            ),
        ),
        (
            animated,
            unsupported("an image in the model is an animated WebP"),
        ),
    ] {
        assert_eq!(read_without_panicking(&glb.bytes()).unwrap_err(), expected);
    }
}

#[test]
fn compressed_images_share_one_pixel_budget() {
    if !cfg!(feature = "ktx2") {
        return;
    }
    assert_ne!(
        read_without_panicking(&large_etc1s(false).bytes())
            .err()
            .map(|error| matches!(error, ModelError::TooLarge { .. })),
        Some(true),
        "one 2048 x 2048 image fits the floor"
    );
    assert_eq!(
        ignoring_size(read_without_panicking(&large_etc1s(true).bytes()).unwrap_err()),
        too_large_error(
            "the model's KTX2 and WebP images decode to more than the 8388608 pixels their 1342 \
             bytes may"
                .to_string()
        )
    );
}

#[test]
fn draco_streams_hold_only_named_attributes_and_fallbacks_read_first() {
    if !cfg!(feature = "draco") {
        return;
    }
    const DRACO: &str = "KHR_draco_mesh_compression";
    assert_cases(vec![
        (
            0,
            |json, _| {
                let primitive = &mut json["meshes"][0]["primitives"][0];
                primitive["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("TEXCOORD_0");
                primitive["extensions"][DRACO]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("TEXCOORD_0");
            },
            invalid("a Draco-compressed primitive holds other attributes than its extension names"),
        ),
        // Normals read from an accessor that fails, beside a corrupt stream: the accessor fails
        // first.
        (
            0,
            |json, bin| {
                let primitive = &mut json["meshes"][0]["primitives"][0];
                primitive["extensions"][DRACO]["attributes"]
                    .as_object_mut()
                    .unwrap()
                    .remove("NORMAL");
                json["accessors"][2]["bufferView"] = json!(9);
                bin.fill(0xFF);
            },
            invalid("a buffer view index is out of range"),
        ),
    ]);
}

/// Files made to reach past the reader's limits, from `tests/fixtures/review-*.glb`.
#[test]
fn files_made_to_reach_past_the_limits_are_errors_without_panicking() {
    let webp = |why: &str| invalid(format!("a WebP image in the model is corrupt: {why}"));
    let files: [(&[u8], bool, ModelError); 4] = [
        (
            include_bytes!("../../tests/fixtures/review-zstd.glb"),
            cfg!(feature = "ktx2"),
            too_large_error(
                "a KTX2 image in the model has a level of 256 bytes whose Zstandard window is \
                 1073741824 bytes, more than the 1024 it may be"
                    .to_string(),
            ),
        ),
        (
            include_bytes!("../../tests/fixtures/review-webp-size.glb"),
            cfg!(feature = "webp"),
            webp("its image is another size than its canvas"),
        ),
        (
            include_bytes!("../../tests/fixtures/review-animated.glb"),
            cfg!(feature = "webp"),
            unsupported("an image in the model is an animated WebP"),
        ),
        (
            include_bytes!("../../tests/fixtures/review-floor.glb"),
            cfg!(feature = "webp"),
            too_large_error(
                "the model's KTX2 and WebP images decode to more than the 8388608 pixels their \
                 880 bytes may"
                    .to_string(),
            ),
        ),
    ];
    for (bytes, enabled, expected) in files {
        if enabled {
            assert_eq!(
                ignoring_size(read_without_panicking(bytes).unwrap_err()),
                expected
            );
        }
    }
}

/// [`Glb::triangle`] with `attribute` of its primitive set to an accessor of `bytes`.
fn with_attribute(mut glb: Glb, attribute: &str, component: u32, kind: &str, bytes: &[u8]) -> Glb {
    let accessor = glb.accessor(component, kind, 3, bytes);
    glb.json["meshes"][0]["primitives"][0]["attributes"][attribute] = json!(accessor);
    glb
}

#[test]
fn vertex_colours_and_second_uvs_read_with_defaults_for_primitives_without_them() {
    let rgb = with_attribute(
        Glb::triangle(),
        "COLOR_0",
        FLOAT,
        "VEC3",
        &floats(&[1.0, 0.0, 0.0, 0.0, 0.5, 0.0, 2.0, -1.0, 1.0]),
    );
    let mut rgba = with_attribute(
        rgb,
        "TEXCOORD_1",
        FLOAT,
        "VEC2",
        &floats(&[0.0, 0.0, 0.5, 0.0, 0.0, 0.5]),
    );
    // A second primitive without either.
    let plain = rgba.json["meshes"][0]["primitives"][0].clone();
    let mut plain = plain;
    plain["attributes"] = json!({ "POSITION": plain["attributes"]["POSITION"] });
    rgba.json["meshes"][0]["primitives"]
        .as_array_mut()
        .unwrap()
        .insert(0, plain);
    let model = rgba.read().unwrap();
    let extras = &model.geometry.extras;
    assert_eq!(extras.len(), 6);
    let white = [u16::MAX; 4];
    for (extra, vertex) in extras[..3].iter().zip(&model.geometry.vertices) {
        assert_eq!((extra.color, extra.uv1), (white, vertex.uv));
    }
    // RGB reads with alpha 1, clamped to 0..1.
    assert_eq!(extras[3].color, [65535, 0, 0, 65535]);
    assert_eq!(extras[4].color, [0, 32768, 0, 65535]);
    assert_eq!(extras[5].color, [65535, 0, 65535, 65535]);
    assert_eq!(extras[4].uv1, [0.5, 0.0]);
    let primitives = &model.scene.meshes[0];
    assert!(!primitives[0].vertex_colors && primitives[1].vertex_colors);

    let normalized = with_attribute(
        Glb::triangle(),
        "COLOR_0",
        UNSIGNED_BYTE,
        "VEC4",
        &[255, 0, 0, 128, 0, 255, 0, 255, 0, 0, 255, 0],
    );
    let model = normalized.read().unwrap();
    assert_eq!(model.geometry.extras[0].color, [65535, 0, 0, 32896]);
    assert!(Glb::triangle().read().unwrap().geometry.extras.is_empty());
}

#[test]
fn texture_transforms_read_their_offset_rotation_scale_and_uv_set() {
    let mut glb = textured(Glb::triangle(), &png(1, 1, [255; 4]));
    glb.json["materials"] = json!([{
        "pbrMetallicRoughness": { "baseColorTexture": { "index": 0, "texCoord": 1 } },
        "occlusionTexture": { "index": 0, "extensions": { "KHR_texture_transform": {
            "offset": [0.5, 0.25], "rotation": std::f32::consts::FRAC_PI_2, "scale": [2, 3],
            "texCoord": 1 } } },
        "emissiveTexture": { "index": 0, "texCoord": 1,
            "extensions": { "KHR_texture_transform": { "texCoord": 2 } } },
    }]);
    let model = glb.read().unwrap();
    let material = &model.scene.materials()[0];
    assert_eq!(material.map_uvs[0].set, 1);
    let occlusion = material.map_uvs[4];
    assert_eq!(occlusion.set, 1);
    // Scaled, then turned a quarter so +U runs up the image, then offset: (1, 0) lands on
    // (0.5, 0.25 - 2) and (0, 1) on (0.5 + 3, 0.25).
    let at = |u: f32, v: f32| {
        let [a, b] = occlusion.transform;
        [a[0] * u + a[1] * v + a[2], b[0] * u + b[1] * v + b[2]]
    };
    let close = |a: [f32; 2], b: [f32; 2]| (a[0] - b[0]).abs() < 1e-6 && (a[1] - b[1]).abs() < 1e-6;
    assert!(close(at(1.0, 0.0), [0.5, -1.75]), "{:?}", at(1.0, 0.0));
    assert!(close(at(0.0, 1.0), [3.5, 0.25]), "{:?}", at(0.0, 1.0));
    // The transform's own third set overrides the slot's, and isn't read.
    assert!(material.maps.emissive.is_none() && material.maps.occlusion.is_some());

    for transform in [
        json!({ "offset": [1] }),
        json!({ "scale": "big" }),
        json!({ "rotation": 1e39 }),
        json!({ "texCoord": -1 }),
    ] {
        let mut bad = textured(Glb::triangle(), &png(1, 1, [255; 4]));
        bad.json["materials"][0]["emissiveTexture"]["extensions"]["KHR_texture_transform"] =
            transform.clone();
        assert!(
            matches!(bad.read(), Err(ModelError::Invalid(_))),
            "{transform}"
        );
    }
}

/// [`Glb::triangle`] with two morph targets: the first moves every vertex 1 along +Z and tilts
/// its normal, the second moves only the first vertex along +X, through a sparse accessor.
fn morphed() -> Glb {
    let mut glb = with_attribute(
        Glb::triangle(),
        "NORMAL",
        FLOAT,
        "VEC3",
        &floats(&[0.0, 0.0, 1.0].repeat(3)),
    );
    let along_z = glb.accessor(FLOAT, "VEC3", 3, &floats(&[0.0, 0.0, 1.0].repeat(3)));
    let tilt = glb.accessor(FLOAT, "VEC3", 3, &floats(&[0.5, 0.0, 0.0].repeat(3)));
    let indices = glb.view(&[0u16, 0].map(u16::to_le_bytes).concat());
    let values = glb.view(&floats(&[2.0, 0.0, 0.0]));
    let sparse = {
        let accessors = glb.json["accessors"].as_array_mut().unwrap();
        accessors.push(json!({
            "componentType": FLOAT, "type": "VEC3", "count": 3,
            "sparse": { "count": 1,
                "indices": { "bufferView": indices, "componentType": UNSIGNED_SHORT },
                "values": { "bufferView": values } },
        }));
        accessors.len() - 1
    };
    glb.json["meshes"][0]["primitives"][0]["targets"] = json!([
        { "POSITION": along_z, "NORMAL": tilt },
        { "POSITION": sparse },
    ]);
    glb.json["meshes"][0]["weights"] = json!([0.5, 0.0]);
    glb
}

#[test]
fn morph_targets_read_vertex_by_vertex_with_their_meshes_or_nodes_weights() {
    let model = morphed().read().unwrap();
    let morph = model.scene.meshes[0][0].morph.expect("morph targets");
    assert_eq!(
        morph,
        crate::Morph {
            first_delta: 0,
            first_vertex: 0,
            targets: 2
        }
    );
    let deltas = &model.geometry.morphs;
    assert_eq!(deltas.len(), 6);
    assert_eq!(deltas[0].position, [0.0, 0.0, 1.0]);
    assert_eq!(deltas[0].normal, [0.5, 0.0, 0.0]);
    assert_eq!(deltas[1].position, [2.0, 0.0, 0.0]);
    assert_eq!(deltas[3].position, [0.0; 3]);
    let pose = model.scene.pose(None);
    assert_eq!(pose.weights, [0.5, 0.0]);
    assert_eq!(pose.primitives[0].first_weight, Some(0));
    // The first vertex reaches 1 along Z and 2 along X either way of its rest place.
    let reach = model.geometry.reach(&model.scene.meshes[0][0], 0).unwrap();
    assert_eq!(reach.min, Vec3::new(-2.0, 0.0, -1.0));
    assert_eq!(reach.max, Vec3::new(2.0, 0.0, 1.0));

    let mut own = morphed();
    own.json["nodes"][0]["weights"] = json!([0.25, 1.0]);
    assert_eq!(own.read().unwrap().scene.pose(None).weights, [0.25, 1.0]);
    let mut unweighted = morphed();
    unweighted.json["meshes"][0]
        .as_object_mut()
        .unwrap()
        .remove("weights");
    assert_eq!(
        unweighted.read().unwrap().scene.pose(None).weights,
        [0.0, 0.0]
    );
}

#[test]
fn clips_animate_morph_target_weights_and_count_them_in_their_duration() {
    let mut glb = morphed();
    let times = glb.accessor(FLOAT, "SCALAR", 2, &floats(&[0.0, 2.0]));
    let values = glb.accessor(FLOAT, "SCALAR", 4, &floats(&[0.0, 1.0, 1.0, 0.0]));
    glb.json["animations"] = json!([{
        "samplers": [{ "input": times, "output": values }],
        "channels": [{ "sampler": 0, "target": { "node": 0, "path": "weights" } }],
    }]);
    let model = glb.read().unwrap();
    let at = |seconds| {
        model
            .scene
            .pose(Some(Playback {
                clip: 0,
                seconds,
                looped: false,
            }))
            .weights
    };
    assert_eq!(at(0.0), [0.0, 1.0]);
    assert_eq!(at(1.0), [0.5, 0.5]);
    assert_eq!(at(5.0), [1.0, 0.0]);
    assert_eq!(model.scene.info().animations[0].duration, 2.0);

    // Five values for two keys of two weights don't divide into keys.
    let mut uneven = glb;
    let five = uneven.accessor(FLOAT, "SCALAR", 5, &floats(&[0.0; 5]));
    uneven.json["animations"][0]["samplers"][0]["output"] = json!(five);
    assert_eq!(
        uneven.read().unwrap_err(),
        invalid("an animation has a different number of key times and values")
    );
}

#[test]
fn malformed_or_oversized_morph_targets_are_errors() {
    let edit = |edit: &dyn Fn(&mut Glb)| {
        let mut glb = morphed();
        edit(&mut glb);
        glb.read().unwrap_err()
    };
    let nan = edit(&|glb| glb.json["meshes"][0]["weights"] = json!([0.5, 1e39]));
    assert_eq!(nan, invalid("a morph target weight isn't a finite number"));
    let nan = edit(&|glb| glb.json["nodes"][0]["weights"] = json!([1e39, 0]));
    assert_eq!(nan, invalid("a morph target weight isn't a finite number"));
    let short = edit(&|glb| glb.json["meshes"][0]["weights"] = json!([0.5]));
    assert_eq!(
        short,
        invalid("a mesh has a different number of weights than morph targets")
    );
    let short = edit(&|glb| glb.json["nodes"][0]["weights"] = json!([0.5]));
    assert!(matches!(short, ModelError::Invalid(_)), "{short}");
    let fewer = edit(&|glb| {
        let accessor = glb.accessor(FLOAT, "VEC3", 2, &floats(&[0.0; 6]));
        glb.json["meshes"][0]["primitives"][0]["targets"][0]["POSITION"] = json!(accessor);
    });
    assert_eq!(fewer, different_counts_error());
    let past = edit(&|glb| {
        glb.json["meshes"][0]["primitives"][0]["targets"][0]["POSITION"] = json!(99);
    });
    assert_eq!(past, invalid("an accessor index is out of range"));
    let infinite = edit(&|glb| {
        let accessor = glb.accessor(FLOAT, "VEC3", 3, &floats(&[f32::NAN; 9]));
        glb.json["meshes"][0]["primitives"][0]["targets"][1]["NORMAL"] = json!(accessor);
    });
    assert_eq!(
        infinite,
        invalid("a morph target's displacement isn't a finite number")
    );
    let nan_key = edit(&|glb| {
        let times = glb.accessor(FLOAT, "SCALAR", 1, &floats(&[0.0]));
        let values = glb.accessor(FLOAT, "SCALAR", 2, &floats(&[f32::NAN, 0.0]));
        glb.json["animations"] = json!([{
            "samplers": [{ "input": times, "output": values }],
            "channels": [{ "sampler": 0, "target": { "node": 0, "path": "weights" } }],
        }]);
    });
    assert_eq!(
        nan_key,
        invalid("an animation's values aren't finite numbers")
    );
    let mismatched = edit(&|glb| {
        let second = glb.json["meshes"][0]["primitives"][0].clone();
        let mut second = second;
        second["targets"] = json!([second["targets"][0]]);
        glb.json["meshes"][0]["primitives"]
            .as_array_mut()
            .unwrap()
            .push(second);
    });
    assert_eq!(
        mismatched,
        invalid("a mesh's primitives have different numbers of morph targets")
    );
    let many = edit(&|glb| {
        let target = glb.json["meshes"][0]["primitives"][0]["targets"][0].clone();
        glb.json["meshes"][0]["primitives"][0]["targets"] =
            json!(vec![target; MAX_MORPH_TARGETS + 1]);
        glb.json["meshes"][0]["weights"] = json!(vec![0.0; MAX_MORPH_TARGETS + 1]);
    });
    assert_eq!(
        many,
        too_large("morph targets in one mesh", MAX_MORPH_TARGETS)
    );
}

fn different_counts_error() -> ModelError {
    invalid("a primitive's attributes hold different numbers of vertices")
}

#[test]
fn lights_and_cameras_read_where_their_nodes_place_them() {
    let mut glb = Glb::triangle();
    glb.json["extensionsUsed"] = json!(["KHR_lights_punctual"]);
    glb.json["extensions"] = json!({ "KHR_lights_punctual": { "lights": [
        { "type": "spot", "color": [1, 0.5, 0], "intensity": 4, "range": 10,
          "spot": { "innerConeAngle": 0.25, "outerConeAngle": 0.5 } },
        { "type": "directional" },
    ] } });
    glb.json["cameras"] = json!([
        { "type": "perspective", "name": "close",
          "perspective": { "yfov": 0.5, "znear": 0.1 } },
        { "type": "orthographic", "orthographic": { "xmag": 2, "ymag": 1.5, "znear": 0.5, "zfar": 20 } },
    ]);
    glb.json["nodes"] = json!([
        { "mesh": 0, "children": [1, 2] },
        { "translation": [0, 2, 0], "extensions": { "KHR_lights_punctual": { "light": 0 } } },
        { "translation": [0, 0, 3], "camera": 1 },
    ]);
    let model = glb.read().unwrap();
    let pose = model.scene.pose(None);
    let spot = pose.lights[0];
    assert_eq!(spot.kind, crate::LightKind::Spot);
    assert_eq!(spot.radiance, Vec3::new(4.0, 2.0, 0.0));
    assert_eq!(spot.position, Vec3::new(0.0, 2.0, 0.0));
    assert_eq!(spot.direction, Vec3::NEG_Z);
    assert_eq!(
        (spot.cos_outer, spot.cos_inner),
        (0.5f32.cos(), 0.25f32.cos())
    );
    assert_eq!(spot.range, 10.0);
    assert_eq!(pose.lights.len(), 1, "the directional light isn't placed");
    assert_eq!(pose.cameras.len(), 1);
    assert_eq!(pose.cameras[0].0, 1);
    assert!(close(
        pose.cameras[0].1.transform_point3(Vec3::ZERO),
        Vec3::new(0.0, 0.0, 3.0)
    ));
    let cameras = model.scene.cameras();
    assert_eq!(
        (cameras[0].projection, cameras[0].near, cameras[0].far),
        (crate::Projection::Perspective { fov_y: 0.5 }, 0.1, 1000.0)
    );
    assert_eq!(
        cameras[1].projection,
        crate::Projection::Orthographic { height: 3.0 }
    );
    assert_eq!(model.scene.camera("close"), Ok(Some(0)));
    assert_eq!(model.scene.camera("1"), Ok(Some(1)));
    assert_eq!(model.scene.camera(""), Ok(None));
    assert_eq!(
        model.scene.camera("far"),
        Err("the model has no camera `far`; it has 0 `close`, 1 (unnamed)".to_string())
    );
    assert_eq!(
        serde_json::to_value(model.scene.info()).unwrap()["cameras"],
        json!([{ "index": 0, "name": "close" }, { "index": 1 }])
    );

    let bad = |edit: &dyn Fn(&mut Value)| {
        let mut bad = Glb {
            json: glb.json.clone(),
            bin: glb.bin.clone(),
        };
        edit(&mut bad.json);
        bad.read().unwrap_err()
    };
    let missing =
        bad(&|json| json["nodes"][1]["extensions"]["KHR_lights_punctual"]["light"] = json!(2));
    assert_eq!(missing, invalid("node 1 names a light the model lacks"));
    let unknown =
        bad(&|json| json["extensions"]["KHR_lights_punctual"]["lights"][1]["type"] = json!("area"));
    assert_eq!(unknown, invalid("a light's type is missing or unknown"));
    let huge = bad(&|json| {
        json["extensions"]["KHR_lights_punctual"]["lights"][0]["intensity"] = json!(1e39)
    });
    assert!(matches!(huge, ModelError::Invalid(_)), "{huge}");
    let many = bad(&|json| {
        json["extensions"]["KHR_lights_punctual"]["lights"] =
            json!(vec![json!({ "type": "point" }); MAX_MODEL_LIGHTS + 1]);
    });
    assert_eq!(many, too_large("lights", MAX_MODEL_LIGHTS));
    let camera = bad(&|json| json["nodes"][2]["camera"] = json!(5));
    assert_eq!(
        camera,
        invalid("node 2 names a mesh, skin or camera the model lacks")
    );
}

#[test]
fn primitives_other_than_triangle_lists_are_counted_and_left_out() {
    let mut glb = Glb::triangle();
    let triangles = glb.json["meshes"][0]["primitives"][0].clone();
    let mut lines = triangles.clone();
    lines["mode"] = json!(1);
    let mut points = triangles.clone();
    points["mode"] = json!(0);
    glb.json["meshes"][0]["primitives"] = json!([lines, triangles, points]);
    let model = glb.read().unwrap();
    assert_eq!(model.scene.skipped_primitives(), 2);
    assert_eq!(model.scene.draw_count(), 1);
    assert_eq!(
        Glb::triangle().read().unwrap().scene.skipped_primitives(),
        0
    );
}
