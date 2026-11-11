//! Brings images and video frames into the working space: uploads of CPU frames, YUV conversion
//! and imports of GPU frames.

use std::sync::Arc;

use lumen_gpu::{LayoutEntry, LayoutKind, Res, Size, TexRef, TextureDesc, wgpu};

use super::{
    FrameCx, InstanceKey, Programs, internal,
    state::{VideoUpload, YuvUpload},
};
use crate::{
    error::RenderError,
    gpu::{color::WORKING_COLOR_FORMAT as WORKING, params::dispatch_for},
    media::{
        CpuMediaFrame, MediaFrame, MediaIdentity, YuvChroma, YuvFormat, YuvFrame, YuvPlanes,
        YuvUniform,
    },
    node::{NodeId, source::media_in::AlphaMode},
};

/// The format CPU RGBA frames are uploaded in, before `INGEST_SHADER` brings them into the working
/// space.
const UPLOAD_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;
/// Brings an image or CPU video texture into the working space: an sRGB-encoded one is decoded,
/// its linear light goes from its primaries to BT.709 with colour outside BT.709 clipped at zero,
/// and it is premultiplied. Colour already premultiplied is divided back out first.
const INGEST_SHADER: &str = concat!(
    include_str!("../shaders/alpha.wgsl"),
    include_str!("../shaders/color.wgsl"),
    "
struct Params {
    premultiplied: u32,
    // Non-zero when the texture holds sRGB-encoded colour rather than linear light.
    encoded: u32,
    _pad0: u32,
    _pad1: u32,
    // The texture's primaries to BT.709, in linear light.
    gamut: mat3x3<f32>,
}

@group(0) @binding(0) var input_tex: texture_2d<f32>;
@group(0) @binding(1) var<uniform> params: Params;
@group(0) @binding(2) var output_tex: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn cs_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = textureDimensions(output_tex);
    if (id.x >= size.x || id.y >= size.y) {
        return;
    }
    var color = textureLoad(input_tex, vec2<i32>(id.xy), 0);
    if (params.premultiplied != 0u) {
        color = unpremultiply(color);
    }
    var linear = color.rgb;
    if (params.encoded != 0u) {
        linear = lumen_srgb_to_linear(linear);
    }
    linear = max(params.gamut * linear, vec3<f32>(0.0));
    textureStore(output_tex, vec2<i32>(id.xy), premultiply(vec4<f32>(linear, color.a)));
}
"
);
/// `INGEST_SHADER`'s parameters.
#[repr(C)]
#[derive(Clone, Copy, bytemuck::Pod, bytemuck::Zeroable)]
struct IngestParams {
    premultiplied: u32,
    encoded: u32,
    _pad: [u32; 2],
    /// Columns of the gamut matrix, each padded to four floats as WGSL lays out a `mat3x3`.
    gamut: [[f32; 4]; 3],
}

impl IngestParams {
    fn new(premultiplied: bool, encoded: bool, gamut: Option<crate::media::GamutMatrix>) -> Self {
        let rows = gamut.map_or(
            [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            |gamut| gamut.0,
        );
        Self {
            premultiplied: u32::from(premultiplied),
            encoded: u32::from(encoded),
            _pad: [0; 2],
            gamut: [0, 1, 2].map(|column| [rows[0][column], rows[1][column], rows[2][column], 0.0]),
        }
    }
}

/// An image frame's pixels as they are uploaded.
#[derive(Clone, Copy)]
enum ImageUpload<'a> {
    /// RGBA8, sRGB-encoded.
    Srgb8(&'a CpuMediaFrame),
    /// Straight linear half floats.
    Linear(&'a crate::media::LinearMediaFrame),
}

const YUV_TO_RGBA_SHADER: &str = concat!(
    include_str!("../shaders/color.wgsl"),
    include_str!("../shaders/yuv_to_rgba.wgsl")
);
const YUV_TO_RGBA_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::compute(0, LayoutKind::UintTexture),
    LayoutEntry::compute(1, LayoutKind::UintTexture),
    LayoutEntry::compute(2, LayoutKind::UintTexture),
    LayoutEntry::compute(3, LayoutKind::UintTexture),
    LayoutEntry::compute(4, LayoutKind::Uniform),
    LayoutEntry::compute(5, LayoutKind::StorageTexture(WORKING)),
];

impl Programs<'_> {
    /// The program that brings CPU images and video frames into the working space.
    pub(crate) fn ingest(&mut self) -> crate::Result<()> {
        self.spatial("ingest", INGEST_SHADER)
    }

