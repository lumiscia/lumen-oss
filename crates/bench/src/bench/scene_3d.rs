//! Measures `render_3d` on scenes built in-process: a heavy model under a moving camera at
//! several resolutions, with shadows and an environment map on and off; an image plane shown
//! smaller than its source, and a model texture; and a model whose clip has ended.
//!
//! Every frame is timed from recording to the GPU going idle, split into recording, uploading,
//! submitting and waiting for the GPU. Recording includes collecting the scene.

use std::{collections::HashMap, sync::Arc, time::Instant};

use anyhow::{Context, anyhow, bail};
use lumen_engine::{
    composition::Composition,
    error::MediaError,
    gpu::GpuCompositionRenderer,
    media::{
        AssetKind, CpuMediaFrame, EnvironmentResolver, ImageMetadata, ImageResolver, MediaFrame,
        MediaIdentity, MediaResolver, MediaStore, ModelResolver, ResolvedMedia,
    },
};
use serde_json::{Value, json};

const FPS: u32 = 60;

struct Args {
    scenarios: Vec<String>,
    frames: u32,
    triangles: u32,
    supersample: u32,
}

const SCENARIOS: [&str; 3] = ["heavy", "plane", "clip-end"];

fn parse_args() -> anyhow::Result<Args> {
    let mut args = Args {
        scenarios: Vec::new(),
        frames: 120,
        triangles: 320_000,
        supersample: 2,
    };
    let mut raw = std::env::args().skip(1);
    while let Some(arg) = raw.next() {
        let mut value = || raw.next().ok_or_else(|| anyhow!("{arg} needs a value"));
        match arg.as_str() {
            "--scenario" => {
                let name = value()?;
                if !SCENARIOS.contains(&name.as_str()) {
                    bail!("unknown scenario {name}; choose from {SCENARIOS:?}");
                }
                args.scenarios.push(name);
            }
            "--frames" => args.frames = value()?.parse()?,
            "--triangles" => args.triangles = value()?.parse()?,
            "--supersample" => args.supersample = value()?.parse()?,
            other => bail!("unknown argument {other}"),
        }
    }
    if args.scenarios.is_empty() {
        args.scenarios = SCENARIOS.map(String::from).to_vec();
    }
    Ok(args)
}

pub fn run() -> anyhow::Result<()> {
    let args = parse_args()?;
    pollster::block_on(async {
        println!(
            "{:<10} {:<16} {:>6} {:>7} {:>5} {:>8} {:>8} {:>8} {:>8} {:>9} {:>9} {:>7}",
            "scenario",
            "case",
            "size",
            "shadows",
            "env",
            "record",
            "upload",
            "submit",
            "gpu",
            "total",
            "total p95",
            "passes"
        );
        for scenario in &args.scenarios {
            for case in cases(scenario, &args) {
                let row = measure(&case, &args).await.with_context(|| case.label())?;
                println!("{row}");
            }
        }
        Ok(())
    })
}

/// One composition to measure, and the frames to measure it over.
struct Case {
    scenario: &'static str,
    name: String,
    size: (u32, u32),
    shadows: bool,
    environment: bool,
    composition: Composition,
    media: Scene3dMedia,
    /// The frames of one pass, which is repeated to fill `--frames`.
    sequence: std::ops::Range<u32>,
}

impl Case {
    fn label(&self) -> String {
        format!("{} {}", self.scenario, self.name)
    }
}

