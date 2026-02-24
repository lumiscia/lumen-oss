use skia_safe::Surface;

use super::create_gpu_surface;

pub(crate) struct VulkanSurfaceFactory {
    state: VulkanStateSlot,
}

enum VulkanStateSlot {
    Uninitialized,
    Unavailable,
    Ready(VulkanState),
}

impl VulkanSurfaceFactory {
    pub(crate) fn new() -> Self {
        Self {
            state: VulkanStateSlot::Uninitialized,
        }
    }

    pub(crate) fn create_surface(&mut self, width: u32, height: u32) -> Option<Surface> {
        let state = self.ensure_state()?;
        create_gpu_surface(&mut state.context, width, height)
    }

    fn ensure_state(&mut self) -> Option<&mut VulkanState> {
        if matches!(self.state, VulkanStateSlot::Uninitialized) {
            self.state = VulkanState::try_create()
                .map(VulkanStateSlot::Ready)
                .unwrap_or(VulkanStateSlot::Unavailable)
        }

        match &mut self.state {
            VulkanStateSlot::Ready(state) => Some(state),
            VulkanStateSlot::Uninitialized | VulkanStateSlot::Unavailable => None,
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
struct VulkanState {
    context: skia_safe::gpu::DirectContext,
    _entry: ash::Entry,
    _instance: ash::Instance,
    _device: ash::Device,
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for VulkanState {
    fn drop(&mut self) {
        unsafe {
            self._device.destroy_device(None);
            self._instance.destroy_instance(None);
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl VulkanState {
    fn try_create() -> Option<Self> {
        use ash::vk;
        use ash::vk::Handle;
        use skia_safe::gpu;

        let app_name = cstr_lumen();
        let entry = unsafe { ash::Entry::load().ok()? };

        let app_info = vk::ApplicationInfo::default()
            .application_name(app_name)
            .application_version(vk::make_api_version(0, 0, 1, 0))
            .engine_name(app_name)
            .engine_version(vk::make_api_version(0, 0, 1, 0))
            .api_version(vk::make_api_version(0, 1, 1, 0));

        let create_info = vk::InstanceCreateInfo::default().application_info(&app_info);
        let instance = unsafe { entry.create_instance(&create_info, None).ok()? };

        let physical_devices = unsafe { instance.enumerate_physical_devices().ok()? };
        let (physical_device, queue_family_index) = physical_devices.iter().find_map(|&pd| {
            let queue_families =
                unsafe { instance.get_physical_device_queue_family_properties(pd) };
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
        let device = unsafe {
            instance
                .create_device(physical_device, &device_create_info, None)
                .ok()?
        };
        let queue = unsafe { device.get_device_queue(queue_family_index, 0) };

        let context = {
            let get_proc = |of: gpu::vk::GetProcOf| -> *const std::ffi::c_void {
                unsafe {
                    let proc = match of {
                        gpu::vk::GetProcOf::Instance(inst, name) => {
                            entry.get_instance_proc_addr(vk::Instance::from_raw(inst as u64), name)
                        }
                        gpu::vk::GetProcOf::Device(dev, name) => {
                            instance.get_device_proc_addr(vk::Device::from_raw(dev as u64), name)
                        }
                    };
                    proc.map_or(std::ptr::null(), |f| f as *const std::ffi::c_void)
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

        Some(Self {
            context,
            _entry: entry,
            _instance: instance,
            _device: device,
        })
    }
}

#[cfg(target_arch = "wasm32")]
struct VulkanState {
    context: skia_safe::gpu::DirectContext,
}

#[cfg(target_arch = "wasm32")]
impl VulkanState {
    fn try_create() -> Option<Self> {
        None
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn cstr_lumen() -> &'static std::ffi::CStr {
    unsafe { std::ffi::CStr::from_bytes_with_nul_unchecked(b"lumen\0") }
}
