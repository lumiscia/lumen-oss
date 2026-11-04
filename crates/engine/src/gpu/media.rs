use std::sync::Arc;

#[cfg(all(target_os = "linux", feature = "vulkan"))]
use std::collections::HashMap;

use super::types::MediaTextureUpload;

/// The textures a plan's imported frames are held in, by the plan's texture they stand in for.
#[derive(Default)]
pub(super) struct GpuMediaImports {
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    vulkan: HashMap<lumen_gpu::TextureId, VulkanMediaTexture>,
}

impl GpuMediaImports {
    /// Frees the textures frames were imported into under `id`.
    pub(super) fn remove(&mut self, id: lumen_gpu::TextureId) {
        #[cfg(all(target_os = "linux", feature = "vulkan"))]
        self.vulkan.remove(&id);
        #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
        let _ = id;
    }
}

#[derive(Default)]
pub(crate) struct GpuMediaFrameImporter {
    #[cfg(all(target_os = "macos", feature = "metal"))]
    metal: Option<MetalMediaImporter>,
    #[cfg(all(target_os = "linux", feature = "vulkan"))]
    vulkan: Option<VulkanMediaImporter>,
}

impl GpuMediaFrameImporter {
    /// Imports a bound frame's GPU frames, in upload order, before any reaches a texture. A failed
    /// import settles its source on the CPU, so the source's other frames here go there with it
    /// instead of mixing the two in one output frame.
    pub(super) fn import_all(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        imports: &mut GpuMediaImports,
        uploads: &[MediaTextureUpload],
    ) -> Vec<Result<Arc<lumen_gpu::wgpu::Texture>, String>> {
        uploads
            .iter()
            .filter_map(|upload| {
                let crate::media::MediaFrame::GpuVideo(frame) = &upload.frame else {
                    return None;
                };
                let imported = self.import(renderer, imports, upload, frame);
                if imported.is_err() {
                    frame.settle_on_cpu();
                }
                Some(imported)
            })
            .collect()
    }