fn cases(scenario: &str, args: &Args) -> Vec<Case> {
    let mut cases = Vec::new();
    match scenario {
        "heavy" => {
            let media = Scene3dMedia::default()
                .model("heavy.glb", terrain_glb(args.triangles, 64, None))
                .environment("sky.hdr", sky_hdr());
            for size in [(1280, 720), (1920, 1080)] {
                for (shadows, environment) in
                    [(false, false), (true, false), (false, true), (true, true)]
                {
                    cases.push(Case {
                        scenario: "heavy",
                        name: format!("{}k tris", args.triangles / 1000),
                        size,
                        shadows,
                        environment,
                        composition: heavy_scene(size, shadows, environment, args.supersample),
                        media: media.clone(),
                        sequence: 0..args.frames,
                    });
                }
            }
        }
        "plane" => {
            let media = Scene3dMedia::default()
                .image(InMemoryImage::checker("plane", 4096, 4096))
                .model("ground.glb", terrain_glb(2_000, 1, None));
            for (name, source) in [
                ("image 4096^2", PlaneSource::Image),
                ("changing 1080p", PlaneSource::Changing),
                ("model texture", PlaneSource::ModelTexture),
            ] {
                cases.push(Case {
                    scenario: "plane",
                    name: name.to_string(),
                    size: (1920, 1080),
                    shadows: false,
                    environment: false,
                    composition: plane_scene(source, args.supersample),
                    media: media.clone(),
                    sequence: 0..args.frames,
                });
            }
        }
        "clip-end" => {
            let media = Scene3dMedia::default()
                .model("rising.glb", terrain_glb(args.triangles / 3, 16, Some(1.0)));
            cases.push(Case {
                scenario: "clip-end",
                name: "after the clip".to_string(),
                size: (1920, 1080),
                shadows: true,
                environment: false,
                composition: clip_scene(args.supersample),
                media,
                // The clip lasts a second, so these frames are all past its end.
                sequence: FPS + 30..FPS + 30 + args.frames,
            });
        }
        _ => unreachable!("scenarios are checked when parsed"),
    }
    cases
}

/// `nodes`, with `scene_inputs` merged into the scene that `render_3d` node 100 draws and `wires`
/// as `(from, to, port)`, feeding the output.
fn scene(
    size: (u32, u32),
    mut nodes: Vec<Value>,
    wires: &[(u32, u32, &str)],
    scene_inputs: &[u32],
) -> Composition {
    nodes.push(json!({ "id": 99, "type": "merge_3d" }));
    nodes.push(json!({ "id": 100_000, "type": "media_output" }));
    let mut connections: Vec<Value> = wires
        .iter()
        .map(|(from, to, port)| json!({ "from_node": from, "to_node": to, "to_port": port }))
        .collect();
    connections.extend(
        scene_inputs
            .iter()
            .map(|from| json!({ "from_node": from, "to_node": 99, "to_port": "objects" })),
    );
    connections.push(json!({ "from_node": 99, "to_node": 100, "to_port": "scene" }));
    connections.push(json!({ "from_node": 100, "to_node": 100_000, "to_port": "source" }));
    let document = json!({
        "timeline": { "fps": FPS, "duration_frames": 600 },
        "render_settings": { "width": size.0, "height": size.1 },
        "nodes": nodes,
        "connections": connections,
    });
    lumen_engine::json::parse_value(&document).expect("the benchmark scene parses")
}

fn orbit_camera(radius: f64, height: f64) -> Value {
    json!({
        "id": 3, "type": "camera_3d", "params": {
            "look_at": true,
            "target": [0, 0, 0],
            "fov": 55,
            "far": 2000,
            "position": format!(
                "=vec3({radius} * sin(time * 0.4), {height} + 4 * sin(time * 0.7), {radius} * cos(time * 0.4))"
            ),
        }
    })
}

fn render_node(size: (u32, u32), supersample: u32, environment: bool) -> Value {
    let mut params = json!({ "width": size.0, "height": size.1, "supersample": supersample });
    if environment {
        params["environment_map"] = json!("sky.hdr");
        params["environment_background"] = json!(true);
    }
    json!({ "id": 100, "type": "render_3d", "params": params })
}

