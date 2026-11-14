//! Scenes, models and maps that more than one `scene_*` test module builds.

use lumen_3d::environment::Equirect;
use lumen_3d::glam::Vec3;
use lumen_3d::test_support::write_glb;
use serde_json::{Value, json};

use super::{BLUE, CLEAR, Frame, Scene, TestMedia, gpu, render, scene};

pub const SIZE: (u32, u32) = (64, 64);

pub const CENTER: u32 = 32;

/// `scene_nodes`, the last of which is the scene, drawn by a `render_3d` filling the canvas.
pub fn render_3d(scene_nodes: Vec<Value>) -> Scene {
    render_3d_with(json!({}), scene_nodes)
}

pub fn render_3d_with(params: Value, scene_nodes: Vec<Value>) -> Scene {
    let last = scene_nodes.last().expect("a scene node")["id"].clone();
    let mut nodes = scene_nodes;
    nodes
        .push(json!({ "id": 100, "type": "render_3d", "params": params, "in": { "scene": last } }));
    scene(SIZE, json!(nodes))
}

/// An unlit shape, which shows its colour exactly wherever it is drawn.
pub fn flat(id: u32, params: Value, color: [u8; 4]) -> Value {
    let mut params = params;
    params["color"] = json!(color);
    params["unlit"] = json!(true);
    json!({ "id": id, "type": "shape_3d", "params": params })
}

pub fn merge(id: u32, objects: &[u32], params: Value) -> Value {
    json!({ "id": id, "type": "merge_3d", "params": params, "in": { "objects": objects } })
}

/// Pixels of row `y` within `xs` that are at least half covered.
pub fn covered(frame: &Frame, y: u32, xs: std::ops::Range<u32>) -> usize {
    xs.filter(|&x| frame.pixel(x, y)[3] >= 128).count()
}

/// A 64 x 64 image of straight RGBA, the colour of each pixel given by `color(x, y)`.
pub fn image(color: impl Fn(u32, u32) -> [u8; 4]) -> TestMedia {
    let pixels = (0..SIZE.1)
        .flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
        .flat_map(|(x, y)| color(x, y))
        .collect();
    TestMedia::default().image("plate", SIZE.0, SIZE.1, pixels)
}

/// A 64 x 64 image with red growing to the right and green downwards.
pub fn gradient() -> TestMedia {
    image(|x, y| [x as u8 * 4, y as u8 * 4, 128, 255])
}

/// The image `plate` as a 2D raster.
pub fn plate(id: u32) -> Value {
    json!({ "id": id, "type": "media_in", "params": { "kind": "image", "source": "plate" } })
}

pub fn plane(id: u32, source: u32, params: Value) -> Value {
    json!({ "id": id, "type": "image_plane_3d", "params": params, "in": { "source": source } })
}

/// A camera that sees 4 units across the canvas however far away, so that a plane 4 units wide
/// fills the frame and maps its texels to pixels.
pub fn ortho(id: u32, params: Value) -> Value {
    let mut camera = json!({ "projection": "orthographic", "ortho_height": 4 });
    for (name, value) in params.as_object().expect("params") {
        camera[name] = value.clone();
    }
    json!({ "id": id, "type": "camera_3d", "params": camera })
}

