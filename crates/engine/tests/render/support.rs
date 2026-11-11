//! One shared GPU device, JSON scenes, and table rows that render through the production path and
//! through a full-frame reference. Set `LUMEN_CASE=substring` to run only the rows it names.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError};

use lumen_engine::{
    composition::Composition,
    error::MediaError,
    gpu::{BoundFrame, GpuCompositionRenderer, WORKING_COLOR_FORMAT},
    media::{
        AssetKind, CpuMediaFrame, EnvironmentResolver, FontResolver, ImageMetadata, ImageResolver,
        MediaFrame, MediaIdentity, MediaResolver, MediaStore, ModelResolver, ResolvedMedia,
        TextSession, VideoFrameResolver, VideoMetadata, YuvFrame,
    },
    node::NodeKind,
};
use lumen_gpu::wgpu;
use serde_json::{Value, json};

const INSTALL_GPU: &str = "Install a Vulkan driver, or lavapipe (the mesa-vulkan-drivers package) \
    and run with LUMEN_GPU_FORCE_FALLBACK_ADAPTER=1 WGPU_ALLOW_UNDERLYING_NONCOMPLIANT_ADAPTER=1 \
    WGPU_BACKEND=vulkan WGPU_POWER_PREF=none VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd*.json";

const OUTPUT_NODE: u32 = 9999;

// One device for the whole binary. Holding its guard runs a test alone, as fonts are process-global.
static GPU: LazyLock<Result<Mutex<lumen_gpu::Renderer>, String>> = LazyLock::new(|| {
    pollster::block_on(lumen_gpu::Renderer::new())
        .map(Mutex::new)
        .map_err(|error| format!("{error:#}"))
});

