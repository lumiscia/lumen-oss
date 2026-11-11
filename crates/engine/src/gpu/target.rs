#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_ffmpeg::{
    CudaDriver, CudaVideoFrame, GpuVideoInput, ImportedCudaExternalBuffer,
    import_owned_vulkan_opaque_fd_buffer,
};
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_ffmpeg::{GpuVideoInput, MetalPixelBufferFrame, MetalPixelBufferPool, MetalTextureCache};
#[cfg(all(target_os = "linux", feature = "vulkan"))]
use lumen_ffmpeg::{
    VulkanDevice, VulkanDeviceDesc, VulkanFrameLayout, VulkanRenderQueue, VulkanVideoFrame, ash::vk,
};

use crate::error::RenderError;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
use crate::{
    composition::BitDepth,
    gpu::{GpuCompositionRenderer, YuvConverter, YuvFormat, YuvLayout},
};

/// Converts rendered frames to NV12 or P010 for a Vulkan Video encoder sharing the renderer's
/// device. Frames render to `Rgba16Float`, so colour is quantised once, in the conversion.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
pub struct VulkanVideoTarget {
    converter: YuvConverter,
}

#[cfg(all(target_os = "linux", feature = "vulkan"))]
impl GpuCompositionRenderer {
    /// Creates a renderer on a Vulkan device that FFmpeg's Vulkan Video encoders can share, and its
    /// decoders too where the hardware has them. Uses [`lumen_gpu::GpuSettings::from_env`].
    pub async fn new_vulkan_video() -> crate::Result<(Self, VulkanDevice)> {
        Self::from_vulkan_video(lumen_gpu::Renderer::new_vulkan_video().await)
    }

    /// [`Self::new_vulkan_video`] reading no environment variables.
    pub async fn new_vulkan_video_with_settings(
        settings: &lumen_gpu::GpuSettings,
    ) -> crate::Result<(Self, VulkanDevice)> {
        Self::from_vulkan_video(lumen_gpu::Renderer::new_vulkan_video_with_settings(settings).await)
    }

    /// Creates a renderer on a Vulkan device that FFmpeg's Vulkan Video decoders can share, and its
    /// encoders too where the hardware has them. Uses [`lumen_gpu::GpuSettings::from_env`].
    pub async fn new_vulkan_decode() -> crate::Result<(Self, VulkanDevice)> {
        Self::from_vulkan_video(lumen_gpu::Renderer::new_vulkan_decode().await)
    }

    /// [`Self::new_vulkan_decode`] reading no environment variables.
    pub async fn new_vulkan_decode_with_settings(
        settings: &lumen_gpu::GpuSettings,
    ) -> crate::Result<(Self, VulkanDevice)> {
        Self::from_vulkan_video(
            lumen_gpu::Renderer::new_vulkan_decode_with_settings(settings).await,
        )
    }

    fn from_vulkan_video(
        created: anyhow::Result<(lumen_gpu::Renderer, lumen_gpu::VulkanVideoDevice)>,
    ) -> crate::Result<(Self, VulkanDevice)> {
        let (renderer, video) = created.map_err(|error| RenderError::Gpu {
            details: format!("{error:#}"),
        })?;
        let desc = VulkanDeviceDesc {
            get_instance_proc_addr: video.get_instance_proc_addr,
            instance: video.instance,
            physical_device: video.physical_device,
            device: video.device,
            device_extensions: &video.device_extensions,
            queue_families: &video.queue_families,
        };
        let render_queue = std::sync::Arc::new(WgpuRenderQueue {
            device: renderer.device.clone(),
            queue: renderer.queue.clone(),
        });
        // SAFETY: The handles come from the wgpu device, which `render_queue` retains. wgpu's queue
        // is in the render family, which `new_vulkan_video` lists with only graphics capabilities,
        // and `render_queue` submits to it through wgpu.
        let device = unsafe { VulkanDevice::from_raw(&desc, render_queue) }.map_err(|error| {
            RenderError::Gpu {
                details: error.to_string(),
            }
        })?;
        Ok((Self::from_renderer(renderer), device))
    }
}

/// Submits frame copies between FFmpeg and the renderer through wgpu, which serialises them with the renderer's own
/// submissions.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
struct WgpuRenderQueue {
    device: lumen_gpu::wgpu::Device,
    queue: lumen_gpu::wgpu::Queue,
}

