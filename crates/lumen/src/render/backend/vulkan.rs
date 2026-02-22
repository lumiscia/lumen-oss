use crate::render::backend::{FrameProvider, RenderBackend, RenderError};
use crate::render::context::{FrameContext, RendererContext};

use ash::vk;
use ash::vk::Handle;
use skia_safe::gpu;

use super::{GpuBackend, GpuState, create_gpu_surface};

pub struct VulkanRenderBackend {
    gpu: Option<super::GpuState>,
}

impl Default for VulkanRenderBackend {
    fn default() -> Self {
        Self { gpu: None }
    }
}

impl RenderBackend for VulkanRenderBackend {
    fn render_frame(
        &mut self,
        renderer_ctx: &mut RendererContext,
        _frame_ctx: &FrameContext,
        _provider: &mut dyn FrameProvider,
    ) -> Result<Vec<u8>, RenderError> {
        self.ensure_gpu_surface(renderer_ctx)?;
        renderer_ctx.clear();
        super::read_surface_rgba(renderer_ctx)
    }
}

#[cfg(feature = "gpu-vulkan")]
impl VulkanRenderBackend {
    fn ensure_gpu_surface(
        &mut self,
        renderer_ctx: &mut RendererContext,
    ) -> Result<(), RenderError> {
        if let Some(gpu) = self.gpu.as_mut() {
            renderer_ctx.surface = super::create_gpu_surface(
                &mut gpu.context,
                renderer_ctx.width,
                renderer_ctx.height,
            )?;
            return Ok(());
        }

        let (surface, gpu) = try_create(renderer_ctx.width, renderer_ctx.height)
            .ok_or(RenderError::NotInitialized)?;
        renderer_ctx.surface = surface;
        self.gpu = Some(gpu);
        Ok(())
    }
}

pub(in crate::render) struct VulkanState {
    _entry: ash::Entry,
    _instance: ash::Instance,
    _device: ash::Device,
}

impl Drop for VulkanState {
    fn drop(&mut self) {
        unsafe {
            self._device.destroy_device(None);
            self._instance.destroy_instance(None);
        }
    }
}

pub(super) fn try_create(width: u32, height: u32) -> Option<(skia_safe::Surface, GpuState)> {
    let entry = unsafe { ash::Entry::load() }.ok()?;

    let app_info = vk::ApplicationInfo::default()
        .application_name(c"lumen")
        .application_version(vk::make_api_version(0, 0, 1, 0))
        .engine_name(c"lumen")
        .engine_version(vk::make_api_version(0, 0, 1, 0))
        .api_version(vk::make_api_version(0, 1, 1, 0));

    let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);

    let instance = unsafe { entry.create_instance(&create_info, None) }.ok()?;

    let physical_devices = unsafe { instance.enumerate_physical_devices() }.ok()?;
    let (physical_device, queue_family_index) = physical_devices.iter().find_map(|&pd| {
        let queue_families = unsafe { instance.get_physical_device_queue_family_properties(pd) };
        queue_families.iter().enumerate().find_map(|(i, qf)| {
            if qf.queue_flags.contains(vk::QueueFlags::GRAPHICS) {
                Some((pd, i as u32))
            } else {
                None
            }
        })
    })?;

    let priorities = [1.0f32];
    let queue_create_info = vk::DeviceQueueCreateInfo::default()
        .queue_family_index(queue_family_index)
        .queue_priorities(&priorities);

    let device_create_info = vk::DeviceCreateInfo::default()
        .queue_create_infos(std::slice::from_ref(&queue_create_info));

    let device =
        unsafe { instance.create_device(physical_device, &device_create_info, None) }.ok()?;
    let queue = unsafe { device.get_device_queue(queue_family_index, 0) };

    let mut context = {
        let get_proc = |of: gpu::vk::GetProcOf| -> gpu::vk::GetProcResult {
            unsafe {
                match of {
                    gpu::vk::GetProcOf::Instance(inst, name) => entry
                        .get_instance_proc_addr(vk::Instance::from_raw(inst as u64), name)
                        .map(|f| f as gpu::vk::GetProcResult)
                        .unwrap_or(std::ptr::null()),
                    gpu::vk::GetProcOf::Device(dev, name) => instance
                        .get_device_proc_addr(vk::Device::from_raw(dev as u64), name)
                        .map(|f| f as gpu::vk::GetProcResult)
                        .unwrap_or(std::ptr::null()),
                }
            }
        };

        let backend = unsafe {
            gpu::vk::BackendContext::new(
                instance.handle().as_raw() as _,
                physical_device.as_raw() as _,
                device.handle().as_raw() as _,
                (queue.as_raw() as _, queue_family_index as usize),
                &get_proc,
            )
        };

        gpu::direct_contexts::make_vulkan(&backend, None)?
    };

    let surface = create_gpu_surface(&mut context, width, height).ok()?;

    Some((
        surface,
        GpuState {
            context,
            _backend: GpuBackend::Vulkan(VulkanState {
                _entry: entry,
                _instance: instance,
                _device: device,
            }),
        },
    ))
}