pub struct Gpu(MutexGuard<'static, lumen_gpu::Renderer>);

/// Waits for the shared device, and panics if this machine has none. A test takes it once and
/// makes as many renderers from it as it needs, and `check` takes it itself.
pub fn gpu() -> Gpu {
    let device = GPU
        .as_ref()
        .unwrap_or_else(|error| panic!("render tests need a GPU: {error}. {INSTALL_GPU}"));
    Gpu(device.lock().unwrap_or_else(PoisonError::into_inner))
}

impl Gpu {
    /// A production renderer with nothing held yet, on the shared device. Its output isn't
    /// dithered, so pixels are the exact rounding of the working space.
    pub fn renderer(&self) -> GpuCompositionRenderer {
        let mut renderer = self.dithered_renderer();
        renderer.set_output_dither(false);
        renderer
    }

    /// A production renderer with its default output dither.
    pub fn dithered_renderer(&self) -> GpuCompositionRenderer {
        GpuCompositionRenderer::from_renderer(lumen_gpu::Renderer::from_device_with_adapter_info(
            self.0.device.clone(),
            self.0.queue.clone(),
            self.0.adapter_info().cloned(),
        ))
    }

    /// A renderer whose sources all fill the canvas and render on every frame.
    pub fn reference(&self) -> GpuCompositionRenderer {
        let mut renderer = self.renderer();
        renderer.set_full_frame_sources(true);
        renderer.set_static_output_reuse(false);
        renderer
    }
}

/// A composition as JSON, so a row can edit it before it is built.
pub struct Scene(pub Value);

/// A one-frame 30 fps scene of `nodes`. A node's `in` maps each input port to a node id, or to an
/// array of them for a variadic port, and the last node feeds the output.
pub fn scene((width, height): (u32, u32), nodes: Value) -> Scene {
    let Value::Array(mut nodes) = nodes else {
        panic!("scene nodes must be an array");
    };
    let mut connections = Vec::new();
    for node in &mut nodes {
        let id = node["id"].clone();
        let Some(inputs) = node.as_object_mut().and_then(|node| node.remove("in")) else {
            continue;
        };
        for (port, from) in inputs.as_object().expect("`in` must map ports to node ids") {
            let sources = from
                .as_array()
                .map_or(std::slice::from_ref(from), Vec::as_slice);
            for source in sources {
                connections.push(json!({ "from_node": source, "to_node": id, "to_port": port }));
            }
        }
    }
    let last = nodes.last().expect("scene needs a node")["id"].clone();
    nodes.push(json!({ "id": OUTPUT_NODE, "type": "media_output" }));
    connections.push(json!({ "from_node": last, "to_node": OUTPUT_NODE, "to_port": "source" }));
    Scene(json!({
        "timeline": { "fps": 30, "duration_frames": 1 },
        "render_settings": { "width": width, "height": height },
        "nodes": nodes,
        "connections": connections,
    }))
}

impl Scene {
    pub fn frames(mut self, duration_frames: u32) -> Self {
        self.0["timeline"]["duration_frames"] = json!(duration_frames);
        self
    }

    pub fn fps(mut self, fps: f64) -> Self {
        self.0["timeline"]["fps"] = json!(fps);
        self
    }
}

impl From<Scene> for Composition {
    fn from(scene: Scene) -> Self {
        lumen_engine::json::parse_value(&scene.0)
            .unwrap_or_else(|error| panic!("scene does not parse: {error:#}"))
    }
}

#[derive(Debug, Default, Clone)]
pub struct TestMedia {
    /// The identity key and pixels by source.
    images: HashMap<String, (String, MediaFrame)>,
    /// The size and pixels of every frame by source.
    still_videos: HashMap<String, ((u32, u32), MediaFrame)>,
    numbered_video: Option<String>,
    /// The identity key and font data by family.
    fonts: HashMap<String, (String, Arc<Vec<u8>>)>,
    /// Present once a font is added. Fonts load into it, so they reach only this media and its
    /// clones, and not the process-wide session.
    session: Option<Arc<TextSession>>,
    font_requests: Arc<AtomicUsize>,
    /// How many image resolvers were asked for, shared with its clones.
    image_requests: Arc<AtomicUsize>,
    /// Identity key and file of each model by source.
    models: HashMap<String, (String, Arc<Vec<u8>>)>,
    /// How many times model bytes were read, shared with its clones.
    model_loads: Arc<AtomicUsize>,
    /// Identity key and file of each environment map by source.
    environments: HashMap<String, (String, Arc<Vec<u8>>)>,
    /// How many times environment map bytes were read, shared with its clones.
    environment_loads: Arc<AtomicUsize>,
}

impl TestMedia {
    pub fn image(self, source: &str, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        self.image_with_id(source, source, width, height, rgba)
    }

    /// Like `image`, as the identity keyed `id`. The engine tells two images of one source apart by
    /// their identities, so give an ID per distinct pixels.
    pub fn image_with_id(
        mut self,
        source: &str,
        id: &str,
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    ) -> Self {
        let frame = MediaFrame::CpuRgba(still_frame(width, height, rgba));
        self.images
            .insert(source.to_string(), (id.to_string(), frame));
        self
    }

    /// Like `image`, with straight-alpha pixels, as a decoded image file has.
    pub fn straight_image(mut self, source: &str, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let frame = MediaFrame::RawRgba(still_frame(width, height, rgba));
        self.images
            .insert(source.to_string(), (source.to_string(), frame));
        self
    }

    /// A 30 fps video of 256 frames, which all show `rgba`.
    pub fn still_video(mut self, source: &str, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let frame = MediaFrame::CpuRgba(still_frame(width, height, rgba));
        self.still_videos
            .insert(source.to_string(), ((width, height), frame));
        self
    }

    /// Like `still_video`, with pixels the renderer premultiplies unless the node says they are.
    pub fn raw_video(mut self, source: &str, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        let frame = MediaFrame::RawRgba(still_frame(width, height, rgba));
        self.still_videos
            .insert(source.to_string(), ((width, height), frame));
        self
    }

    /// A 30 fps video of 256 frames of `width` × `height`, which all show `frame`.
    pub fn yuv_video(mut self, source: &str, width: u32, height: u32, frame: YuvFrame) -> Self {
        let frame = MediaFrame::Yuv(Arc::new(frame));
        self.still_videos
            .insert(source.to_string(), ((width, height), frame));
        self
    }

    /// A 30 fps video whose frame `n` is a 1 × 1 pixel with red `n`.
    pub fn numbered_video(mut self, source: &str) -> Self {
        self.numbered_video = Some(source.to_string());
        self
    }

    /// Serves `data` as the font `family`, as the identity keyed `id`. Use names no other row or
    /// bundled font has, and an ID per distinct data.
    pub fn font(mut self, family: &str, id: &str, data: Vec<u8>) -> Self {
        self.session.get_or_insert_with(Default::default);
        self.fonts
            .insert(family.to_string(), (id.to_string(), Arc::new(data)));
        self
    }

    /// This media without its fonts, in the same text session. Text in a font only the original
    /// serves falls back until a render through the original loads it.
    pub fn without_fonts(&self) -> Self {
        Self {
            fonts: HashMap::new(),
            ..self.clone()
        }
    }

    /// How many times font data was read, across this media and its clones.
    pub fn font_requests(&self) -> usize {
        self.font_requests.load(Ordering::Relaxed)
    }

    pub fn image_requests(&self) -> usize {
        self.image_requests.load(Ordering::Relaxed)
    }

    #[cfg(feature = "3d")]
    pub fn model(self, source: &str, bytes: Vec<u8>) -> Self {
        self.model_with_id(source, source, bytes)
    }

    /// Like `model`, under the identity key `id`. The engine tells two files of one source apart
    /// by their identities, so give a key per distinct file.
    #[cfg(feature = "3d")]
    pub fn model_with_id(mut self, source: &str, id: &str, bytes: Vec<u8>) -> Self {
        self.models
            .insert(source.to_string(), (id.to_string(), Arc::new(bytes)));
        self
    }

    #[cfg(feature = "3d")]
    pub fn model_loads(&self) -> usize {
        self.model_loads.load(Ordering::Relaxed)
    }

    #[cfg(feature = "3d")]
    pub fn environment(self, source: &str, bytes: Vec<u8>) -> Self {
        self.environment_with_id(source, source, bytes)
    }

    /// Like `environment`, under the identity key `id`, as `model_with_id`.
    #[cfg(feature = "3d")]
    pub fn environment_with_id(mut self, source: &str, id: &str, bytes: Vec<u8>) -> Self {
        self.environments
            .insert(source.to_string(), (id.to_string(), Arc::new(bytes)));
        self
    }

    #[cfg(feature = "3d")]
    pub fn environment_loads(&self) -> usize {
        self.environment_loads.load(Ordering::Relaxed)
    }
}

/// A run-length encoded Radiance `.hdr` file of `width` by `height` linear RGB `texels`, rows
/// from the top.
#[cfg(feature = "3d")]
pub fn hdr_file(width: u32, height: u32, texels: &[[f32; 3]]) -> Vec<u8> {
    assert_eq!(texels.len(), (width * height) as usize);
    assert!((8..0x8000).contains(&width), "run-length encoded rows");
    let mut bytes =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
    let rgbe = |texel: &[f32; 3]| {
        let largest = texel[0].max(texel[1]).max(texel[2]);
        if largest < 1e-32 {
            return [0; 4];
        }
        let exponent = largest.log2().floor() as i32 + 1;
        let scale = ((8 - exponent) as f32).exp2();
        [
            (texel[0] * scale) as u8,
            (texel[1] * scale) as u8,
            (texel[2] * scale) as u8,
            (exponent + 128) as u8,
        ]
    };
    for row in texels.chunks(width as usize) {
        let pixels: Vec<[u8; 4]> = row.iter().map(rgbe).collect();
        bytes.extend([2, 2, (width >> 8) as u8, width as u8]);
        for channel in 0..4 {
            for span in pixels.chunks(128) {
                bytes.push(span.len() as u8);
                bytes.extend(span.iter().map(|pixel| pixel[channel]));
            }
        }
    }
    bytes
}

struct TestEnvironment {
    data: Arc<Vec<u8>>,
    loads: Arc<AtomicUsize>,
}

impl EnvironmentResolver for TestEnvironment {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        self.loads.fetch_add(1, Ordering::Relaxed);
        Ok(self.data.to_vec())
    }
}

