use std::sync::Arc;

#[cfg(all(target_os = "linux", feature = "vulkan"))]
use std::collections::HashMap;

use crate::media::{YuvFormat, YuvPlanes};

/// The textures imported frames are held in, by the slot of the video source they come from.
#[derive(Default)]
pub(super) struct GpuMediaImports {
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    vulkan: HashMap<u32, VulkanMediaTexture>,
}

impl GpuMediaImports {
    /// Frees the textures frames were imported into under `slot`.
    pub(super) fn remove(&mut self, slot: u32) {
        #[cfg(all(target_os = "linux", feature = "vulkan"))]
        self.vulkan.remove(&slot);
        #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
        let _ = slot;
    }
}

/// A GPU frame's planes as textures the frame's passes can read, and what their samples are.
pub(super) struct ImportedFrame {
    pub(super) planes: YuvPlanes<Arc<lumen_gpu::wgpu::Texture>>,
    pub(super) format: YuvFormat,
}

#[derive(Default)]
pub(crate) struct GpuMediaFrameImporter {
    #[cfg(all(target_os = "macos", feature = "metal"))]
    metal: Option<MetalMediaImporter>,
}

impl GpuMediaFrameImporter {
    pub(super) fn import(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        imports: &mut GpuMediaImports,
        slot: u32,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<ImportedFrame, String> {
        #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
        let _ = (imports, slot);
        #[cfg(not(any(
            all(target_os = "linux", feature = "vulkan"),
            all(target_os = "macos", feature = "metal")
        )))]
        let _ = renderer;

        match frame.frame.backend() {
            #[cfg(all(target_os = "macos", feature = "metal"))]
            lumen_ffmpeg::GpuBackend::Metal => self.import_metal(renderer, frame),
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            lumen_ffmpeg::GpuBackend::Vulkan => {
                import_vulkan(renderer, &mut imports.vulkan, slot, frame)
            }
            backend => Err(format!(
                "GPU media backend {backend:?} is not supported by this renderer build"
            )),
        }
    }

    #[cfg(all(target_os = "macos", feature = "metal"))]
    fn import_metal(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<ImportedFrame, String> {
        if self.metal.is_none() {
            self.metal = Some(MetalMediaImporter::new(renderer)?);
        }
        self.metal
            .as_mut()
            .ok_or_else(|| "Metal media importer was not initialized".to_string())?
            .import_frame(renderer, frame)
    }
}

