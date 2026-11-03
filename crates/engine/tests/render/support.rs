//! One shared GPU device, JSON scenes, and table rows that render through the production path and
//! through a full-frame reference. Set `LUMEN_CASE=substring` to run only the rows it names.

use std::any::Any;
use std::collections::HashMap;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::sync::{Arc, LazyLock, Mutex, MutexGuard, PoisonError, mpsc};

use lumen_engine::{
    composition::Composition,
    error::MediaError,
    gpu::{BoundFrame, CompiledOutput, GpuCompositionRenderer},
    media::{
        CpuMediaFrame, FontResolver, ImageMetadata, ImageResolver, MediaFrame, MediaStore,
        VideoFrameResolver, VideoMetadata,
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
    /// A production renderer with empty plan caches on the shared device.
    pub fn renderer(&self) -> GpuCompositionRenderer {
        GpuCompositionRenderer::from_renderer(self.0.sibling())
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
    images: HashMap<String, Arc<CpuMediaFrame>>,
    numbered_video: Option<String>,
    fonts: HashMap<String, TestFont>,
}

impl TestMedia {
    pub fn image(mut self, source: &str, width: u32, height: u32, rgba: Vec<u8>) -> Self {
        assert_eq!(rgba.len(), width as usize * height as usize * 4);
        let frame = CpuMediaFrame {
            rgba: Arc::new(rgba),
            width,
            height,
            row_bytes: width as usize * 4,
        };
        self.images.insert(source.to_string(), Arc::new(frame));
        self
    }

    /// A 30 fps video whose frame `n` is a 1 × 1 pixel with red `n`.
    pub fn numbered_video(mut self, source: &str) -> Self {
        self.numbered_video = Some(source.to_string());
        self
    }

    /// The font file `data` for text in `family`, which must be the family the file names.
    /// Fonts are process-global and keyed by `resolver_id`, so use an id no other row does for
    /// different data.
    pub fn font(mut self, family: &str, resolver_id: &str, data: &'static [u8]) -> Self {
        let font = TestFont {
            id: resolver_id.to_string(),
            data,
        };
        self.fonts.insert(family.to_string(), font);
        self
    }
}

#[derive(Debug, Clone)]
struct TestFont {
    id: String,
    data: &'static [u8],
}

impl FontResolver for TestFont {
    fn id(&self) -> &str {
        &self.id
    }

    fn data(&self) -> Result<Vec<Vec<u8>>, MediaError> {
        Ok(vec![self.data.to_vec()])
    }
}

impl MediaStore for TestMedia {
    fn get_image_resolver(&self, source: &str) -> Option<Box<dyn ImageResolver>> {
        let frame = Arc::clone(self.images.get(source)?);
        Some(Box::new(TestImage {
            id: source.to_string(),
            frame,
        }))
    }

    fn get_video_resolver(&self, stream_id: &str) -> Option<Box<dyn VideoFrameResolver>> {
        (self.numbered_video.as_deref() == Some(stream_id)).then(|| {
            Box::new(NumberedVideo {
                id: stream_id.to_string(),
            }) as _
        })
    }

    fn font_resolver_id(&self, family: &str) -> Option<String> {
        Some(self.fonts.get(family)?.id.clone())
    }

    fn get_font_resolver(&self, family: &str) -> Option<Box<dyn FontResolver>> {
        Some(Box::new(self.fonts.get(family)?.clone()))
    }
}

struct TestImage {
    id: String,
    frame: Arc<CpuMediaFrame>,
}

impl ImageResolver for TestImage {
    fn id(&self) -> &str {
        &self.id
    }

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

struct NumberedVideo {
    id: String,
}

impl VideoFrameResolver for NumberedVideo {
    fn id(&self) -> &str {
        &self.id
    }

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
        rgba: read_rgba8(gpu, gpu.texture(raster.texture).unwrap(), size),
    }
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
        rgba: read_rgba8(gpu, gpu.texture(raster.texture).unwrap(), size),
    }
}

fn read_rgba8(
    gpu: &lumen_gpu::Renderer,
    texture: &wgpu::Texture,
    size: lumen_gpu::Size,
) -> Vec<u8> {
    let row = size.width as usize * 4;
    let padded_row = (size.width * 4).next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT);
    let buffer = gpu.device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("lumen-engine render test readback"),
        size: u64::from(padded_row) * u64::from(size.height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = gpu.device.create_command_encoder(&Default::default());
    encoder.copy_texture_to_buffer(
        texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded_row),
                rows_per_image: Some(size.height),
            },
        },
        size.as_extent(),
    );
    gpu.queue.submit([encoder.finish()]);

    let (sender, receiver) = mpsc::channel();
    buffer
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            sender.send(result).unwrap()
        });
    gpu.device
        .poll(wgpu::PollType::wait_indefinitely())
        .unwrap();
    receiver.recv().unwrap().unwrap();
    let mapped = buffer.slice(..).get_mapped_range().unwrap();
    mapped
        .chunks(padded_row as usize)
        .flat_map(|padded| &padded[..row])
        .copied()
        .collect()
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
        }
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
        let reference = self.render_frames(&mut gpu.reference(), self.bounded.map(|_| false));
        for (frame, reference) in frames.iter().zip(&reference) {
            if let Err(error) = within_rounding(&frame.rgba, &reference.rgba) {
                panic!("frame {} differs from the reference: {error}", frame.at);
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
            .map(|&at| {
                let frame = render(renderer, &self.scene, at, &self.media);
                if let Some(bounded) = bounded {
                    let domain = self.output_domain(renderer);
                    assert_eq!(
                        !domain.is_full_frame(),
                        bounded,
                        "frame {at}: output source has the domain {domain:?}"
                    );
                }
                frame
            })
            .collect()
    }

    fn output_domain(&self, renderer: &GpuCompositionRenderer) -> lumen_gpu::TextureDomain {
        let source = self
            .scene
            .graph()
            .nodes
            .values()
            .find_map(|node| match node {
                NodeKind::MediaOutput(output) => Some(&output.source),
                _ => None,
            })
            .expect("scene has a media_output");
        match &renderer.compiled().expect("a frame rendered").node_outputs[source] {
            CompiledOutput::Raster(raster) => raster.domain,
            CompiledOutput::Empty => panic!("output source {source:?} is empty"),
        }
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

pub fn panic_message(payload: &(dyn Any + Send)) -> &str {
    payload
        .downcast_ref::<String>()
        .map(String::as_str)
        .or_else(|| payload.downcast_ref::<&str>().copied())
        .unwrap_or("panicked")
}
