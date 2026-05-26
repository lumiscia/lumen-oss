use std::sync::Arc;

#[cfg(any(
    all(target_os = "linux", feature = "cuda", feature = "vulkan"),
    all(target_os = "macos", feature = "metal")
))]
use std::collections::HashMap;
#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
use std::sync::OnceLock;

use super::types::MediaTextureUpload;

#[derive(Default)]
pub(super) struct GpuMediaFrameImporter {
    #[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
    cuda: Option<CudaMediaImporter>,
    #[cfg(all(target_os = "macos", feature = "metal"))]
    metal: Option<MetalMediaImporter>,
}

impl GpuMediaFrameImporter {
    pub(super) fn import(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        upload: &MediaTextureUpload,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
        #[cfg(not(any(
            all(target_os = "linux", feature = "cuda", feature = "vulkan"),
            all(target_os = "macos", feature = "metal")
        )))]
        let _ = (renderer, upload);

        match frame.frame.backend() {
            #[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
            lumen_ffmpeg::GpuBackend::Cuda => self.import_cuda(renderer, upload, frame),
            #[cfg(all(target_os = "macos", feature = "metal"))]
            lumen_ffmpeg::GpuBackend::Metal => self.import_metal(renderer, upload, frame),
            backend => Err(format!(
                "GPU media backend {backend:?} is not supported by this renderer build"
            )),
        }
    }

    #[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
    fn import_cuda(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        upload: &MediaTextureUpload,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
        let (width, height) = frame.dimensions();
        let decoded_size = lumen_gpu::Size::new(width.max(1), height.max(1));
        if self.cuda.is_none() {
            let exportable = renderer
                .create_exportable_vulkan_texture(
                    Some("lumen cuda media texture bootstrap"),
                    decoded_size,
                    lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
                    lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
                        | lumen_gpu::wgpu::TextureUsages::COPY_DST
                        | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
                )
                .map_err(|error| error.to_string())?;
            self.cuda = Some(CudaMediaImporter::new(&exportable)?);
        }
        self.cuda
            .as_mut()
            .ok_or_else(|| "CUDA media importer was not initialized".to_string())?
            .import_frame(renderer, upload, frame)
    }

    #[cfg(all(target_os = "macos", feature = "metal"))]
    fn import_metal(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        upload: &MediaTextureUpload,
        frame: &Arc<crate::media::GpuVideoMediaFrame>,
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
        if self.metal.is_none() {
            self.metal = Some(MetalMediaImporter::new(renderer)?);
        }
        self.metal
            .as_mut()
            .ok_or_else(|| "Metal media importer was not initialized".to_string())?
            .import_frame(renderer, upload, frame)
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
struct MetalMediaImporter {
    texture_cache: lumen_ffmpeg::MetalTextureCache,
    textures: HashMap<lumen_gpu::TextureId, MetalMediaTexture>,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
struct MetalMediaTexture {
    _size: lumen_gpu::Size,
    _texture: Arc<lumen_gpu::wgpu::Texture>,
    _frame: Arc<crate::media::GpuVideoMediaFrame>,
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl MetalMediaImporter {
    fn new(renderer: &lumen_gpu::Renderer) -> Result<Self, String> {
        let device = lumen_gpu::metal_device_from_wgpu(&renderer.device)
            .map_err(|error| error.to_string())?;
        let texture_cache =
            lumen_ffmpeg::MetalTextureCache::create(&device).map_err(|error| error.to_string())?;
        Ok(Self {
            texture_cache,
            textures: HashMap::new(),
        })
    }

    fn import_frame(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        upload: &MediaTextureUpload,
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
        let texture = Arc::new(lumen_gpu::texture_from_metal(
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
        ));
        self.textures.insert(
            upload.texture,
            MetalMediaTexture {
                _size: size,
                _texture: Arc::clone(&texture),
                _frame: Arc::clone(frame),
            },
        );
        Ok(texture)
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
impl Drop for MetalMediaImporter {
    fn drop(&mut self) {
        self.textures.clear();
        self.texture_cache.flush();
    }
}

#[cfg(all(target_os = "macos", feature = "metal"))]
fn media_texture_usage() -> lumen_gpu::wgpu::TextureUsages {
    lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
        | lumen_gpu::wgpu::TextureUsages::COPY_DST
        | lumen_gpu::wgpu::TextureUsages::COPY_SRC
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
struct CudaMediaImporter {
    driver: &'static lumen_ffmpeg::CudaDriver,
    context: lumen_ffmpeg::CudaContext<'static>,
    converter: lumen_ffmpeg::CudaNv12ToRgbaConverter<'static>,
    textures: HashMap<lumen_gpu::TextureId, CudaMediaTexture>,
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
struct CudaMediaTexture {
    size: lumen_gpu::Size,
    imported: lumen_ffmpeg::ImportedCudaExternalImage<'static>,
    rgba: lumen_ffmpeg::CudaDeviceAllocation<'static>,
    exportable: lumen_gpu::ExportableVulkanTexture,
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl CudaMediaImporter {
    fn new(exportable: &lumen_gpu::ExportableVulkanTexture) -> Result<Self, String> {
        let driver = cuda_driver()?;
        let vulkan_uuid = exportable.device_info().device_uuid;
        let cuda_ordinal = driver
            .devices()?
            .into_iter()
            .find(|device| device.uuid == vulkan_uuid)
            .map(|device| device.ordinal)
            .unwrap_or(0);
        let context = driver.create_primary_context_for_ordinal(cuda_ordinal)?;
        let converter = driver.create_nv12_to_rgba_converter(&context)?;
        Ok(Self {
            driver,
            context,
            converter,
            textures: HashMap::new(),
        })
    }

    fn import_frame(
        &mut self,
        renderer: &lumen_gpu::Renderer,
        upload: &MediaTextureUpload,
        frame: &crate::media::GpuVideoMediaFrame,
    ) -> Result<Arc<lumen_gpu::wgpu::Texture>, String> {
        let lumen_ffmpeg::GpuVideoFrame::Cuda(decoded) = &frame.frame else {
            return Err(format!(
                "unsupported GPU media backend {:?}; only CUDA media frames can be imported here",
                frame.frame.backend()
            ));
        };
        let (decoded_width, decoded_height) = decoded.dimensions();
        let decoded_size = lumen_gpu::Size::new(decoded_width.max(1), decoded_height.max(1));

        let needs_texture = self
            .textures
            .get(&upload.texture)
            .is_none_or(|texture| texture.size != decoded_size);
        if needs_texture {
            let texture = create_cuda_media_texture(renderer, self.driver, decoded_size)?;
            self.textures.insert(upload.texture, texture);
        }
        let texture = self
            .textures
            .get_mut(&upload.texture)
            .ok_or_else(|| "CUDA media texture cache entry was not initialized".to_string())?;

        self.context.set_current()?;
        self.converter.convert(decoded, &texture.rgba)?;
        self.driver
            .copy_rgba_frame_to_image(&texture.rgba, &texture.imported)?;
        self.driver.synchronize_context()?;
        Ok(texture.exportable.texture_arc())
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
impl Drop for CudaMediaImporter {
    fn drop(&mut self) {
        let _ = self.context.set_current();
        self.textures.clear();
    }
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn cuda_driver() -> Result<&'static lumen_ffmpeg::CudaDriver, String> {
    static CUDA_DRIVER: OnceLock<lumen_ffmpeg::CudaDriver> = OnceLock::new();
    if let Some(driver) = CUDA_DRIVER.get() {
        return Ok(driver);
    }
    let _ = CUDA_DRIVER.set(lumen_ffmpeg::CudaDriver::load()?);
    CUDA_DRIVER
        .get()
        .ok_or_else(|| "CUDA driver was not initialized".to_string())
}

#[cfg(all(target_os = "linux", feature = "cuda", feature = "vulkan"))]
fn create_cuda_media_texture(
    renderer: &lumen_gpu::Renderer,
    driver: &'static lumen_ffmpeg::CudaDriver,
    size: lumen_gpu::Size,
) -> Result<CudaMediaTexture, String> {
    use lumen_ffmpeg::import_owned_vulkan_opaque_fd_image;
    let exportable = renderer
        .create_exportable_vulkan_texture(
            Some("lumen cuda media texture"),
            size,
            lumen_gpu::wgpu::TextureFormat::Rgba8Unorm,
            lumen_gpu::wgpu::TextureUsages::TEXTURE_BINDING
                | lumen_gpu::wgpu::TextureUsages::COPY_DST
                | lumen_gpu::wgpu::TextureUsages::COPY_SRC,
        )
        .map_err(|error| error.to_string())?;

    let rgba = driver.allocate_rgba_frame(size.width, size.height)?;
    let imported = import_owned_vulkan_opaque_fd_image(
        driver,
        exportable
            .memory_fd()
            .try_clone()
            .map_err(|error| format!("failed to duplicate Vulkan memory fd: {error}"))?,
        exportable.allocation_size(),
        size.width,
        size.height,
    )?;
    Ok(CudaMediaTexture {
        size,
        imported,
        rgba,
        exportable,
    })
}