fn still_frame(width: u32, height: u32, rgba: Vec<u8>) -> Arc<CpuMediaFrame> {
    assert_eq!(rgba.len(), width as usize * height as usize * 4);
    Arc::new(CpuMediaFrame {
        rgba: Arc::new(rgba),
        width,
        height,
        row_bytes: width as usize * 4,
    })
}

/// Roboto Regular renamed to `family`, which has six letters as "Roboto" does, with glyphs
/// `scale` times as large.
pub fn roboto_scaled(family: &str, scale: u16) -> Vec<u8> {
    assert_eq!(family.len(), "Roboto".len());
    let mut data = lumen_text::ROBOTO_REGULAR_BYTES.to_vec();
    let utf16 = |name: &str| {
        name.encode_utf16()
            .flat_map(u16::to_be_bytes)
            .collect::<Vec<_>>()
    };
    for (old, new) in [
        (b"Roboto".to_vec(), family.as_bytes().to_vec()),
        (utf16("Roboto"), utf16(family)),
    ] {
        for offset in 0..=data.len() - old.len() {
            if data[offset..offset + old.len()] == old[..] {
                data[offset..offset + old.len()].copy_from_slice(&new);
            }
        }
    }
    let table_count = u16::from_be_bytes(data[4..6].try_into().unwrap()) as usize;
    let head = data[12..12 + table_count * 16]
        .as_chunks::<16>()
        .0
        .iter()
        .find(|table| &table[..4] == b"head")
        .unwrap();
    let units_offset = u32::from_be_bytes(head[8..12].try_into().unwrap()) as usize + 18;
    let units = u16::from_be_bytes(data[units_offset..units_offset + 2].try_into().unwrap());
    data[units_offset..units_offset + 2].copy_from_slice(&(units / scale).to_be_bytes());
    data
}

