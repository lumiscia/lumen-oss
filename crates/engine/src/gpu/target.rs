#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use std::sync::OnceLock;

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use lumen_ffmpeg::{
    CudaContext, CudaDeviceAllocation, CudaDriver, CudaVideoFrame, GpuVideoInput,
    ImportedCudaExternalImage, import_owned_vulkan_opaque_fd_image,
};
#[cfg(all(target_os = "macos", feature = "metal"))]
use lumen_ffmpeg::{GpuVideoInput, MetalPixelBufferFrame, MetalPixelBufferPool, MetalTextureCache};
#[cfg(all(target_os = "linux", feature = "vulkan"))]
use lumen_ffmpeg::{VulkanDevice, VulkanDeviceDesc, VulkanRenderQueue, VulkanVideoFrame, ash::vk};

use crate::error::RenderError;
#[cfg(all(target_os = "linux", feature = "vulkan"))]
use crate::gpu::GpuCompositionRenderer;

/// Converts rendered RGBA frames to NV12 for a Vulkan Video encoder sharing the renderer's device.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
pub struct VulkanVideoTarget {
    pipeline: lumen_gpu::wgpu::ComputePipeline,
    params: lumen_gpu::wgpu::Buffer,
    nv12: std::sync::Arc<lumen_gpu::wgpu::Buffer>,
    size: lumen_gpu::Size,
}