    pub(super) fn import(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        imports: &mut GpuMediaImports,
        upload: &MediaTextureUpload,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
        #[cfg(not(all(target_os = "linux", feature = "vulkan")))]
        let _ = (imports, upload);
        #[cfg(not(any(
            all(target_os = "linux", feature = "vulkan"),
            all(target_os = "macos", feature = "metal")
        )))]
        let _ = renderer;

        match frame.frame.backend() {
            #[cfg(all(target_os = "macos", feature = "metal"))]
            lumen_ffmpeg::GpuBackend::Metal => self.import_metal(renderer, frame),
            #[cfg(all(target_os = "linux", feature = "vulkan"))]
            lumen_ffmpeg::GpuBackend::Vulkan => self
                .vulkan
                .get_or_insert_with(|| VulkanMediaImporter::new(renderer))
                .import_frame(renderer, &mut imports.vulkan, upload, frame),
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
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
        if self.metal.is_none() {
            self.metal = Some(MetalMediaImporter::new(renderer)?);
        }
        self.metal
            .as_mut()
            .ok_or_else(|| "Metal media importer was not initialized".to_string())?
            .import_frame(renderer, frame)
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
struct MetalMediaImporter {
    texture_cache: lumen_ffmpeg::MetalTextureCache,
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

    /// The texture holds `frame` until wgpu releases it, which is after the GPU finishes with it.
    fn import_frame(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
        let lumen_ffmpeg::GpuVideoFrame::Metal(decoded) = &frame.frame else {
            return Err(format!(
                "unsupported GPU media backend {:?}; only Metal media frames can be imported here",
                frame.frame.backend()
            ));
        };
        let (width, height) = decoded.dimensions();
        let size = lumen_gpu::Size::new(width.max(1), height.max(1));
        let metal_texture = decoded
            .create_texture(
                &self.texture_cache,
                objc2_metal::MTLPixelFormat::BGRA8Unorm,
                0,
            )
            .map_err(|error| error.to_string())?;
        let owner = Arc::clone(frame);
        // SAFETY: the texture is a 2D single-layer, single-mip, single-sample BGRA8 Metal texture
        // on the renderer's device, created at the decoded frame's size.
        let texture = Arc::new(unsafe {
            lumen_gpu::texture_from_metal(
                &renderer.device,
                metal_texture,
                &lumen_gpu::wgpu::TextureDescriptor {
                    label: Some("lumen Metal decoded media texture"),
                    size: size.as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: lumen_gpu::wgpu::TextureDimension::D2,
                    format: lumen_gpu::wgpu::TextureFormat::Bgra8Unorm,
                    usage: media_texture_usage(),
                    view_formats: &[],
                },
                Some(Box::new(move || drop(owner))),
            )
        });
        Ok(texture)
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

/// Converts frames FFmpeg decoded on the renderer's device from NV12 to RGBA.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
struct VulkanMediaImporter {
    pipeline: lumen_gpu::wgpu::ComputePipeline,
}

#[cfg(all(target_os = "linux", feature = "vulkan"))]
struct VulkanMediaTexture {
    size: lumen_gpu::Size,
    nv12: Arc<lumen_gpu::wgpu::Buffer>,
    rgba: Arc<lumen_gpu::wgpu::Texture>,
    params: lumen_gpu::wgpu::Buffer,
    written_params: Option<lumen_ffmpeg::YuvToRgb>,
    bind_group: lumen_gpu::wgpu::BindGroup,
}

#[cfg(all(target_os = "linux", feature = "vulkan"))]
impl VulkanMediaImporter {
    fn new(renderer: &lumen_gpu::Renderer) -> Self {
        use lumen_gpu::wgpu;

        let module = renderer
            .device
            .create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("lumen NV12 to RGBA"),
                source: wgpu::ShaderSource::Wgsl(include_str!("shaders/nv12_to_rgba.wgsl").into()),
            });
        let pipeline = renderer
            .device
            .create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("lumen NV12 to RGBA"),
                layout: None,
                module: &module,
                entry_point: Some("main"),
                compilation_options: Default::default(),
                cache: None,
            });
        Self { pipeline }
    }

    /// Copies `frame` out of FFmpeg's image and converts it into a texture that stays the plan's
    /// until the frame size changes. Nothing else reads the image, so the frame only has to outlive
    /// the copy, and the copy's semaphore wait and layout change go through FFmpeg's frame lock.
    fn import_frame(
        &self,
        renderer: &lumen_gpu::Renderer,
        textures: &mut HashMap<lumen_gpu::TextureId, VulkanMediaTexture>,
        upload: &MediaTextureUpload,
        frame: &crate::media::GpuVideoMediaFrame,
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
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
        let (width, height) = decoded.dimensions();
        let size = lumen_gpu::Size::new(width, height);
        if textures
            .get(&upload.texture)
            .is_none_or(|texture| texture.size != size)
        {
            let texture = self.create_texture(renderer, size)?;
            textures.insert(upload.texture, texture);
        }
        let texture = textures
            .get_mut(&upload.texture)
            .ok_or_else(|| "Vulkan media texture was not initialized".to_string())?;

        let yuv_to_rgb = decoded.yuv_to_rgb();
        if texture.written_params != Some(yuv_to_rgb) {
            let pitch = lumen_ffmpeg::VulkanVideoFrame::nv12_pitch(width);
            let chroma_offset =
                lumen_ffmpeg::VulkanVideoFrame::nv12_chroma_offset(width, height) as u32;
            let mut params = [0_u8; PARAMS_BYTES as usize];
            params[..16].copy_from_slice(bytemuck::cast_slice(&[
                width,
                height,
                pitch,
                chroma_offset,
            ]));
            params[16..40].copy_from_slice(bytemuck::cast_slice(&[
                yuv_to_rgb.y_scale,
                yuv_to_rgb.y_offset,
                yuv_to_rgb.r_cr,
                yuv_to_rgb.g_cb,
                yuv_to_rgb.g_cr,
                yuv_to_rgb.b_cb,
            ]));
            renderer.queue.write_buffer(&texture.params, 0, &params);
            texture.written_params = Some(yuv_to_rgb);
        }

        let buffer =
            lumen_gpu::raw_vulkan_buffer(&texture.nv12).map_err(|error| format!("{error:#}"))?;
        // SAFETY: the buffer was created on this device for this frame size with COPY_DST, and the
        // retained Arc keeps it alive through the copy.
        unsafe { decoded.copy_to_buffer(buffer, Box::new(Arc::clone(&texture.nv12))) }
            .map_err(|error| error.to_string())?;

        let mut encoder = renderer
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen NV12 to RGBA"),
            });
        {
            let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
                label: Some("lumen NV12 to RGBA"),
                timestamp_writes: None,
            });
            pass.set_pipeline(&self.pipeline);
            pass.set_bind_group(0, &texture.bind_group, &[]);
            pass.dispatch_workgroups(width.div_ceil(8), height.div_ceil(8), 1);
        }
        renderer.queue.submit([encoder.finish()]);
        Ok(Arc::clone(&texture.rgba))
    }

    fn create_texture(
        &self,
        renderer: &lumen_gpu::Renderer,
        size: lumen_gpu::Size,
    ) -> Result<VulkanMediaTexture, String> {
        use lumen_gpu::wgpu;

        let device = &renderer.device;
        let limits = device.limits();
        let nv12_size = lumen_ffmpeg::VulkanVideoFrame::nv12_size(size.width, size.height);
        if size.width > limits.max_texture_dimension_2d
            || size.height > limits.max_texture_dimension_2d
            || nv12_size > limits.max_storage_buffer_binding_size
        {
            return Err(format!(
                "{}x{} frames are too large for this device",
                size.width, size.height
            ));
        }
        let nv12 = Arc::new(device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen decoded NV12"),
            size: nv12_size,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        }));
        // wgpu would clear a buffer it believes is uninitialized when the pass first reads it, which
        // is after raw commands have written it.
        let mut clear = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("lumen clear decoded NV12"),
        });
        clear.clear_buffer(&nv12, 0, None);
        renderer.queue.submit([clear.finish()]);

        let rgba = Arc::new(device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lumen Vulkan decoded media texture"),
            size: size.as_extent(),
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::STORAGE_BINDING
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        }));
        let params = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen NV12 to RGBA params"),
            size: PARAMS_BYTES,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let view = rgba.create_view(&wgpu::TextureViewDescriptor::default());
        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("lumen NV12 to RGBA"),
            layout: &self.pipeline.get_bind_group_layout(0),
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: nv12.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&view),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: params.as_entire_binding(),
                },
            ],
        });
        Ok(VulkanMediaTexture {
            size,
            nv12,
            rgba,
            params,
            written_params: None,
            bind_group,
        })
    }
}

/// Four `u32`s and six `f32`s, rounded up to the uniform struct's 16-byte alignment.
#[cfg(all(target_os = "linux", feature = "vulkan"))]
const PARAMS_BYTES: u64 = 48;