struct TestFont {
    data: Arc<Vec<u8>>,
    reads: Arc<AtomicUsize>,
}

impl FontResolver for TestFont {
    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        self.reads.fetch_add(1, Ordering::Relaxed);
        Ok(if self.data.is_empty() {
            Vec::new()
        } else {
            vec![self.data.to_vec()]
        })
    }
}

struct TestModel {
    data: Arc<Vec<u8>>,
    loads: Arc<AtomicUsize>,
}

impl ModelResolver for TestModel {
    fn data(&self) -> Result<Vec<u8>, MediaError> {
        self.loads.fetch_add(1, Ordering::Relaxed);
        Ok(self.data.to_vec())
    }
}

impl MediaStore for TestMedia {
    fn resolve(&self, source: &str, kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
        let missing = || MediaError::source_not_found(source, "not registered");
        let by_source = || MediaIdentity::of_key(source);
        let (identity, resolver) = match kind {
            AssetKind::Image => {
                self.image_requests.fetch_add(1, Ordering::Relaxed);
                let (id, frame) = self.images.get(source).ok_or_else(missing)?;
                let image = TestImage {
                    frame: frame.clone(),
                };
                (
                    MediaIdentity::of_key(id),
                    MediaResolver::Image(Box::new(image)),
                )
            }
            AssetKind::Video => {
                let video: Box<dyn VideoFrameResolver> =
                    if let Some((size, frame)) = self.still_videos.get(source) {
                        Box::new(StillVideo {
                            size: *size,
                            frame: frame.clone(),
                        })
                    } else if self.numbered_video.as_deref() == Some(source) {
                        Box::new(NumberedVideo)
                    } else {
                        return Err(missing());
                    };
                (by_source(), MediaResolver::Video(video))
            }
            AssetKind::Font => {
                let (id, data) = self.fonts.get(source).ok_or_else(missing)?;
                let font = TestFont {
                    data: Arc::clone(data),
                    reads: Arc::clone(&self.font_requests),
                };
                (
                    MediaIdentity::of_key(id),
                    MediaResolver::Font(Box::new(font)),
                )
            }
            AssetKind::Model => {
                let (id, data) = self.models.get(source).ok_or_else(missing)?;
                let model = TestModel {
                    data: Arc::clone(data),
                    loads: Arc::clone(&self.model_loads),
                };
                (
                    MediaIdentity::of_key(id),
                    MediaResolver::Model(Box::new(model)),
                )
            }
            AssetKind::Environment => {
                let (id, data) = self.environments.get(source).ok_or_else(missing)?;
                let environment = TestEnvironment {
                    data: Arc::clone(data),
                    loads: Arc::clone(&self.environment_loads),
                };
                (
                    MediaIdentity::of_key(id),
                    MediaResolver::Environment(Box::new(environment)),
                )
            }
            AssetKind::Audio => return Err(missing()),
        };
        Ok(ResolvedMedia::new(identity, resolver))
    }

    fn text_session(&self) -> Option<&TextSession> {
        self.session.as_deref()
    }
}