#[cfg(all(target_os = "linux", feature = "vulkan"))]
impl GpuCompositionRenderer {
    /// Creates a renderer on a Vulkan device that FFmpeg's Vulkan Video encoders can share.
    pub async fn new_vulkan_video() -> crate::Result<(Self, VulkanDevice)> {
        let (renderer, video) = lumen_gpu::Renderer::new_vulkan_video()
            .await
            .map_err(|error| RenderError::Gpu {
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

/// Submits FFmpeg's frame copies through wgpu, which serialises them with the renderer's own
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
    pub fn nv12(renderer: &lumen_gpu::Renderer, size: lumen_gpu::Size) -> Self {
        use lumen_gpu::wgpu;

        let device = &renderer.device;
        let module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("lumen RGBA to NV12"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shaders/rgba_to_nv12.wgsl").into()),
        });
        let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
            label: Some("lumen RGBA to NV12"),
            layout: None,
            module: &module,
            entry_point: Some("main"),
            compilation_options: Default::default(),
            cache: None,
        });
        let pitch = VulkanVideoFrame::nv12_pitch(size.width);
        let chroma_offset = VulkanVideoFrame::nv12_chroma_offset(size.width, size.height) as u32;
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen NV12 params"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        renderer.queue.write_buffer(
            &params,
            0,
            bytemuck::cast_slice(&[size.width, size.height, pitch, chroma_offset]),
        );
        let nv12 = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen NV12 frame"),
            size: VulkanVideoFrame::nv12_size(size.width, size.height),
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        Self {
            pipeline,
            params,
            nv12: std::sync::Arc::new(nv12),
            size,
        }
    }

    /// Submits the conversion of `texture` and returns the NV12 frame for the encoder. The frame
    /// must be sent before the next conversion, which reuses the buffer.
    pub fn convert(
        &self,
        renderer: &lumen_gpu::Renderer,
        texture: &lumen_gpu::wgpu::Texture,
        pts: Option<i64>,
    ) -> crate::Result<VulkanVideoFrame> {
        use lumen_gpu::wgpu;

        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = renderer
            .device
            .create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("lumen RGBA to NV12"),
                layout: &self.pipeline.get_bind_group_layout(0),
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: wgpu::BindingResource::TextureView(&view),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: self.nv12.as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: self.params.as_entire_binding(),
                    },
                ],
            });
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen RGBA to NV12"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("lumen RGBA to NV12"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.dispatch_workgroups(
                self.size.width.div_ceil(4).div_ceil(8),
                self.size.height.div_ceil(2).div_ceil(8),
                1,
            );
        }
        renderer.queue.submit([encoder.finish()]);

        let device =
            lumen_gpu::raw_vulkan_device(&renderer.device).map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let buffer =
            lumen_gpu::raw_vulkan_buffer(&self.nv12).map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let (width, height) = (self.size.width, self.size.height);
        // SAFETY: The buffer lives on `device`, the conversion writing it was just submitted to
        // that device's render queue, and the frame retains the buffer, which retains the device.
        Ok(unsafe { VulkanVideoFrame::new(device, buffer, width, height, pts, self.nv12.clone()) })
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
pub struct CudaNvencTargetPool {
    driver: &'static CudaDriver,
    cuda_ordinal: i32,
    size: lumen_gpu::Size,
    format: lumen_gpu::wgpu::TextureFormat,
    vulkan_device: lumen_gpu::VulkanDeviceInfo,
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
pub struct CudaNvencTarget {
    driver: &'static CudaDriver,
    external: lumen_gpu::ExternalTexture,
    cuda_frame: CudaDeviceAllocation<'static>,
    imported: ImportedCudaExternalImage,
    exportable: lumen_gpu::ExportableVulkanTexture,
    context: CudaContext<'static>,
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
    pub fn rgba8(renderer: &lumen_gpu::Renderer, size: lumen_gpu::Size) -> crate::Result<Self> {
        let bootstrap = renderer
            .create_exportable_vulkan_texture(
                Some("lumen CUDA/NVENC target bootstrap"),
                size,
                lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                cuda_nvenc_usage(),
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let vulkan_device = bootstrap.device_info().clone();
        let driver = cuda_driver().map_err(|details| RenderError::Gpu { details })?;
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
        drop(bootstrap);
        Ok(Self {
            driver,
            cuda_ordinal,
            size,
            format: lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            vulkan_device,
        })
    }

    pub fn acquire(&self, renderer: &lumen_gpu::Renderer) -> crate::Result<CudaNvencTarget> {
        let context = self
            .driver
            .create_primary_context_for_ordinal(self.cuda_ordinal)
            .map_err(|details| RenderError::Gpu { details })?;
        let exportable = renderer
            .create_exportable_vulkan_texture(
                Some("lumen CUDA/NVENC render target"),
                self.size,
                self.format,
                cuda_nvenc_usage(),
            )
            .map_err(|error| RenderError::Gpu {
                details: error.to_string(),
            })?;
        let imported = import_owned_vulkan_opaque_fd_image(
            self.driver,
            exportable
                .memory_fd()
                .try_clone()
                .map_err(|error| RenderError::Gpu {
                    details: format!("failed to duplicate Vulkan memory fd: {error}"),
                })?,
            exportable.allocation_size(),
            self.size.width,
            self.size.height,
        )
        .map_err(|details| RenderError::Gpu { details })?;
        let cuda_frame = self
            .driver
            .allocate_rgba_frame(self.size.width, self.size.height)
            .map_err(|details| RenderError::Gpu { details })?;
        let external = lumen_gpu::ExternalTexture::new(
            exportable.texture_arc(),
            lumen_gpu::TextureDesc {
                domain: lumen_gpu::TextureDomain::full_frame(self.size),
                format: self.format,
                usage: cuda_nvenc_usage(),
            },
        );
        Ok(CudaNvencTarget {
            driver: self.driver,
            external,
            cuda_frame,
            imported,
            exportable,
            context,
        })
    }

    pub fn format(&self) -> lumen_gpu::wgpu::TextureFormat {
        self.format
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

    pub fn driver(&self) -> &'static CudaDriver {
        self.driver
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl CudaNvencTarget {
    pub fn external_texture(&self) -> lumen_gpu::ExternalTexture {
        self.external.clone()
    }

    pub fn copy_rendered_frame_to_cuda(&self) -> crate::Result<()> {
        self.context
            .set_current()
            .map_err(|details| RenderError::Gpu { details })?;
        Ok(self
            .driver
            .copy_image_to_rgba_frame(&self.imported, &self.cuda_frame)
            .map_err(|details| RenderError::Gpu { details })?)
    }

    pub fn video_frame(&self, pts: Option<i64>) -> CudaVideoFrame {
        self.cuda_frame.as_video_frame(pts)
    }

    pub fn video_input<'a>(&'a self, frame: &'a CudaVideoFrame) -> GpuVideoInput<'a> {
        GpuVideoInput::Cuda(frame)
    }

    pub fn allocation_size(&self) -> u64 {
        self.exportable.allocation_size()
    }

    pub fn row_pitch(&self) -> u64 {
        self.exportable.row_pitch()
    }

    pub fn memory_fd_raw(&self) -> i32 {
        self.exportable.memory_fd_raw()
    }

    pub fn memory_type_index(&self) -> u32 {
        self.exportable.memory_type_index()
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl Drop for CudaNvencTarget {
    fn drop(&mut self) {
        let _ = self.context.set_current();
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn cuda_driver() -> Result<&'static CudaDriver, String> {
    static CUDA_DRIVER: OnceLock<CudaDriver> = OnceLock::new();
    if let Some(driver) = CUDA_DRIVER.get() {
        return Ok(driver);
    }
    let _ = CUDA_DRIVER.set(CudaDriver::load()?);
    CUDA_DRIVER
        .get()
        .ok_or_else(|| "CUDA driver was not initialized".to_string())
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
                RenderError::Gpu {
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
        let texture = lumen_gpu::texture_from_metal(
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
        );
        Ok(MetalVideoToolboxTarget {
            pixel_frame,
            external: lumen_gpu::ExternalTexture::from_texture(
                texture,
                lumen_gpu::TextureDesc {
                    domain: lumen_gpu::TextureDomain::full_frame(self.size),
                    format: self.format,
                    usage: external_output_usage(),
                },
            ),
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
            size: target.nv12.size(),
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor::default());
        encoder.copy_buffer_to_buffer(&target.nv12, 0, &readback, 0, target.nv12.size());
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
        let target = VulkanVideoTarget::nv12(gpu, size);

        target.convert(gpu, &texture, None).expect("convert");
        let nv12 = read_nv12(gpu, &target);
        let chroma = VulkanVideoFrame::nv12_chroma_offset(width, height) as usize;
        // Pure red in BT.709 limited range.
        assert!(nv12[..chroma].iter().all(|&luma| luma == 63));
        assert!(nv12[chroma..].chunks(2).all(|cbcr| cbcr == [102, 240]));

        let mut config = VideoEncoderConfig::cpu_rgba(width, height, 30, VideoCodec::Av1);
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
                width,
                height,
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
}