/// A plane filling the frame, in front of an orthographic camera.
pub fn framed(source: u32) -> Vec<Value> {
    vec![
        plane(1, source, json!({ "width": 4 })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ]
}

/// Opaque blue on the left half, nothing on the right.
pub fn half_blue() -> TestMedia {
    image(|x, _| if x < CENTER { BLUE } else { CLEAR })
}

/// The glTF sample fox: skinned, textured, without vertex normals, with the clips `Survey`,
/// `Walk` and `Run`.
pub fn fox_media() -> TestMedia {
    TestMedia::default().model("fox.glb", include_bytes!("../../fixtures/Fox.glb").to_vec())
}

/// The fox standing side on to the default camera, nose to the right, filling most of the frame.
pub fn fox(id: u32, params: Value) -> Value {
    let mut fox = json!({
        "source": "fox.glb",
        "position": [0, -0.8, 0],
        "rotation": [0, 90, 0],
        "scale": [0.02, 0.02, 0.02],
    });
    for (name, value) in params.as_object().expect("params") {
        fox[name] = value.clone();
    }
    json!({ "id": id, "type": "model_3d", "params": fox })
}

/// A `.glb` of one mesh with a primitive for each of `quads`: a rectangle facing +Z, given by its
/// left, bottom, right and top edges, that shows the whole of its textures, top left at top left,
/// in the material its index in `materials` names.
pub fn quads(materials: Value, quads: &[([f32; 4], usize)]) -> Vec<u8> {
    let quads: Vec<Quad> = quads
        .iter()
        .map(|&(edges, material)| Quad {
            edges,
            material,
            normal: [0.0, 0.0, 1.0],
            mirrored_u: false,
        })
        .collect();
    model_glb(materials, None, &quads)
}

/// A rectangle facing +Z for [`model_glb`].
pub struct Quad {
    /// Its left, bottom, right and top edges.
    pub edges: [f32; 4],
    pub material: usize,
    /// The normal at each of its vertices.
    pub normal: [f32; 3],
    /// Whether its texture coordinates run right to left.
    pub mirrored_u: bool,
}

/// A `.glb` of one mesh with a primitive for each of `quads`, which show the whole of their
/// textures, top left at top left. `image`, a PNG, is texture 0.
pub fn model_glb(materials: Value, image: Option<&[u8]>, quads: &[Quad]) -> Vec<u8> {
    let mut bin = Vec::new();
    let (mut views, mut accessors, mut primitives) = (Vec::new(), Vec::new(), Vec::new());
    let mut accessor = |bytes: Vec<u8>, kind: &str, component: u32, count: usize| {
        views.push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len() }));
        bin.extend(bytes);
        bin.resize(bin.len().next_multiple_of(4), 0);
        accessors.push(json!({
            "bufferView": views.len() - 1, "componentType": component, "type": kind, "count": count
        }));
        accessors.len() - 1
    };
    let floats =
        |values: &[f32]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
    for quad in quads {
        let [left, bottom, right, top] = quad.edges;
        let positions = [
            left, top, 0.0, right, top, 0.0, left, bottom, 0.0, right, bottom, 0.0,
        ];
        let position = accessor(floats(&positions), "VEC3", 5126, 4);
        let normal = accessor(floats(&quad.normal.repeat(4)), "VEC3", 5126, 4);
        let (start, end) = if quad.mirrored_u {
            (1.0, 0.0)
        } else {
            (0.0, 1.0)
        };
        let uv = accessor(
            floats(&[start, 0.0, end, 0.0, start, 1.0, end, 1.0]),
            "VEC2",
            5126,
            4,
        );
        let indices: Vec<u8> = [0u16, 2, 1, 1, 2, 3]
            .iter()
            .flat_map(|i| i.to_le_bytes())
            .collect();
        let indices = accessor(indices, "SCALAR", 5123, 6);
        primitives.push(json!({
            "attributes": { "POSITION": position, "NORMAL": normal, "TEXCOORD_0": uv },
            "indices": indices,
            "material": quad.material,
        }));
    }
    let image = image.map(|image| {
        views.push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": image.len() }));
        bin.extend(image);
        bin.resize(bin.len().next_multiple_of(4), 0);
        views.len() - 1
    });
    let mut document = json!({
        "asset": { "version": "2.0" },
        "extensionsUsed": ["KHR_materials_unlit"],
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{ "primitives": primitives }],
        "materials": materials,
        "accessors": accessors,
        "bufferViews": views,
        "buffers": [{ "byteLength": bin.len() }],
    });
    if let Some(view) = image {
        document["images"] = json!([{ "bufferView": view, "mimeType": "image/png" }]);
        document["textures"] = json!([{ "source": 0 }]);
    }
    write_glb(&serde_json::to_vec(&document).unwrap(), &bin)
}

/// An unlit material of `color`, in linear light with straight alpha.
pub fn unlit(name: &str, color: [f32; 4]) -> Value {
    json!({
        "name": name,
        "pbrMetallicRoughness": { "baseColorFactor": color },
        "extensions": { "KHR_materials_unlit": {} },
    })
}

pub fn model(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "model_3d", "params": params })
}

/// A 1 x 1 PNG of `rgba`, stored without compression.
pub fn texel_png(rgba: [u8; 4]) -> Vec<u8> {
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
    // One row: no filter, then the texel.
    let row = [0, rgba[0], rgba[1], rgba[2], rgba[3]];
    let (a, b) = row.iter().fold((1u32, 0u32), |(a, b), &byte| {
        (
            (a + u32::from(byte)) % 65521,
            (b + a + u32::from(byte)) % 65521,
        )
    });
    let mut zlib = vec![0x78, 0x01, 0x01, 5, 0, !5, !0];
    zlib.extend(row);
    zlib.extend(((b << 16) | a).to_be_bytes());
    let mut png = vec![0x89, b'P', b'N', b'G', b'\r', b'\n', 0x1a, b'\n'];
    png.extend(chunk(b"IHDR", &[0, 0, 0, 1, 0, 0, 0, 1, 8, 6, 0, 0, 0]));
    png.extend(chunk(b"IDAT", &zlib));
    png.extend(chunk(b"IEND", &[]));
    png
}