struct TestImage {
    frame: MediaFrame,
}

impl ImageResolver for TestImage {
    fn metadata(&self) -> ImageMetadata {
        let (MediaFrame::CpuRgba(frame) | MediaFrame::RawRgba(frame)) = &self.frame else {
            unreachable!("test images are RGBA");
        };
        ImageMetadata {
            width: frame.width,
            height: frame.height,
        }
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        Ok(self.frame.clone())
    }
}

struct StillVideo {
    size: (u32, u32),
    frame: MediaFrame,
}

impl VideoFrameResolver for StillVideo {
    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: self.size.0,
            height: self.size.1,
            frame_count: 256,
            fps: 30.0,
        }
    }

    fn frame(&self, _frame: u32) -> Result<MediaFrame, MediaError> {
        Ok(self.frame.clone())
    }
}

struct NumberedVideo;

impl VideoFrameResolver for NumberedVideo {
    fn metadata(&self) -> VideoMetadata {
        VideoMetadata {
            width: 1,
            height: 1,
            frame_count: 256,
            fps: 30.0,
        }
    }

    fn frame(&self, frame: u32) -> Result<MediaFrame, MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::new(CpuMediaFrame {
            rgba: Arc::new(vec![frame as u8, 0, 0, 255]),
            width: 1,
            height: 1,
            row_bytes: 4,
        })))
    }
}

/// One rendered frame, and the frame time it was rendered at.
pub struct Frame {
    pub at: f64,
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl Frame {
    pub fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        assert!(
            x < self.width && y < self.height,
            "({x}, {y}) is off the frame"
        );
        let start = (y * self.width + x) as usize * 4;
        self.rgba[start..start + 4].try_into().unwrap()
    }

    /// One string per row, with `#` for an opaque pixel and `.` for any other.
    pub fn alpha_rows(&self) -> Vec<String> {
        self.rgba
            .chunks(self.width as usize * 4)
            .map(|row| {
                row.chunks(4)
                    .map(|pixel| if pixel[3] == 255 { '#' } else { '.' })
                    .collect()
            })
            .collect()
    }
}

/// Renders `scene` at frame time `at` and reads back its output.
pub fn render(
    renderer: &mut GpuCompositionRenderer,
    scene: &Composition,
    at: f64,
    media: &impl MediaStore,
) -> Frame {
    let raster = renderer
        .render_frame_at(scene, at, media)
        .unwrap_or_else(|error| panic!("frame {at} does not render: {error}"));
    let size = raster.domain.storage_size;
    let gpu = renderer.gpu_renderer();
    Frame {
        at,
        width: size.width,
        height: size.height,
        rgba: read_rgba8(gpu, &raster.texture, size),
    }
}

/// Like `render`, and also returns the texture the frame was read from.
pub fn render_held(
    renderer: &mut GpuCompositionRenderer,
    scene: &Composition,
    at: f64,
    media: &impl MediaStore,
) -> (Frame, Arc<wgpu::Texture>) {
    let raster = renderer
        .render_frame_at(scene, at, media)
        .unwrap_or_else(|error| panic!("frame {at} does not render: {error}"));
    let size = raster.domain.storage_size;
    let frame = Frame {
        at,
        width: size.width,
        height: size.height,
        rgba: read_rgba8(renderer.gpu_renderer(), &raster.texture, size),
    };
    (frame, raster.texture)
}

/// Like `render`, but hands the bound frame to `inspect` before it is drawn.
pub fn render_inspecting(
    renderer: &mut GpuCompositionRenderer,
    scene: &Composition,
    at: f64,
    media: &impl MediaStore,
    inspect: impl FnOnce(&GpuCompositionRenderer, &BoundFrame),
) -> Frame {
    let bound = renderer
        .bind_frame_at(scene, at, media)
        .unwrap_or_else(|error| panic!("frame {at} does not bind: {error}"));
    inspect(renderer, &bound);
    let (raster, _) = renderer
        .submit_bound_frame(&bound)
        .unwrap_or_else(|error| panic!("frame {at} does not render: {error}"));
    let size = raster.domain.storage_size;
    let gpu = renderer.gpu_renderer();
    Frame {
        at,
        width: size.width,
        height: size.height,
        rgba: read_rgba8(gpu, &raster.texture, size),
    }
}