fn heavy_scene(
    size: (u32, u32),
    shadows: bool,
    environment: bool,
    supersample: u32,
) -> Composition {
    let nodes = vec![
        json!({ "id": 1, "type": "model_3d", "params": { "source": "heavy.glb" } }),
        json!({
            "id": 2, "type": "light_3d", "params": {
                "light": "directional", "intensity": 3, "rotation": [-50, 30, 0],
                "cast_shadows": shadows, "shadow_distance": 160,
            }
        }),
        orbit_camera(70.0, 16.0),
        render_node(size, supersample, environment),
    ];
    scene(size, nodes, &[], &[1, 2, 3])
}

/// What a plane scene shows a raster on.
#[derive(Clone, Copy)]
enum PlaneSource {
    /// A static 4096 x 4096 image on an `image_plane_3d`.
    Image,
    /// A raster that changes every frame on an `image_plane_3d`, so its mips are made every frame.
    Changing,
    /// The static image on a model's material, through a `texture` input.
    ModelTexture,
}

fn plane_scene(source: PlaneSource, supersample: u32) -> Composition {
    let size = (1920, 1080);
    let raster = match source {
        PlaneSource::Changing => json!({
            "id": 1, "type": "background", "params": { "paint": "=rgb(frame % 255, 120, 200)" }
        }),
        _ => {
            json!({ "id": 1, "type": "media_in", "params": { "source": "plane", "kind": "image" } })
        }
    };
    let shown = match source {
        PlaneSource::ModelTexture => json!({
            "id": 2, "type": "model_3d", "params": {
                "source": "ground.glb", "texture_material": "tile 0",
            }
        }),
        _ => json!({
            "id": 2, "type": "image_plane_3d", "params": {
                "width": 40, "position": [0, 0, 0], "rotation": [-70, 0, 0],
            }
        }),
    };
    let port = match source {
        PlaneSource::ModelTexture => "texture",
        _ => "source",
    };
    let nodes = vec![
        raster,
        shown,
        orbit_camera(60.0, 25.0),
        render_node(size, supersample, false),
    ];
    scene(size, nodes, &[(1, 2, port)], &[2, 3])
}

fn clip_scene(supersample: u32) -> Composition {
    let size = (1920, 1080);
    let nodes = vec![
        json!({
            "id": 1, "type": "model_3d", "params": {
                "source": "rising.glb", "animation": "rise", "animation_loop": false,
            }
        }),
        json!({
            "id": 2, "type": "light_3d", "params": {
                "light": "directional", "intensity": 3, "rotation": [-50, 30, 0],
                "cast_shadows": true, "shadow_distance": 160,
            }
        }),
        json!({
            "id": 3, "type": "camera_3d", "params": {
                "fov": 55, "far": 2000, "position": [0, 40, 70], "look_at": true,
                "target": [0, 0, 0],
            }
        }),
        render_node(size, supersample, false),
    ];
    scene(size, nodes, &[], &[1, 2, 3])
}