/// Where the ground below is seen: an orthographic camera looking straight down at `center`,
/// `height` units across, which leaves out everything above 0.05 so that only the ground and
/// the shadows on it show.
pub struct GroundView {
    pub center: [f32; 2],
    pub height: f32,
}

impl GroundView {
    pub const WHOLE: Self = Self {
        center: [0.0, 0.0],
        height: 6.0,
    };

    /// `objects` over a white, fully rough ground 6 units across at y = 0, without light from
    /// all around.
    pub fn scene(&self, objects: Vec<Value>, render: Value) -> Scene {
        scene(SIZE, json!(self.nodes(objects, render)))
    }

    /// The nodes of [`Self::scene`], the `render_3d` last, with id 100.
    pub fn nodes(&self, objects: Vec<Value>, render: Value) -> Vec<Value> {
        let [x, z] = self.center;
        let mut nodes = vec![
            json!({ "id": 50, "type": "shape_3d", "params": {
                "shape": "plane", "size": [6, 1, 6], "roughness": 1 } }),
            json!({ "id": 51, "type": "camera_3d", "params": {
                "projection": "orthographic", "ortho_height": self.height, "position": [x, 4, z],
                "rotation": [-90, 0, 0], "near": 3.95, "far": 10 } }),
        ];
        nodes.extend(objects);
        let ids: Vec<u32> = nodes
            .iter()
            .filter(|node| {
                node["type"]
                    .as_str()
                    .is_some_and(|kind| kind.ends_with("_3d"))
            })
            .map(|node| node["id"].as_u64().unwrap() as u32)
            .collect();
        nodes.push(merge(90, &ids, json!({})));
        let mut params = json!({ "environment": 0 });
        for (name, value) in render.as_object().expect("params") {
            params[name] = value.clone();
        }
        nodes.push(
            json!({ "id": 100, "type": "render_3d", "params": params, "in": { "scene": 90 } }),
        );
        nodes
    }

    /// The pixel showing the ground at `x`, `z`.
    pub fn pixel(&self, x: f32, z: f32) -> (u32, u32) {
        let at = |value: f32, center: f32| ((value - center) / self.height + 0.5) * SIZE.0 as f32;
        (at(x, self.center[0]) as u32, at(z, self.center[1]) as u32)
    }

    /// Whether pixel `(x, y)` lies `margin` pixels or more inside the ground's `left`, `top`,
    /// `right` and `bottom`, or outside them for a negative margin.
    pub fn inside(
        &self,
        (x, y): (u32, u32),
        [left, top, right, bottom]: [f32; 4],
        margin: f32,
    ) -> bool {
        let (x0, y0) = self.pixel(left, top);
        let (x1, y1) = self.pixel(right, bottom);
        let (x, y) = (x as f32 + 0.5, y as f32 + 0.5);
        x >= x0 as f32 + margin
            && x <= x1 as f32 + 1.0 - margin
            && y >= y0 as f32 + margin
            && y <= y1 as f32 + 1.0 - margin
    }
}

pub fn light(id: u32, params: Value) -> Value {
    json!({ "id": id, "type": "light_3d", "params": params })
}

pub fn frame_of(scene: Scene, media: &TestMedia) -> Frame {
    render(&mut gpu().renderer(), &scene.into(), 0.0, media)
}

pub fn every_pixel() -> impl Iterator<Item = (u32, u32)> {
    (0..SIZE.1).flat_map(|y| (0..SIZE.0).map(move |x| (x, y)))
}

/// A `width` by `height` equirectangular `.hdr` whose texels hold `radiance` of the direction
/// through their centres.
pub fn environment_file(width: u32, height: u32, radiance: impl Fn(Vec3) -> Vec3) -> Vec<u8> {
    let map = Equirect::from_fn(width, height, radiance);
    let texels: Vec<[f32; 3]> = map.texels().iter().map(|texel| texel.to_array()).collect();
    crate::support::hdr_file(width, height, &texels)
}

/// A sphere of radius 1 at the origin, about 18 pixels across its radius from the default camera.
pub fn sphere(material: Value) -> Value {
    let mut params = json!({ "shape": "sphere", "size": [2, 2, 2] });
    for (name, value) in material.as_object().expect("material") {
        params[name] = value.clone();
    }
    json!({ "id": 1, "type": "shape_3d", "params": params })
}

pub fn lit_by(map: &str, params: Value) -> Value {
    let mut render = json!({ "environment_map": map });
    for (name, value) in params.as_object().expect("params") {
        render[name] = value.clone();
    }
    render
}