pub fn read_rgba8(
    gpu: &lumen_gpu::Renderer,
    texture: &wgpu::Texture,
    size: lumen_gpu::Size,
) -> Vec<u8> {
    gpu.read_texture_rgba8(texture, size)
        .unwrap_or_else(|error| panic!("readback fails: {error}"))
}

/// Every channel is finite and alpha is within `0..=1`. Colour may be brighter than its alpha.
pub fn in_working_space(texel: [f32; 4]) -> bool {
    texel.iter().all(|channel| channel.is_finite()) && (0.0..=1.0).contains(&texel[3])
}

/// The texels of a `WORKING_COLOR_FORMAT` texture.
pub fn read_rgba16f(
    gpu: &lumen_gpu::Renderer,
    texture: &wgpu::Texture,
    size: lumen_gpu::Size,
) -> Vec<[f32; 4]> {
    assert_eq!(texture.format(), WORKING_COLOR_FORMAT);
    let row = size.width as usize * 8;
    let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: (padded_row * size.height as usize) as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row as u32),
                rows_per_image: None,
            },
        },
        size.as_extent(),
    );
    gpu.queue.submit([encoder.finish()]);
    buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .expect("wait for readback");
    let bytes = buffer.slice(..).get_mapped_range().expect("map").to_vec();
    bytes
        .chunks(padded_row)
        .flat_map(|row_bytes| row_bytes[..row].chunks(8))
        .map(|texel| {
            std::array::from_fn(|channel| {
                f16_to_f32(u16::from_le_bytes([
                    texel[channel * 2],
                    texel[channel * 2 + 1],
                ]))
            })
        })
        .collect()
}

fn f16_to_f32(bits: u16) -> f32 {
    let sign = if bits & 0x8000 == 0 { 1.0 } else { -1.0 };
    let exponent = i32::from((bits >> 10) & 0x1f);
    let mantissa = f32::from(bits & 0x3ff);
    sign * match exponent {
        0 => mantissa * 2f32.powi(-24),
        31 if mantissa == 0.0 => f32::INFINITY,
        31 => f32::NAN,
        _ => (1.0 + mantissa / 1024.0) * 2f32.powi(exponent - 15),
    }
}

type FramesCheck = dyn Fn(&[Frame]);

/// What a row's frames must look like, besides matching the reference render.
#[allow(
    clippy::large_enum_variant,
    reason = "a table holds few rows, and each is built once"
)]
pub enum Expect {
    /// Every pixel of every frame is this colour.
    Uniform([u8; 4]),
    /// These pixels of every frame are these colours.
    Probes(Vec<(u32, u32, [u8; 4])>),
    /// Every frame has these `Frame::alpha_rows`.
    AlphaRows(Vec<&'static str>),
    /// Every frame looks like this composition at the same frame time.
    Matches(Composition),
    /// Every frame has a pixel that is not transparent black. Use this when only the reference
    /// comparison matters, so that two blank renders do not pass.
    Nonblank,
    /// Each frame has its own expectation.
    PerFrame(Vec<Expect>),
    /// Runs holding the device, so it must not call `gpu()`.
    Custom(Box<FramesCheck>),
}

/// One row of a table: a scene, the frames to render on one renderer, and what they must show.
pub struct Case {
    name: String,
    scene: Composition,
    media: TestMedia,
    frames: Vec<f64>,
    expect: Expect,
    tolerance: u8,
    bounded: Option<bool>,
    uniform_columns: bool,
}

impl Case {
    /// A row that renders frame 0 and allows each channel of `expect` to be off by 1.
    pub fn new(name: impl Into<String>, scene: impl Into<Composition>, expect: Expect) -> Self {
        Self {
            name: name.into(),
            scene: scene.into(),
            media: TestMedia::default(),
            frames: vec![0.0],
            expect,
            tolerance: 1,
            bounded: None,
            uniform_columns: false,
        }
    }

    /// For a scene that is the same down every column: compares it with the reference to within 1
    /// in every byte, without bounding how many bytes differ. The reference's float rasters round
    /// in the output like the render's, and one column that rounds the other way differs in all
    /// of its pixels.
    pub fn uniform_columns(mut self) -> Self {
        self.uniform_columns = true;
        self
    }

    pub fn media(mut self, media: TestMedia) -> Self {
        self.media = media;
        self
    }