#[cfg(all(target_os = "linux", feature = "vulkan"))]
impl VulkanRenderQueue for WgpuRenderQueue {
    fn submit(
        &self,
        record: &mut dyn FnMut(vk::CommandBuffer),
        waits: &[(vk::Semaphore, u64)],
        signals: &[(vk::Semaphore, u64)],
        retain: Box<dyn Send>,
    ) -> Result<(), String> {
        // SAFETY: lumen-ffmpeg records commands on this device and `retain` keeps what they use
        // alive until wgpu reports the submission done.
        unsafe { lumen_gpu::submit_raw_vulkan(&self.device, &self.queue, record, waits, signals) }
            .map_err(|error| format!("{error:#}"))?;
        self.queue.on_submitted_work_done(move || drop(retain));
        Ok(())
    }
}

#[cfg(all(target_os = "linux", feature = "vulkan"))]
impl VulkanVideoTarget {
    /// The format frames for this target render to.
    pub const FORMAT: lumen_gpu::wgpu::TextureFormat = lumen_gpu::wgpu::TextureFormat::Rgba16Float;

    /// A target for frames of `size`, NV12 at 8 bits and P010 at 10, which adds the ordered
    /// dither as it quantises if `dither`.
    pub fn new(
        renderer: &lumen_gpu::Renderer,
        size: lumen_gpu::Size,
        depth: BitDepth,
        dither: bool,
    ) -> Self {
        let layout = YuvLayout::new(size.width, size.height, YuvFormat::semi_planar(depth));
        Self {
            converter: YuvConverter::new(renderer, layout, dither),
        }
    }