/// The format of a hardware-decoded frame of the given layout, with the colour and rotation FFmpeg
/// read for it.
#[cfg(any(
    all(target_os = "linux", feature = "vulkan"),
    all(target_os = "macos", feature = "metal")
))]
fn gpu_frame_format(
    frame: &lumen_ffmpeg::GpuVideoFrame,
    subsampling: crate::media::ChromaSubsampling,
    depth: u8,
    msb_aligned: bool,
) -> YuvFormat {
    use crate::media::Rotation;

    let (width, height) = frame.dimensions();
    YuvFormat {
        width,
        height,
        subsampling,
        depth,
        msb_aligned,
        color: crate::media::ffmpeg::yuv_color(frame.color_tags(), height),
        rotation: Rotation::from_degrees(frame.rotation()).unwrap_or_default(),
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
struct MetalMediaImporter {
    texture_cache: lumen_ffmpeg::MetalTextureCache,
}

/// The chroma subsampling and bit depth of a biplanar CoreVideo YCbCr format. The 10-bit formats
/// hold each sample in the high bits of 16.
#[cfg(all(target_os = "macos", feature = "metal"))]
fn biplanar_layout(pixel_format: u32) -> Option<(crate::media::ChromaSubsampling, u8)> {
    use crate::media::ChromaSubsampling::{Yuv420, Yuv422, Yuv444};

    Some(match &pixel_format.to_be_bytes() {
        b"420v" | b"420f" => (Yuv420, 8),
        b"x420" | b"xf20" => (Yuv420, 10),
        b"422v" | b"422f" => (Yuv422, 8),
        b"x422" | b"xf22" => (Yuv422, 10),
        b"444v" | b"444f" => (Yuv444, 8),
        b"x444" | b"xf44" => (Yuv444, 10),
        _ => return None,
    })
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl MetalMediaImporter {
    fn new(renderer: &lumen_gpu::Renderer) -> Result<Self, String> {
        let device = lumen_gpu::metal_device_from_wgpu(&renderer.device)
            .map_err(|error| error.to_string())?;
        let texture_cache =
            lumen_ffmpeg::MetalTextureCache::create(&device).map_err(|error| error.to_string())?;
        Ok(Self { texture_cache })
    }

    /// Wraps the pixel buffer's luma and chroma planes as unsigned-integer textures. Each texture
    /// holds `frame` until wgpu releases it, which is after the GPU finishes with it.
    fn import_frame(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<ImportedFrame, String> {
        use lumen_gpu::wgpu::TextureFormat;
        use objc2_metal::{MTLPixelFormat, MTLTexture};

        let lumen_ffmpeg::GpuVideoFrame::Metal(decoded) = &frame.frame else {
            return Err(format!(
                "unsupported GPU media backend {:?}; only Metal media frames can be imported here",
                frame.frame.backend()
            ));
        };
        let pixel_format = decoded.pixel_format();
        let (subsampling, depth) = biplanar_layout(pixel_format).ok_or_else(|| {
            format!(
                "VideoToolbox decoded to CoreVideo format {:?}, which isn't biplanar YCbCr",
                String::from_utf8_lossy(&pixel_format.to_be_bytes())
            )
        })?;
        let wide = depth > 8;
        let plane = |index: usize, metal: MTLPixelFormat, format: TextureFormat| {
            let metal_texture = decoded
                .create_texture(&self.texture_cache, metal, index)
                .map_err(|error| error.to_string())?;
            let size = lumen_gpu::Size::new(
                metal_texture.width().max(1) as u32,
                metal_texture.height().max(1) as u32,
            );
            let owner = Arc::clone(frame);
            // SAFETY: CoreVideo made a 2D single-layer, single-mip, single-sample texture of
            // `format` on the renderer's device, at the size read back from it.
            Ok::<_, String>(Arc::new(unsafe {
                lumen_gpu::texture_from_metal(
                    &renderer.device,
                    metal_texture,
                    &lumen_gpu::wgpu::TextureDescriptor {
                        label: Some("lumen Metal decoded media plane"),
                        size: size.as_extent(),
                        mip_level_count: 1,
                        sample_count: 1,
                        dimension: lumen_gpu::wgpu::TextureDimension::D2,
                        format,
                        usage: media_texture_usage(),
                        view_formats: &[],
                    },
                    Some(Box::new(move || drop(owner))),
                )
            }))
        };
        let (luma, chroma) = if wide {
            (
                plane(0, MTLPixelFormat::R16Uint, TextureFormat::R16Uint)?,
                plane(1, MTLPixelFormat::RG16Uint, TextureFormat::Rg16Uint)?,
            )
        } else {
            (
                plane(0, MTLPixelFormat::R8Uint, TextureFormat::R8Uint)?,
                plane(1, MTLPixelFormat::RG8Uint, TextureFormat::Rg8Uint)?,
            )
        };
        Ok(ImportedFrame {
            planes: YuvPlanes {
                luma,
                chroma: crate::media::YuvChroma::Interleaved(chroma),
                alpha: None,
            },
            format: gpu_frame_format(&frame.frame, subsampling, depth, wide),
        })
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl Drop for MetalMediaImporter {
    fn drop(&mut self) {
        self.texture_cache.flush();
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn media_texture_usage() -> lumen_gpu::wgpu::TextureUsages {
    lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
        | lumen_gpu::wgpu::TextureUsages::COPY_DST
        | lumen_gpu::wgpu::TextureUsages::COPY_SRC
}

/// The buffer a source's frames are copied out of FFmpeg's image into, and the plane textures
/// copied from it, which stay the slot's until the frame's layout changes.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
struct VulkanMediaTexture {
    layout: lumen_ffmpeg::VulkanFrameLayout,
    buffer: Arc<lumen_gpu::wgpu::Buffer>,
    luma: Arc<lumen_gpu::wgpu::Texture>,
    chroma: Arc<lumen_gpu::wgpu::Texture>,
}

/// Copies `frame` out of FFmpeg's image into `slot`'s buffer, then into its plane textures.
/// Nothing else reads the image, so the frame only has to outlive the first copy, whose semaphore
/// wait and layout change go through FFmpeg's frame lock.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
fn import_vulkan(
    renderer: &lumen_gpu::Renderer,
    textures: &mut HashMap<u32, VulkanMediaTexture>,
    slot: u32,
    frame: &crate::media::GpuVideoMediaFrame,
) -> Result<ImportedFrame, String> {
    use lumen_ffmpeg::YuvSubsampling;
    use lumen_gpu::wgpu;

    let lumen_ffmpeg::GpuVideoFrame::Vulkan(decoded) = &frame.frame else {
        return Err(format!(
            "unsupported GPU media backend {:?}; only Vulkan media frames can be imported here",
            frame.frame.backend()
        ));
    };
    let device =
        lumen_gpu::raw_vulkan_device(&renderer.device).map_err(|error| format!("{error:#}"))?;
    if decoded.device() != device {
        return Err("the frame was decoded on another Vulkan device".to_string());
    }
    let layout = decoded.layout();
    if textures
        .get(&slot)
        .is_none_or(|texture| texture.layout != layout)
    {
        textures.insert(slot, VulkanMediaTexture::new(renderer, layout)?);
    }
    let texture = textures
        .get(&slot)
        .ok_or_else(|| "Vulkan media texture was not initialized".to_string())?;

    let buffer =
        lumen_gpu::raw_vulkan_buffer(&texture.buffer).map_err(|error| format!("{error:#}"))?;
    // SAFETY: the buffer was created on this device for this layout with COPY_DST, and the
    // retained Arc keeps it alive through the copy.
    unsafe { decoded.copy_to_buffer(buffer, Box::new(Arc::clone(&texture.buffer))) }
        .map_err(|error| error.to_string())?;

    // Submitted after the copy above on the same queue, whose closing barrier makes its writes
    // visible to these reads.
    let mut encoder = renderer
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lumen decoded YCbCr planes"),
        });
    for (plane, offset, pitch) in [
        (&texture.luma, 0, layout.luma_pitch()),
        (
            &texture.chroma,
            layout.chroma_offset(),
            layout.chroma_pitch(),
        ),
    ] {
        encoder.copy_buffer_to_texture(
            wgpu::TexelCopyBufferInfo {
                buffer: &texture.buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset,
                    bytes_per_row: Some(pitch),
                    rows_per_image: None,
                },
            },
            plane.as_image_copy(),
            plane.size(),
        );
    }
    renderer.queue.submit([encoder.finish()]);
    let subsampling = match layout.subsampling {
        YuvSubsampling::Yuv420 => crate::media::ChromaSubsampling::Yuv420,
        YuvSubsampling::Yuv422 => crate::media::ChromaSubsampling::Yuv422,
        YuvSubsampling::Yuv444 => crate::media::ChromaSubsampling::Yuv444,
    };
    Ok(ImportedFrame {
        planes: YuvPlanes {
            luma: Arc::clone(&texture.luma),
            chroma: crate::media::YuvChroma::Interleaved(Arc::clone(&texture.chroma)),
            alpha: None,
        },
        format: gpu_frame_format(&frame.frame, subsampling, layout.depth, layout.depth > 8),
    })
}

#[cfg(all(target_os = "linux", feature = "vulkan"))]
impl VulkanMediaTexture {
    fn new(
        renderer: &lumen_gpu::Renderer,
        layout: lumen_ffmpeg::VulkanFrameLayout,
    ) -> Result<Self, String> {
        use lumen_gpu::wgpu::{self, TextureFormat};

        let device = &renderer.device;
        let limits = device.limits();
        let (chroma_width, chroma_height) = layout.chroma_size();
        if layout.width > limits.max_texture_dimension_2d
            || layout.height > limits.max_texture_dimension_2d
            || layout.size() > limits.max_buffer_size
        {
            return Err(format!(
                "{}x{} frames are too large for this device",
                layout.width, layout.height
            ));
        }
        let buffer = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen decoded YCbCr"),
            size: layout.size(),
            usage: wgpu::BufferUsages::COPY_SRC | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        // wgpu would clear a buffer it believes is uninitialized when a copy first reads it, which
        // is after raw commands have written it.
        let mut clear = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lumen clear decoded YCbCr"),
        });
        clear.clear_buffer(&buffer, 0, None);
        renderer.queue.submit([clear.finish()]);

        let plane = |label, size: lumen_gpu::Size, format| {
            Arc::new(device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: size.as_extent(),
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
                view_formats: &[],
            }))
        };
        let (luma_format, chroma_format) = if layout.bytes_per_sample() == 2 {
            (TextureFormat::R16Uint, TextureFormat::Rg16Uint)
        } else {
            (TextureFormat::R8Uint, TextureFormat::Rg8Uint)
        };
        Ok(Self {
            layout,
            buffer,
            luma: plane(
                "lumen Vulkan decoded luma",
                lumen_gpu::Size::new(layout.width, layout.height),
                luma_format,
            ),
            chroma: plane(
                "lumen Vulkan decoded chroma",
                lumen_gpu::Size::new(chroma_width, chroma_height),
                chroma_format,
            ),
        })
    }
}