    pub fn frames(mut self, frames: impl IntoIterator<Item = f64>) -> Self {
        self.frames = frames.into_iter().collect();
        self
    }

    pub fn tolerance(mut self, tolerance: u8) -> Self {
        self.tolerance = tolerance;
        self
    }

    /// Asserts whether the node feeding the output is sized to its bounds rather than the canvas,
    /// and that it fills the canvas in the reference render.
    pub fn bounded(mut self, bounded: bool) -> Self {
        self.bounded = Some(bounded);
        self
    }

    fn run(&self, gpu: &Gpu) {
        assert!(!self.frames.is_empty(), "a case needs at least one frame");
        let frames = self.render_frames(&mut gpu.renderer(), self.bounded);
        self.expect.assert(self, gpu, &frames);
        self.assert_working_space(gpu);
        let reference = self.render_frames(&mut gpu.reference(), self.bounded.map(|_| false));
        for (frame, reference) in frames.iter().zip(&reference) {
            if self.uniform_columns {
                assert!(
                    frame.rgba.len() == reference.rgba.len()
                        && channels_within(&frame.rgba, &reference.rgba, 1),
                    "frame {} differs from the reference by more than 1",
                    frame.at
                );
            } else if let Err(error) = within_rounding(&frame.rgba, &reference.rgba) {
                panic!("frame {} differs from the reference: {error}", frame.at);
            }
        }
    }

    /// Renders each frame to a float output, which takes the working space unchanged, and checks
    /// it holds what `WORKING_COLOR_FORMAT` promises.
    fn assert_working_space(&self, gpu: &Gpu) {
        let mut renderer = gpu.renderer();
        renderer
            .compile_with_output_format(&self.scene, WORKING_COLOR_FORMAT)
            .unwrap_or_else(|error| panic!("the scene does not compile: {error}"));
        for &at in &self.frames {
            let raster = renderer
                .render_frame_at(&self.scene, at, &self.media)
                .unwrap_or_else(|error| panic!("frame {at} does not render: {error}"));
            let size = raster.domain.storage_size;
            let texels = read_rgba16f(renderer.gpu_renderer(), &raster.texture, size);
            if let Some((index, texel)) = texels
                .iter()
                .enumerate()
                .find(|(_, texel)| !in_working_space(**texel))
            {
                panic!(
                    "frame {at}: pixel ({}, {}) is {texel:?}, outside the working space",
                    index as u32 % size.width,
                    index as u32 / size.width
                );
            }
        }
    }

    fn render_frames(
        &self,
        renderer: &mut GpuCompositionRenderer,
        bounded: Option<bool>,
    ) -> Vec<Frame> {
        self.frames
            .iter()
            .enumerate()
            .map(|(index, &at)| {
                let frame = render(renderer, &self.scene, at, &self.media);
                if let Some(bounded) = bounded {
                    // A later frame that shows the output kept from an earlier one records
                    // nothing.
                    let domain = self.output_domain(renderer);
                    assert!(index > 0 || domain.is_some(), "the output source recorded");
                    if let Some(domain) = domain {
                        assert_eq!(
                            !domain.is_full_frame(),
                            bounded,
                            "frame {at}: output source has the domain {domain:?}"
                        );
                    }
                }
                frame
            })
            .collect()
    }