    /// Submits the conversion of `texture` and returns the frame for the encoder. The frame must
    /// be sent before the next conversion, which reuses the buffer.
    pub fn convert(
        &self,
        renderer: &lumen_gpu::Renderer,
        texture: &lumen_gpu::wgpu::Texture,
        pts: Option<i64>,
    ) -> crate::Result<VulkanVideoFrame> {
        self.converter.submit(renderer, texture);
        let device =
            lumen_gpu::raw_vulkan_device(&renderer.device).map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let yuv = self.converter.buffer();
        let buffer = lumen_gpu::raw_vulkan_buffer(yuv).map_err(|error| RenderError::Gpu {
            details: error.to_string(),
        })?;
        let YuvLayout {
            width,
            height,
            format,
        } = self.converter.layout();
        let layout = match format.depth() {
            BitDepth::Eight => VulkanFrameLayout::nv12(width, height),
            BitDepth::Ten => VulkanFrameLayout::p010(width, height),
        };
        // SAFETY: The buffer lives on `device`, the conversion writing it in `layout` was just
        // submitted to that device's render queue, and the frame retains the buffer, which retains
        // the device.
        Ok(unsafe { VulkanVideoFrame::new(device, buffer, layout, pts, yuv.clone()) })
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
pub struct CudaNvencTargetPool {
    driver: CudaDriver,
    cuda_ordinal: i32,
    size: lumen_gpu::Size,
    depth: BitDepth,
    dither: bool,
    vulkan_device: lumen_gpu::VulkanDeviceInfo,
}

/// A float RGBA render target whose frames are converted to NV12, or P010 at 10 bits, in Vulkan
/// memory that CUDA maps, for NVENC to read.
#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
pub struct CudaNvencTarget {
    external: lumen_gpu::ExternalTexture,
    imported: ImportedCudaExternalBuffer,
    converter: YuvConverter,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
pub struct MetalVideoToolboxTargetPool {
    texture_cache: MetalTextureCache,
    pixel_buffer_pool: MetalPixelBufferPool,
    size: lumen_gpu::Size,
    format: lumen_gpu::wgpu::TextureFormat,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
pub struct MetalVideoToolboxTarget {
    pixel_frame: MetalPixelBufferFrame,
    external: lumen_gpu::ExternalTexture,
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl CudaNvencTargetPool {
    /// A pool of targets for frames of `size`, NV12 at 8 bits and P010 at 10, which add the
    /// ordered dither as they quantise if `dither`.
    pub fn new(
        renderer: &lumen_gpu::Renderer,
        size: lumen_gpu::Size,
        depth: BitDepth,
        dither: bool,
    ) -> crate::Result<Self> {
        let vulkan_device =
            renderer
                .vulkan_device_info()
                .map_err(|error| RenderError::Internal {
                    details: error.to_string(),
                })?;
        let driver = CudaDriver::load().map_err(|details| RenderError::Gpu { details })?;
        let cuda_ordinal = driver
            .devices()
            .map_err(|details| RenderError::Gpu { details })?
            .into_iter()
            .find(|device| device.uuid == vulkan_device.device_uuid)
            .map(|device| device.ordinal)
            .ok_or_else(|| RenderError::Gpu {
                details: format!(
                    "no CUDA device UUID matched Vulkan device {} ({})",
                    vulkan_device.name,
                    format_uuid(&vulkan_device.device_uuid)
                ),
            })?;
        Ok(Self {
            driver,
            cuda_ordinal,
            size,
            depth,
            dither,
            vulkan_device,
        })
    }

    pub fn acquire(&self, renderer: &lumen_gpu::Renderer) -> crate::Result<CudaNvencTarget> {
        use lumen_gpu::wgpu;

        let context = self
            .driver
            .create_primary_context_for_ordinal(self.cuda_ordinal)
            .map_err(|details| RenderError::Gpu { details })?;
        let texture = renderer.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lumen CUDA/NVENC render target"),
            size: self.size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: self.format(),
            usage: cuda_nvenc_usage(),
            view_formats: &[],
        });
        let layout = YuvLayout::new(
            self.size.width,
            self.size.height,
            YuvFormat::semi_planar(self.depth),
        );
        let exportable = renderer
            .create_exportable_vulkan_buffer(
                Some("lumen CUDA/NVENC YUV frame"),
                layout.size(),
                wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let imported = import_owned_vulkan_opaque_fd_buffer(
            &context,
            exportable
                .memory_fd()
                .try_clone()
                .map_err(|error| RenderError::Gpu {
                    details: format!("failed to duplicate Vulkan memory fd: {error}"),
                })?,
            exportable.allocation_size(),
            layout.size(),
        )
        .map_err(|details| RenderError::Gpu { details })?;
        Ok(CudaNvencTarget {
            external: lumen_gpu::ExternalTexture::from_texture(texture),
            imported,
            converter: YuvConverter::with_buffer(
                renderer,
                layout,
                exportable.buffer_arc(),
                self.dither,
            ),
        })
    }

    pub fn format(&self) -> lumen_gpu::wgpu::TextureFormat {
        VulkanVideoTarget::FORMAT
    }

    pub fn size(&self) -> lumen_gpu::Size {
        self.size
    }

    pub fn vulkan_device(&self) -> &lumen_gpu::VulkanDeviceInfo {
        &self.vulkan_device
    }

    pub fn cuda_ordinal(&self) -> i32 {
        self.cuda_ordinal
    }

    pub fn driver(&self) -> &CudaDriver {
        &self.driver
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl CudaNvencTarget {
    pub fn external_texture(&self) -> lumen_gpu::ExternalTexture {
        self.external.clone()
    }

    /// Submits the conversion of the rendered frame. Once the submission has finished,
    /// [`video_frame`](Self::video_frame) holds the frame until the next conversion.
    pub fn convert(&self, renderer: &lumen_gpu::Renderer) -> lumen_gpu::wgpu::SubmissionIndex {
        self.converter
            .submit(renderer, &self.external.texture_arc())
    }

    pub fn video_frame(&self, pts: Option<i64>) -> crate::Result<CudaVideoFrame> {
        let layout = self.converter.layout();
        let (width, height, pitch) = (layout.width, layout.height, u64::from(layout.pitch()));
        let frame = match layout.format.depth() {
            BitDepth::Eight => self.imported.nv12_frame(width, height, pitch, pts),
            BitDepth::Ten => self.imported.p010_frame(width, height, pitch, pts),
        };
        Ok(frame.map_err(|details| RenderError::Gpu { details })?)
    }

    pub fn video_input<'a>(&'a self, frame: &'a CudaVideoFrame) -> GpuVideoInput<'a> {
        GpuVideoInput::Cuda(frame)
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn cuda_nvenc_usage() -> lumen_gpu::wgpu::TextureUsages {
    lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT
        | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
        | lumen_gpu::wgpu::TextureUsages::STORAGE_BINDING
        | lumen_gpu::wgpu::TextureUsages::COPY_DST
        | lumen_gpu::wgpu::TextureUsages::COPY_SRC
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn format_uuid(uuid: &[u8; 16]) -> String {
    uuid.iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<Vec<_>>()
        .join("")
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl MetalVideoToolboxTargetPool {
    pub fn bgra8(renderer: &lumen_gpu::Renderer, size: lumen_gpu::Size) -> crate::Result<Self> {
        let metal_device =
            lumen_gpu::metal_device_from_wgpu(&renderer.device).map_err(|error| {
                RenderError::Internal {
                    details: error.to_string(),
                }
            })?;
        let texture_cache =
            MetalTextureCache::create(&metal_device).map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let pixel_buffer_pool =
            MetalPixelBufferPool::bgra8(size.width, size.height).map_err(|error| {
                RenderError::Gpu {
                    details: error.to_string(),
                }
            })?;
        Ok(Self {
            texture_cache,
            pixel_buffer_pool,
            size,
            format: lumen_gpu::wgpu::TextureFormat::Bgra8Unorm,
        })
    }

    pub fn acquire(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        frame: u32,
    ) -> crate::Result<MetalVideoToolboxTarget> {
        let pixel_frame = self
            .pixel_buffer_pool
            .create_frame(Some(i64::from(frame)))
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let metal_texture = pixel_frame
            .create_texture(
                &self.texture_cache,
                objc2_metal::MTLPixelFormat::BGRA8Unorm,
                0,
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        // SAFETY: the texture is a 2D single-layer, single-mip, single-sample Metal texture on the
        // renderer's device, created at this size and format.
        let texture = unsafe {
            lumen_gpu::texture_from_metal(
                &renderer.device,
                metal_texture,
                &lumen_gpu::wgpu::TextureDescriptor {
                    label: Some("lumen VideoToolbox render target"),
                    size: self.size.as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: lumen_gpu::wgpu::TextureDimension::D2,
                    format: self.format,
                    usage: external_output_usage(),
                    view_formats: &[],
                },
                None,
            )
        };
        Ok(MetalVideoToolboxTarget {
            pixel_frame,
            external: lumen_gpu::ExternalTexture::from_texture(texture),
        })
    }

    pub fn format(&self) -> lumen_gpu::wgpu::TextureFormat {
        self.format
    }

    pub fn size(&self) -> lumen_gpu::Size {
        self.size
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl MetalVideoToolboxTarget {
    pub fn external_texture(&self) -> lumen_gpu::ExternalTexture {
        self.external.clone()
    }

    pub fn video_input(&self) -> GpuVideoInput<'_> {
        GpuVideoInput::MetalPixelBuffer(&self.pixel_frame)
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn external_output_usage() -> lumen_gpu::wgpu::TextureUsages {
    lumen_gpu::wgpu::TextureUsages::RENDER_ATTACHMENT
        | lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
        | lumen_gpu::wgpu::TextureUsages::COPY_SRC
        | lumen_gpu::wgpu::TextureUsages::COPY_DST
}

#[cfg(all(test, target_os = "linux", feature = "vulkan"))]
mod tests {
    use lumen_ffmpeg::ash::vk::Handle;
    use lumen_ffmpeg::{
        EncodeMode, GpuBackend, GpuVideoInput, InputContext, MuxedEncoder, VideoCodec,
        VideoEncoderConfig,
    };
    use lumen_gpu::wgpu;

    use super::*;

    fn read_nv12(renderer: &lumen_gpu::Renderer, target: &VulkanVideoTarget) -> Vec<u8> {
        let readback = renderer.device.create_buffer(&wgpu::BufferDescriptor {
            label: None,
            size: target.converter.buffer().size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        let nv12 = target.converter.buffer();
        encoder.copy_buffer_to_buffer(nv12, 0, &readback, 0, nv12.size());
        renderer.queue.submit([encoder.finish()]);
        readback.slice(..).map_async(wgpu::MapMode::Read, |_| {});
        renderer
            .device
            .poll(wgpu::PollType::wait_indefinitely())
            .expect("wait for NV12 readback");
        readback.slice(..).get_mapped_range().expect("map").to_vec()
    }

    #[test]
    fn vulkan_video_target_encodes_rendered_texture() {
        let (renderer, device) = match pollster::block_on(GpuCompositionRenderer::new_vulkan_video())
        {
            Ok(created) => created,
            Err(error) => {
                eprintln!("Vulkan Video unavailable ({error}); skipping zero-copy test");
                return;
            }
        };
        let gpu = renderer.gpu_renderer();
        let (width, height) = (256, 144);
        let size = lumen_gpu::Size::new(width, height);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            &[255, 0, 0, 255].repeat((width * height) as usize),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            size.as_extent(),
        );
        let target = VulkanVideoTarget::new(gpu, size, BitDepth::Eight, false);

        target.convert(gpu, &texture, None).expect("convert");
        let nv12 = read_nv12(gpu, &target);
        let chroma = VulkanVideoFrame::nv12_chroma_offset(width, height) as usize;
        // Pure red in BT.709 limited range.
        assert!(nv12[..chroma].iter().all(|&luma| luma == 63));
        assert!(nv12[chroma..].chunks(2).all(|cbcr| cbcr == [102, 240]));

        let mut config = VideoEncoderConfig::cpu_rgba(width, height, 30, "av1_vulkan");
        config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);
        config.vulkan_device = Some(device);
        let path =
            std::env::temp_dir().join(format!("lumen_vulkan_zero_copy_{}.mkv", std::process::id()));
        let mut encoder = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
            Ok(encoder) => encoder,
            // AVERROR(ENOSYS): the device has an encode queue but no AV1 encode support.
            Err(error)
                if error.operation == "avcodec_find_encoder_by_name" || error.code == Some(-38) =>
            {
                eprintln!("av1_vulkan unavailable ({error}); skipping zero-copy encode");
                return;
            }
            Err(error) => panic!("av1_vulkan is available but failed to initialise: {error}"),
        };
        // SAFETY: The encoder rejects the frame's foreign device before recording any command.
        let foreign = unsafe {
            VulkanVideoFrame::new(
                vk::Device::from_raw(1),
                vk::Buffer::from_raw(1),
                VulkanFrameLayout::nv12(width, height),
                None,
                std::sync::Arc::new(()),
            )
        };
        let error = encoder
            .write_gpu_frame(&GpuVideoInput::Vulkan(&foreign))
            .expect_err("a frame from another device must be rejected");
        assert!(error.message.contains("device"), "{error}");

        for frame in 0..10 {
            let video_frame = target.convert(gpu, &texture, Some(frame)).expect("convert");
            encoder
                .write_gpu_frame(&GpuVideoInput::Vulkan(&video_frame))
                .expect("encode Vulkan frame");
        }
        encoder.finish().expect("finish encode");

        let info = InputContext::open(path.to_string_lossy().to_string())
            .expect("open encoded file")
            .media_info();
        let stream = &info.video_streams[0];
        assert_eq!(stream.codec, VideoCodec::Av1);
        assert_eq!((stream.width, stream.height), (width, height));
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn vulkan_video_target_encodes_ten_bit_or_says_why_not() {
        let (renderer, device) = match pollster::block_on(GpuCompositionRenderer::new_vulkan_video())
        {
            Ok(created) => created,
            Err(error) => {
                eprintln!("Vulkan Video unavailable ({error}); skipping 10-bit zero-copy test");
                return;
            }
        };
        let gpu = renderer.gpu_renderer();
        let (width, height) = (256, 144);
        let size = lumen_gpu::Size::new(width, height);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        gpu.queue.write_texture(
            texture.as_image_copy(),
            &[255, 255, 255, 255].repeat((width * height) as usize),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(width * 4),
                rows_per_image: None,
            },
            size.as_extent(),
        );
        let target = VulkanVideoTarget::new(gpu, size, BitDepth::Ten, false);
        target.convert(gpu, &texture, None).expect("convert");
        let p010 = read_nv12(gpu, &target);
        let layout = VulkanFrameLayout::p010(width, height);
        assert_eq!(target.converter.buffer().size(), layout.size());
        let sample = |offset: usize| u16::from_le_bytes([p010[offset], p010[offset + 1]]);
        // White in 10-bit limited range, in the high bits.
        assert_eq!(sample(0), 940 << 6);
        assert_eq!(sample(layout.chroma_offset() as usize), 512 << 6);
        assert_eq!(sample(layout.chroma_offset() as usize + 2), 512 << 6);

        for encoder in ["av1_vulkan", "hevc_vulkan"] {
            let mut config = VideoEncoderConfig::cpu_rgba(width, height, 30, encoder);
            config.mode = EncodeMode::GpuTexture(GpuBackend::Vulkan);
            config.vulkan_device = Some(device.clone());
            config.bit_depth = lumen_ffmpeg::BitDepth::Ten;
            let path = std::env::temp_dir().join(format!(
                "lumen_vulkan_10bit_{encoder}_{}.mkv",
                std::process::id()
            ));
            let mut muxed = match MuxedEncoder::create(path.to_string_lossy().to_string(), config) {
                Ok(muxed) => muxed,
                Err(error) => {
                    // The device has no 10-bit encode for this codec: the error says so, and no
                    // 8-bit stream is written in its place.
                    eprintln!("{encoder} can't encode 10-bit here: {error}");
                    assert!(!path.exists() || std::fs::metadata(&path).unwrap().len() == 0);
                    let _ = std::fs::remove_file(&path);
                    continue;
                }
            };
            let eight_bit = VulkanVideoTarget::new(gpu, size, BitDepth::Eight, false)
                .convert(gpu, &texture, Some(0))
                .expect("convert");
            let error = muxed
                .write_gpu_frame(&GpuVideoInput::Vulkan(&eight_bit))
                .expect_err("an NV12 frame must not reach a 10-bit encoder");
            assert!(error.message.contains("8-bit"), "{error}");
            for frame in 0..5 {
                let video_frame = target.convert(gpu, &texture, Some(frame)).expect("convert");
                muxed
                    .write_gpu_frame(&GpuVideoInput::Vulkan(&video_frame))
                    .expect("encode Vulkan frame");
            }
            muxed.finish().expect("finish encode");
            let info = InputContext::open(path.to_string_lossy().to_string())
                .expect("open encoded file")
                .media_info();
            assert_eq!(
                info.video_streams[0].pixel_format,
                lumen_ffmpeg::PixelFormat::Yuv420p10,
                "{encoder}"
            );
            eprintln!("{encoder} wrote 10-bit {}", path.display());
            let _ = std::fs::remove_file(path);
        }
    }

    /// The half float nearest below `value`, which must be a positive normal one.
    fn f16_bits(value: f32) -> u16 {
        let bits = value.to_bits();
        ((((bits >> 23) & 0xff) - 112) << 10 | ((bits >> 13) & 0x3ff)) as u16
    }

    fn f16_value(bits: u16) -> f32 {
        let exponent = i32::from(bits >> 10) - 15;
        (1.0 + f32::from(bits & 0x3ff) / 1024.0) * 2f32.powi(exponent)
    }

    fn srgb_encode(linear: f32) -> f32 {
        if linear <= 0.003_130_8 {
            linear * 12.92
        } else {
            1.055 * linear.powf(1.0 / 2.4) - 0.055
        }
    }

    #[test]
    fn the_nv12_dither_keeps_the_mean_of_fractional_luma() {
        let gpu = match crate::gpu::test_device::shared() {
            Ok(gpu) => gpu,
            Err(error) => {
                eprintln!("no GPU ({error}); skipping NV12 dither test");
                return;
            }
        };
        let size = lumen_gpu::Size::new(16, 16);
        let texture = gpu.device.create_texture(&wgpu::TextureDescriptor {
            label: None,
            size: size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        let target = VulkanVideoTarget::new(&gpu, size, BitDepth::Eight, true);
        let pitch = VulkanVideoFrame::nv12_pitch(size.width) as usize;
        for fraction in [0.2, 0.25, 0.4, 0.5, 0.6, 0.75, 0.8] {
            let grey = f16_bits(crate::gpu::color::srgb_to_linear(
                (60.0 + fraction - 16.0) / 219.0,
            ));
            let luma = 16.0 + 219.0 * srgb_encode(f16_value(grey));
            let texel: Vec<u8> = [grey, grey, grey, f16_bits(1.0)]
                .iter()
                .flat_map(|half| half.to_le_bytes())
                .collect();
            gpu.queue.write_texture(
                texture.as_image_copy(),
                &texel.repeat((size.width * size.height) as usize),
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(size.width * 8),
                    rows_per_image: None,
                },
                size.as_extent(),
            );
            target.converter.submit(&gpu, &texture);
            let nv12 = read_nv12(&gpu, &target);
            let sum: f64 = (0..size.height as usize)
                .flat_map(|row| &nv12[row * pitch..row * pitch + size.width as usize])
                .map(|&code| f64::from(code))
                .sum();
            let mean = sum / (size.width * size.height) as usize as f64;
            assert!(
                (mean - f64::from(luma)).abs() <= 1.0 / 64.0 + 0.01,
                "luma {luma} averaged {mean}"
            );
        }
    }
}