async fn measure(case: &Case, args: &Args) -> anyhow::Result<String> {
    let mut renderer = GpuCompositionRenderer::new().await?;
    renderer.compile(&case.composition)?;
    let (mut record, mut upload, mut submit, mut gpu, mut total) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let frame = |renderer: &mut GpuCompositionRenderer, frame: u32| -> anyhow::Result<_> {
        let started = Instant::now();
        let bound = renderer.bind_frame(&case.composition, frame, &case.media)?;
        let bound_at = Instant::now();
        renderer.upload_bound_frame(&bound)?;
        let uploaded_at = Instant::now();
        let (_raster, submission) = renderer.submit_render()?;
        let submitted_at = Instant::now();
        drop(bound);
        renderer
            .gpu_renderer()
            .device
            .poll(lumen_gpu::wgpu::PollType::Wait {
                submission_index: Some(submission),
                timeout: None,
            })
            .map_err(|error| anyhow!("poll: {error}"))?;
        let done = Instant::now();
        let ms = |from: Instant, to: Instant| (to - from).as_secs_f64() * 1e3;
        Ok([
            ms(started, bound_at),
            ms(bound_at, uploaded_at),
            ms(uploaded_at, submitted_at),
            ms(submitted_at, done),
            ms(started, done),
        ])
    };
    // The first pass loads the model and builds the programs.
    for index in case.sequence.clone().take(20) {
        frame(&mut renderer, index)?;
    }
    let passes_before = renderer.frame_graph_stats().recorder.passes;
    let sequence: Vec<u32> = case.sequence.clone().collect();
    for index in 0..args.frames {
        let [a, b, c, d, e] = frame(&mut renderer, sequence[index as usize % sequence.len()])?;
        record.push(a);
        upload.push(b);
        submit.push(c);
        gpu.push(d);
        total.push(e);
    }
    let passes = renderer.frame_graph_stats().recorder.passes - passes_before;
    let median = |series: &mut Vec<f64>| {
        series.sort_unstable_by(f64::total_cmp);
        series[series.len() / 2]
    };
    let p95 = |series: &[f64]| series[(series.len() - 1) * 95 / 100];
    let p95_total = {
        total.sort_unstable_by(f64::total_cmp);
        p95(&total)
    };
    let on = |flag: bool| if flag { "on" } else { "off" };
    Ok(format!(
        "{:<10} {:<16} {:>6} {:>7} {:>5} {:>8.2} {:>8.2} {:>8.2} {:>8.2} {:>9.2} {:>9.2} {:>7.1}",
        case.scenario,
        case.name,
        format!("{}p", case.size.1),
        on(case.shadows),
        on(case.environment),
        median(&mut record),
        median(&mut upload),
        median(&mut submit),
        median(&mut gpu),
        median(&mut total),
        p95_total,
        passes as f64 / f64::from(args.frames),
    ))
}

/// The media the scenes read: models, environment maps and one image, by source.
#[derive(Debug, Default, Clone)]
struct Scene3dMedia {
    models: HashMap<String, Arc<Vec<u8>>>,
    environments: HashMap<String, Arc<Vec<u8>>>,
    images: HashMap<String, InMemoryImage>,
}

impl Scene3dMedia {
    fn model(mut self, source: &str, glb: Vec<u8>) -> Self {
        self.models.insert(source.to_string(), Arc::new(glb));
        self
    }

    fn environment(mut self, source: &str, hdr: Vec<u8>) -> Self {
        self.environments.insert(source.to_string(), Arc::new(hdr));
        self
    }

    fn image(mut self, image: InMemoryImage) -> Self {
        self.images.insert(image.id.clone(), image);
        self
    }
}

struct Bytes(Arc<Vec<u8>>);

impl ModelResolver for Bytes {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        Ok(self.0.to_vec())
    }
}

impl EnvironmentResolver for Bytes {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        Ok(self.0.to_vec())
    }
}

impl MediaStore for Scene3dMedia {
    // Each source is set once, before the run, so it is named by its source alone.
    fn resolve(&self, source: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
        let missing = || MediaError::source_not_found(source, "not benchmark media");
        let resolver = match kind {
            AssetKind::Model => {
                let data = self.models.get(source).ok_or_else(missing)?;
                MediaResolver::Model(Box::new(Bytes(Arc::clone(data))))
            }
            AssetKind::Environment => {
                let data = self.environments.get(source).ok_or_else(missing)?;
                MediaResolver::Environment(Box::new(Bytes(Arc::clone(data))))
            }
            AssetKind::Image => {
                let image = self.images.get(source).ok_or_else(missing)?;
                MediaResolver::Image(Box::new(image.clone()))
            }
            _ => return Err(missing()),
        };
        Ok(ResolvedMedia::new(MediaIdentity::of_key(source), resolver))
    }
}

/// An opaque image of single-texel black and white squares, the finest pattern a raster holds.
#[derive(Debug, Clone)]
struct InMemoryImage {
    id: String,
    frame: Arc<CpuMediaFrame>,
}