    fn output_domain(&self, renderer: &GpuCompositionRenderer) -> Option<lumen_gpu::TextureDomain> {
        let source = self
            .scene
            .graph()
            .nodes
            .values()
            .find_map(|node| match node {
                NodeKind::MediaOutput(output) => output.source,
                _ => None,
            })
            .expect("scene has a media_output");
        renderer.recorded_domain(&self.scene, source)
    }
}

impl Expect {
    fn assert(&self, case: &Case, gpu: &Gpu, frames: &[Frame]) {
        let same_pixel = |frame: &Frame, x, y, expected: [u8; 4]| {
            let actual = frame.pixel(x, y);
            assert!(
                channels_within(&actual, &expected, case.tolerance),
                "frame {}: pixel ({x}, {y}) is {actual:?}, expected {expected:?} within {}",
                frame.at,
                case.tolerance,
            );
        };
        match self {
            Self::Uniform(color) => {
                for frame in frames {
                    for y in 0..frame.height {
                        for x in 0..frame.width {
                            same_pixel(frame, x, y, *color);
                        }
                    }
                }
            }
            Self::Probes(probes) => {
                for frame in frames {
                    for &(x, y, color) in probes {
                        same_pixel(frame, x, y, color);
                    }
                }
            }
            Self::AlphaRows(rows) => {
                for frame in frames {
                    assert_eq!(frame.alpha_rows(), *rows, "frame {}", frame.at);
                }
            }
            Self::Matches(other) => {
                let mut renderer = gpu.renderer();
                for frame in frames {
                    let other = render(&mut renderer, other, frame.at, &case.media);
                    assert_eq!(
                        (frame.width, frame.height),
                        (other.width, other.height),
                        "frame {}: size differs from the composition it should match",
                        frame.at
                    );
                    for y in 0..frame.height {
                        for x in 0..frame.width {
                            same_pixel(frame, x, y, other.pixel(x, y));
                        }
                    }
                }
            }
            Self::Nonblank => {
                for frame in frames {
                    assert!(
                        frame.rgba.iter().any(|&byte| byte > 0),
                        "frame {} is blank",
                        frame.at
                    );
                }
            }
            Self::PerFrame(expects) => {
                assert_eq!(expects.len(), frames.len(), "one expectation per frame");
                for (expect, frame) in expects.iter().zip(frames) {
                    expect.assert(case, gpu, std::slice::from_ref(frame));
                }
            }
            Self::Custom(check) => check(frames),
        }
    }
}

fn channels_within(actual: &[u8], expected: &[u8], tolerance: u8) -> bool {
    actual
        .iter()
        .zip(expected)
        .all(|(actual, expected)| actual.abs_diff(*expected) <= tolerance)
}

/// Whether a render matches its reference up to the rounding of the hardware filter, which depends
/// on the size of the texture read: no channel differs by more than 1, and at most 0.1% of bytes
/// differ at all. Coverage the render lost, rather than rounded, is a channel off by more than 1.
pub fn within_rounding(pixels: &[u8], reference: &[u8]) -> Result<(), String> {
    if pixels.is_empty() || pixels.len() != reference.len() {
        return Err(format!(
            "{} bytes against {} in the reference",
            pixels.len(),
            reference.len()
        ));
    }
    let mut differing = 0;
    for (index, (&actual, &expected)) in pixels.iter().zip(reference).enumerate() {
        if actual.abs_diff(expected) > 1 {
            return Err(format!(
                "byte {index} (pixel {}, channel {}) is {actual}, expected {expected}",
                index / 4,
                index % 4
            ));
        }
        differing += usize::from(actual != expected);
    }
    if differing * 1000 > pixels.len() {
        return Err(format!("{differing} of {} bytes differ", pixels.len()));
    }
    Ok(())
}

/// Runs every row, then panics listing the rows that failed and why.
pub fn check(cases: Vec<Case>) {
    check_matching(cases, std::env::var("LUMEN_CASE").ok().as_deref());
}

pub fn check_matching(cases: Vec<Case>, filter: Option<&str>) {
    let gpu = gpu();
    let cases: Vec<_> = cases
        .iter()
        .filter(|case| filter.is_none_or(|filter| case.name.contains(filter)))
        .collect();
    let failures: Vec<_> = cases
        .iter()
        .filter_map(|case| {
            let result = catch_unwind(AssertUnwindSafe(|| case.run(&gpu)));
            let error = result.err()?;
            Some(format!("  {}: {}", case.name, panic_message(&*error)))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {} rows failed:\n{}",
        failures.len(),
        cases.len(),
        failures.join("\n")
    );
}

/// Runs `run` on every named row, then panics listing the rows that failed and why. For rows that
/// take the device themselves, so it must not be held.
pub fn check_each<T>(rows: Vec<(&str, T)>, run: impl Fn(T)) {
    let total = rows.len();
    let failures: Vec<_> = rows
        .into_iter()
        .filter_map(|(name, row)| {
            let error = catch_unwind(AssertUnwindSafe(|| run(row))).err()?;
            Some(format!("  {name}: {}", panic_message(&*error)))
        })
        .collect();
    assert!(
        failures.is_empty(),
        "{} of {total} rows failed:\n{}",
        failures.len(),
        failures.join("\n")
    );
}

pub fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("panicked")
}