    /// The program `FrameCx::yuv_to_rgba` records with.
    pub(crate) fn yuv_to_rgba(&mut self) -> crate::Result<()> {
        self.compute(
            "yuv-to-rgba",
            YUV_TO_RGBA_SHADER,
            "cs_main",
            YUV_TO_RGBA_LAYOUT,
        )
    }
}

/// The media failure of decoding a frame on the CPU after importing it on the GPU failed for
/// `reason`.
#[cfg(feature = "ffmpeg")]
fn cpu_retry_failed(error: crate::error::MediaError, reason: &str) -> crate::error::LumenError {
    use crate::error::MediaError;
    match error {
        MediaError::Failed(mut failure) => {
            let details = format!(
                "failed to import GPU media frame: {reason}; decoding it on the CPU also failed: {}",
                failure.reason.details()
            );
            failure.reason = failure.reason.with_details(details);
            MediaError::Failed(failure).into()
        }
        error => error.into(),
    }
}

impl FrameCx<'_> {
    /// An image in the working space at `size`, the size of `key`. Its texture, 8-bit for
    /// sRGB-encoded frames and half float for linear ones, is uploaded once and kept in the
    /// renderer's media cache, and each frame that shows it brings it into the working space.
    pub(crate) fn image_texture(
        &mut self,
        key: crate::gpu::MediaTextureKey,
        frame: &MediaFrame,
        size: Size,
    ) -> crate::Result<TexRef> {
        let (upload, premultiplied, gamut) = match frame {
            MediaFrame::CpuRgba(cpu) => (ImageUpload::Srgb8(cpu), true, None),
            MediaFrame::RawRgba(cpu) => (ImageUpload::Srgb8(cpu), false, None),
            MediaFrame::WideRgba { frame, gamut } => {
                (ImageUpload::Srgb8(frame), false, Some(*gamut))
            }
            MediaFrame::LinearRgba(linear) => (ImageUpload::Linear(linear), false, None),
            MediaFrame::Yuv(_) => {
                return Err(internal("an image resolver returned a video frame"));
            }
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(_) => {
                return Err(internal("an image resolver returned a GPU frame"));
            }
        };
        let native = match upload {
            ImageUpload::Srgb8(cpu) => (cpu.width, cpu.height),
            ImageUpload::Linear(linear) => (linear.width, linear.height),
        };
        // A copy at another size is stored straight, which is known before the cache is asked.
        let stored_premultiplied = premultiplied && native == (size.width, size.height);
        let encoded = matches!(upload, ImageUpload::Srgb8(_));
        let texture = match self.media_cache.get(&key) {
            Some(texture) => texture,
            None => {
                let format = if encoded {
                    UPLOAD_FORMAT
                } else {
                    wgpu::TextureFormat::Rgba16Float
                };
                let texture =
                    crate::gpu::renderer::create_media_texture(self.rec.device(), size, format);
                let write = |bytes: &[u8]| {
                    self.rec.queue().write_texture(
                        texture.as_image_copy(),
                        bytes,
                        wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(
                                size.width * format.block_copy_size(None).unwrap_or(4),
                            ),
                            rows_per_image: Some(size.height),
                        },
                        size.as_extent(),
                    );
                };
                match upload {
                    ImageUpload::Srgb8(cpu) => {
                        write(&crate::gpu::image_size::rgba8_at(cpu, premultiplied, size));
                    }
                    ImageUpload::Linear(linear) => {
                        write(bytemuck::cast_slice(&crate::gpu::image_size::linear_at(
                            linear, size,
                        )));
                    }
                }
                self.media_cache.insert(key.clone(), Arc::clone(&texture));
                texture
            }
        };
        let tex = self.adopted_media(texture, Some(key));
        let params = IngestParams::new(stored_premultiplied, encoded, gamut);
        self.ingest(tex, size, &params)
    }

    /// `tex` of `size` in the working space, as `params` says it is stored.
    fn ingest(&mut self, tex: TexRef, size: Size, params: &IngestParams) -> crate::Result<TexRef> {
        let target = self.storage_target(size);
        self.spatial_pass("ingest", INGEST_SHADER, tex, params, target, size)?;
        Ok(target)
    }

    /// The texture of an image, which holds the pixels `image` names, or of a video plane.
    fn adopted_media(
        &mut self,
        texture: Arc<wgpu::Texture>,
        image: Option<crate::gpu::MediaTextureKey>,
    ) -> TexRef {
        let rec = &mut *self.rec;
        let tick = self.st.tick;
        let (tex, last_used, _) = self
            .st
            .media
            .textures
            .entry(Arc::as_ptr(&texture) as usize)
            .or_insert_with(|| (rec.adopt(texture), tick, image));
        *last_used = tick;
        *tex
    }

    /// A CPU video frame in the working space, from a pooled 8-bit texture it is written to before
    /// the frame's passes run.
    fn cpu_video_texture(
        &mut self,
        frame: Arc<CpuMediaFrame>,
        size: Size,
        premultiplied: bool,
    ) -> crate::Result<TexRef> {
        let usage = TextureDesc::sampled(size, UPLOAD_FORMAT).usage;
        let tex = self.rec.acquire_written_early(size, UPLOAD_FORMAT, usage);
        self.st
            .media
            .video_uploads
            .push(VideoUpload { tex, frame, size });
        self.ingest(tex, size, &IngestParams::new(premultiplied, true, None))
    }

    /// The texture of a frame decoded on the CPU, either as RGBA or as YUV planes.
    fn cpu_frame_texture(
        &mut self,
        frame: MediaFrame,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        match frame {
            MediaFrame::CpuRgba(frame) => self.cpu_video_texture(frame, size, true),
            MediaFrame::RawRgba(frame) => {
                self.cpu_video_texture(frame, size, alpha == AlphaMode::Premultiplied)
            }
            MediaFrame::Yuv(frame) => self.yuv_video_texture(frame, size, alpha),
            MediaFrame::WideRgba { .. } | MediaFrame::LinearRgba(_) => {
                Err(internal("a video resolver returned an image frame"))
            }
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(_) => Err(internal("a CPU frame was expected, not a GPU one")),
        }
    }

    /// Writes a CPU YUV frame's planes to pooled textures before the frame's passes run, and
    /// converts them to RGBA.
    fn yuv_video_texture(
        &mut self,
        frame: Arc<YuvFrame>,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        frame
            .validate()
            .map_err(|details| internal(&format!("a YUV frame can't be read: {details}")))?;
        let max = self.rec.device().limits().max_texture_dimension_2d;
        if frame.format.width > max || frame.format.height > max {
            return Err(RenderError::Limit {
                details: format!(
                    "a {}x{} YUV frame is larger than this device's {max}-pixel texture limit",
                    frame.format.width, frame.format.height
                ),
            }
            .into());
        }
        let wide = frame.format.bytes_per_sample() == 2;
        let rec = &mut *self.rec;
        let uploads = &mut self.st.media.yuv_uploads;
        let planes = frame.layout.map_with_shape(&frame.format, |plane, shape| {
            let format = match (shape.channels, wide) {
                (1, false) => wgpu::TextureFormat::R8Uint,
                (_, false) => wgpu::TextureFormat::Rg8Uint,
                (1, true) => wgpu::TextureFormat::R16Uint,
                (_, true) => wgpu::TextureFormat::Rg16Uint,
            };
            let plane_size = Size::new(shape.width, shape.height);
            let tex = rec.acquire_written_early(
                plane_size,
                format,
                wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            );
            uploads.push(YuvUpload {
                tex,
                frame: Arc::clone(&frame),
                offset: plane.offset,
                // `validate` rejects strides that don't fit.
                stride: plane.stride as u32,
                shape,
            });
            tex
        });
        self.yuv_to_rgba(&planes, &frame.format, size, alpha)
    }

    /// Converts YUV planes into a texture of `size` in the working space, showing the frame
    /// `format` describes upright and stretched to it by nearest neighbour. Colour that `alpha`
    /// says is already premultiplied is divided by alpha before decoding. Plane textures hold
    /// unsigned-integer samples (`R8Uint`, `Rg8Uint` for interleaved chroma, or their 16-bit forms)
    /// with the frame at their top left, and any texture this frame's passes can read will do:
    /// pooled ones written before the passes, or adopted ones a decoder filled on the GPU.
    pub(crate) fn yuv_to_rgba(
        &mut self,
        planes: &YuvPlanes<TexRef>,
        format: &YuvFormat,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        let program = self.program(
            "yuv-to-rgba",
            YUV_TO_RGBA_SHADER,
            "cs_main",
            YUV_TO_RGBA_LAYOUT,
        )?;
        let (u, v, interleaved) = match planes.chroma {
            YuvChroma::Interleaved(chroma) => (chroma, chroma, true),
            YuvChroma::Planar { u, v } => (u, v, false),
        };
        // An absent alpha plane binds luma in its place, which the shader doesn't read.
        let alpha_plane = planes.alpha.unwrap_or(planes.luma);
        let uniform = self.rec.uniform(&YuvUniform::new(
            format,
            interleaved,
            planes.alpha.is_some(),
            alpha == AlphaMode::Premultiplied,
        ));
        let target = self.storage_target(size);
        self.rec.compute(
            program,
            &[
                (0, Res::Tex(planes.luma)),
                (1, Res::Tex(u)),
                (2, Res::Tex(v)),
                (3, Res::Tex(alpha_plane)),
                (4, uniform),
                (5, Res::Tex(target)),
            ],
            dispatch_for(size),
        );
        Ok(target)
    }

    /// The texture `media_in` node `node_id` shows `source_frame` of the video `resolver` names
    /// through, taking the frame from `frame` the first time this frame shows it.
    pub(crate) fn video_texture(
        &mut self,
        node_id: NodeId,
        identity: &MediaIdentity,
        source_frame: u32,
        frame: impl FnOnce() -> crate::Result<MediaFrame>,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        let instance = (self.graph_key(), self.path, node_id);
        let key = (
            instance,
            identity.clone(),
            source_frame,
            (size.width, size.height),
            alpha == AlphaMode::Premultiplied,
        );
        if let Some(tex) = self.st.media.video_frames.get(&key) {
            return Ok(*tex);
        }
        let tex = self.decoded_video_texture(instance, frame()?, size, alpha)?;
        self.st.media.video_frames.insert(key, tex);
        Ok(tex)
    }

    /// The texture a video frame shows through. A GPU frame is imported now, ahead of this frame's
    /// submission in queue order, which settles its source on the GPU. One the renderer can't use
    /// moves its source to the CPU, unless the source has already imported a GPU frame, even earlier
    /// in this output frame, which fails the render instead.
    fn decoded_video_texture(
        &mut self,
        #[cfg_attr(not(feature = "ffmpeg"), allow(unused_variables))] instance: InstanceKey,
        frame: MediaFrame,
        size: Size,
        alpha: AlphaMode,
    ) -> crate::Result<TexRef> {
        match frame {
            frame @ (MediaFrame::CpuRgba(_)
            | MediaFrame::RawRgba(_)
            | MediaFrame::WideRgba { .. }
            | MediaFrame::LinearRgba(_)
            | MediaFrame::Yuv(_)) => self.cpu_frame_texture(frame, size, alpha),
            #[cfg(feature = "ffmpeg")]
            MediaFrame::GpuVideo(frame) => {
                // Imports copy into their slot's textures before the frame's passes run, so each
                // distinct frame a source shows this frame needs a slot of its own.
                let imported = self.st.media.frame_imports.entry(instance).or_default();
                let key = (instance, *imported);
                *imported += 1;
                let tick = self.st.tick;
                let next = &mut self.st.media.next_video_import;
                let slot = self.st.media.video_imports.entry(key).or_insert_with(|| {
                    *next += 1;
                    (*next, tick)
                });
                slot.1 = tick;
                let slot = slot.0;
                let imported = self
                    .importer
                    .import(self.renderer, self.imports, slot, &frame)
                    .and_then(|imported| {
                        if frame.settle_on_gpu() {
                            Ok(imported)
                        } else {
                            Err("its source moved to the CPU".to_string())
                        }
                    });
                match imported {
                    Ok(imported) => {
                        let planes = imported
                            .planes
                            .map_with_shape(&imported.format, |texture, _| {
                                self.adopted_media(Arc::clone(texture), None)
                            });
                        self.yuv_to_rgba(&planes, &imported.format, size, alpha)
                    }
                    Err(reason) => {
                        let cpu = frame
                            .decode_on_cpu(&reason)
                            .map_err(|error| cpu_retry_failed(error, &reason))?;
                        self.cpu_frame_texture(cpu, size, alpha)
                    }
                }
            }
        }
    }
}