impl InMemoryImage {
    fn checker(id: &str, width: u32, height: u32) -> Self {
        let mut rgba = Vec::with_capacity(width as usize * height as usize * 4);
        for y in 0..height {
            for x in 0..width {
                let value = if (x + y) % 2 == 0 { 255 } else { 0 };
                rgba.extend_from_slice(&[value, value, value, 255]);
            }
        }
        Self {
            id: id.to_string(),
            frame: Arc::new(CpuMediaFrame {
                rgba: Arc::new(rgba),
                width,
                height,
                row_bytes: width as usize * 4,
            }),
        }
    }
}

impl ImageResolver for InMemoryImage {
    fn metadata(&self) -> ImageMetadata {
        ImageMetadata {
            width: self.frame.width,
            height: self.frame.height,
        }
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.frame)))
    }
}

/// How high the terrain stands at `(x, z)`, in metres.
fn height(x: f32, z: f32) -> f32 {
    3.0 * (x * 0.05).sin() * (z * 0.07).cos() + 1.2 * (x * 0.21 + z * 0.13).sin()
}

/// The unit normal of the terrain at `(x, z)`.
fn normal(x: f32, z: f32) -> [f32; 3] {
    let step = 0.05;
    let dx = height(x + step, z) - height(x - step, z);
    let dz = height(x, z + step) - height(x, z - step);
    let length = (dx * dx + 4.0 * step * step + dz * dz).sqrt();
    [-dx / length, 2.0 * step / length, -dz / length]
}

/// A `.glb` of a terrain 200 metres across made of about `triangles` triangles, one mesh primitive
/// and material to each of `tiles` x `tiles` square tiles of it, so that it draws `tiles`^2 times.
/// With a `clip` of that many seconds, the whole terrain rises 2 metres over the clip.
fn terrain_glb(triangles: u32, tiles: u32, clip: Option<f32>) -> Vec<u8> {
    const SIZE: f32 = 200.0;
    let quads_per_side = ((triangles as f32 / 2.0).sqrt() as u32 / tiles).max(1);
    let quad = SIZE / (quads_per_side * tiles) as f32;
    let mut bin = Vec::new();
    let (mut views, mut accessors, mut primitives, mut materials) =
        (Vec::new(), Vec::new(), Vec::new(), Vec::new());
    let mut accessor = |bytes: Vec<u8>,
                        kind: &str,
                        component: u32,
                        count: usize,
                        range: Option<([f32; 3], [f32; 3])>| {
        views.push(json!({ "buffer": 0, "byteOffset": bin.len(), "byteLength": bytes.len() }));
        bin.extend(bytes);
        bin.resize(bin.len().next_multiple_of(4), 0);
        let mut accessor = json!({
            "bufferView": views.len() - 1, "componentType": component, "type": kind, "count": count
        });
        if let Some((min, max)) = range {
            accessor["min"] = json!(min);
            accessor["max"] = json!(max);
        }
        accessors.push(accessor);
        accessors.len() - 1
    };
    let floats =
        |values: &[f32]| -> Vec<u8> { values.iter().flat_map(|v| v.to_le_bytes()).collect() };
    let side = quads_per_side + 1;
    for tile in 0..tiles * tiles {
        let (origin_x, origin_z) = (
            -SIZE / 2.0 + (tile % tiles) as f32 * quads_per_side as f32 * quad,
            -SIZE / 2.0 + (tile / tiles) as f32 * quads_per_side as f32 * quad,
        );
        let (mut positions, mut normals) = (Vec::new(), Vec::new());
        let (mut low, mut high) = ([f32::MAX; 3], [f32::MIN; 3]);
        for row in 0..side {
            for column in 0..side {
                let (x, z) = (
                    origin_x + column as f32 * quad,
                    origin_z + row as f32 * quad,
                );
                let position = [x, height(x, z), z];
                for axis in 0..3 {
                    low[axis] = low[axis].min(position[axis]);
                    high[axis] = high[axis].max(position[axis]);
                }
                positions.extend(position);
                normals.extend(normal(x, z));
            }
        }
        let mut indices: Vec<u8> = Vec::new();
        for row in 0..quads_per_side {
            for column in 0..quads_per_side {
                let first = row * side + column;
                for index in [
                    first,
                    first + side,
                    first + 1,
                    first + 1,
                    first + side,
                    first + side + 1,
                ] {
                    indices.extend(index.to_le_bytes());
                }
            }
        }
        let position = accessor(
            floats(&positions),
            "VEC3",
            5126,
            (side * side) as usize,
            Some((low, high)),
        );
        let normal = accessor(floats(&normals), "VEC3", 5126, (side * side) as usize, None);
        let indices = accessor(
            indices,
            "SCALAR",
            5125,
            (quads_per_side * quads_per_side * 6) as usize,
            None,
        );
        primitives.push(json!({
            "attributes": { "POSITION": position, "NORMAL": normal },
            "indices": indices,
            "material": tile,
        }));
        let hue = (tile as f32 * 0.618_034).fract();
        let channel =
            |offset: f32| 0.25 + 0.6 * (0.5 + 0.5 * ((hue + offset) * std::f32::consts::TAU).cos());
        materials.push(json!({
            "name": format!("tile {tile}"),
            "pbrMetallicRoughness": {
                "baseColorFactor": [channel(0.0), channel(0.33), channel(0.66), 1.0],
                "metallicFactor": (tile % 4) as f32 * 0.25,
                "roughnessFactor": 0.3 + 0.1 * (tile % 7) as f32,
            },
        }));
    }
    let mut document = json!({
        "asset": { "version": "2.0" },
        "scene": 0,
        "scenes": [{ "nodes": [0] }],
        "nodes": [{ "mesh": 0 }],
        "meshes": [{ "primitives": primitives }],
        "materials": materials,
    });
    if let Some(seconds) = clip {
        let times = accessor(
            floats(&[0.0, seconds]),
            "SCALAR",
            5126,
            2,
            Some(([0.0; 3], [seconds, 0.0, 0.0])),
        );
        let values = accessor(
            floats(&[0.0, 0.0, 0.0, 0.0, 2.0, 0.0]),
            "VEC3",
            5126,
            2,
            None,
        );
        document["animations"] = json!([{
            "name": "rise",
            "samplers": [{ "input": times, "output": values, "interpolation": "LINEAR" }],
            "channels": [{ "sampler": 0, "target": { "node": 0, "path": "translation" } }],
        }]);
    }
    document["accessors"] = json!(accessors);
    document["bufferViews"] = json!(views);
    document["buffers"] = json!([{ "byteLength": bin.len() }]);
    let mut json = serde_json::to_vec(&document).expect("glTF JSON");
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

/// A 256 x 128 equirectangular Radiance file of a blue sky brightening to the horizon, with a sun.
fn sky_hdr() -> Vec<u8> {
    const WIDTH: usize = 256;
    const HEIGHT: usize = 128;
    let mut bytes =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {HEIGHT} +X {WIDTH}\n").into_bytes();
    for row in 0..HEIGHT {
        let up = 1.0 - (row as f32 + 0.5) / HEIGHT as f32 * 2.0;
        for column in 0..WIDTH {
            let turn = (column as f32 + 0.5) / WIDTH as f32 * std::f32::consts::TAU;
            let sun = (up * 0.8 + turn.cos() * (1.0 - up * up).max(0.0).sqrt() * 0.6).max(0.0);
            let sky = 0.5 + 0.5 * (1.0 - up.abs());
            let radiance = [
                0.35 * sky + 40.0 * sun.powi(64),
                0.55 * sky + 38.0 * sun.powi(64),
                0.9 * sky + 30.0 * sun.powi(64),
            ];
            let largest = radiance.iter().copied().fold(0.0, f32::max);
            let exponent = largest.log2().floor() as i32 + 1;
            let scale = (8 - exponent) as f32;
            bytes.extend(radiance.map(|c| (c * scale.exp2()) as u8));
            bytes.push((exponent + 128) as u8);
        }
    }
    bytes
}
