use std::ffi::{CStr, c_void};

use anyhow::{Context, Result, bail};
use ash::vk;

use crate::{GpuAdapterInfo, Renderer};

// VK_KHR_video_encode_av1 is newer than ash's Vulkan headers.
const AV1_ENCODE_EXTENSION: &CStr = c"VK_KHR_video_encode_av1";
const AV1_ENCODE_FEATURES: vk::StructureType = vk::StructureType::from_raw(1_000_513_004);

#[repr(C)]
struct VideoEncodeAv1Features {
    s_type: vk::StructureType,
    p_next: *mut c_void,
    video_encode_av1: vk::Bool32,
}

/// Raw handles of a wgpu device that can also encode with Vulkan Video.
pub struct VulkanVideoDevice {
    pub get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,
    pub device_extensions: Vec<&'static CStr>,
    /// `(family index, queue count, capabilities)` of the queues video work may use.
    pub queue_families: Vec<(u32, u32, vk::QueueFlags)>,
    /// The queue wgpu submits to.
    pub queue: vk::Queue,
    pub queue_family: u32,
}

impl Renderer {
    /// Creates a Vulkan renderer whose device also has Vulkan Video encode queues, extensions and
    /// features, so an encoder can share it.
    pub async fn new_vulkan_video() -> Result<(Self, VulkanVideoDevice)> {
        let mut instance_desc = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
        instance_desc.backends = wgpu::Backends::VULKAN;
        let instance = wgpu::Instance::new(instance_desc);
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::from_env()
                    .unwrap_or(wgpu::PowerPreference::HighPerformance),
                compatible_surface: None,
                force_fallback_adapter: false,
                apply_limit_buckets: false,
            })
            .await
            .context("no Vulkan wgpu adapter")?;
        let adapter_info = GpuAdapterInfo::from(adapter.get_info());

        let (open_device, queue_families) = {
            let hal_adapter = unsafe { adapter.as_hal::<wgpu_hal::api::Vulkan>() }
                .context("wgpu adapter is not using the Vulkan backend")?;
            open_video_device(&hal_adapter)?
        };
        let (device, queue) = unsafe {
            adapter.create_device_from_hal(open_device, &wgpu::DeviceDescriptor::default())
        }
        .context("create wgpu device")?;

        let video = {
            let hal_device = unsafe { device.as_hal::<wgpu_hal::api::Vulkan>() }
                .context("wgpu device is not using the Vulkan backend")?;
            let shared = hal_device.shared_instance();
            VulkanVideoDevice {
                get_instance_proc_addr: shared.entry().static_fn().get_instance_proc_addr,
                instance: shared.raw_instance().handle(),
                physical_device: hal_device.raw_physical_device(),
                device: hal_device.raw_device().handle(),
                device_extensions: hal_device.enabled_device_extensions().to_vec(),
                queue_families,
                queue: hal_device.raw_queue(),
                queue_family: hal_device.queue_family_index(),
            }
        };
        Ok((
            Self::from_device_with_adapter_info(device, queue, adapter_info),
            video,
        ))
    }
}

pub fn raw_vulkan_buffer(buffer: &wgpu::Buffer) -> Result<vk::Buffer> {
    let hal_buffer = unsafe { buffer.as_hal::<wgpu_hal::api::Vulkan>() }
        .context("wgpu buffer is not a Vulkan buffer")?;
    Ok(unsafe { hal_buffer.raw_handle() })
}

type VideoQueueFamilies = Vec<(u32, u32, vk::QueueFlags)>;

fn open_video_device(
    adapter: &wgpu_hal::vulkan::Adapter,
) -> Result<(
    wgpu_hal::OpenDevice<wgpu_hal::api::Vulkan>,
    VideoQueueFamilies,
)> {
    let shared_instance = adapter.shared_instance();
    let instance = shared_instance.raw_instance();
    let physical_device = adapter.raw_physical_device();
    let api_version =
        unsafe { instance.get_physical_device_properties(physical_device) }.api_version;
    if shared_instance.instance_api_version() < vk::API_VERSION_1_3
        || api_version < vk::API_VERSION_1_3
    {
        bail!("Vulkan Video encode needs Vulkan 1.3");
    }

    let supported = unsafe { instance.enumerate_device_extension_properties(physical_device) }
        .context("enumerate Vulkan device extensions")?;
    let supports = |name: &CStr| {
        supported
            .iter()
            .any(|extension| extension.extension_name_as_c_str() == Ok(name))
    };
    let required = [
        ash::khr::video_queue::NAME,
        ash::khr::video_encode_queue::NAME,
        ash::khr::video_maintenance1::NAME,
    ];
    if let Some(missing) = required.iter().find(|name| !supports(name)) {
        bail!(
            "Vulkan device does not support {}",
            missing.to_string_lossy()
        );
    }
    let extensions: Vec<&'static CStr> = required
        .into_iter()
        .chain(
            [
                ash::khr::video_encode_h264::NAME,
                ash::khr::video_encode_h265::NAME,
                AV1_ENCODE_EXTENSION,
            ]
            .into_iter()
            .filter(|name| supports(name)),
        )
        .collect();
    let av1 = extensions.contains(&AV1_ENCODE_EXTENSION);

    let mut synchronization2 = vk::PhysicalDeviceSynchronization2Features::default();
    let mut video_maintenance1 = vk::PhysicalDeviceVideoMaintenance1FeaturesKHR::default();
    let mut av1_encode = VideoEncodeAv1Features {
        s_type: AV1_ENCODE_FEATURES,
        p_next: std::ptr::null_mut(),
        video_encode_av1: vk::FALSE,
    };
    {
        let mut features = vk::PhysicalDeviceFeatures2::default()
            .push_next(&mut synchronization2)
            .push_next(&mut video_maintenance1);
        if av1 {
            av1_encode.p_next = features.p_next;
            features.p_next = (&raw mut av1_encode).cast();
        }
        unsafe { instance.get_physical_device_features2(physical_device, &mut features) };
    }
    if synchronization2.synchronization2 == vk::FALSE
        || video_maintenance1.video_maintenance1 == vk::FALSE
    {
        bail!("Vulkan device lacks the synchronization2 or videoMaintenance1 feature");
    }
    synchronization2.p_next = std::ptr::null_mut();
    video_maintenance1.p_next = std::ptr::null_mut();
    av1_encode.p_next = std::ptr::null_mut();

    let families = unsafe { instance.get_physical_device_queue_family_properties(physical_device) };
    let find_family = |wanted: vk::QueueFlags, unwanted: vk::QueueFlags| {
        families.iter().enumerate().find_map(|(index, family)| {
            (family.queue_flags.contains(wanted) && !family.queue_flags.intersects(unwanted))
                .then_some((index as u32, family.queue_count, wanted))
        })
    };
    let encode = find_family(vk::QueueFlags::VIDEO_ENCODE_KHR, vk::QueueFlags::empty()).context(
        "Vulkan device has no video encode queue (RADV needs RADV_PERFTEST=video_encode)",
    )?;
    // A dedicated compute family keeps FFmpeg's own submissions off wgpu's queue.
    let compute = find_family(
        vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER,
        vk::QueueFlags::GRAPHICS,
    );
    // wgpu-hal always renders on queue 0 of family 0.
    let render = (
        0,
        1,
        families[0].queue_flags
            & (vk::QueueFlags::GRAPHICS | vk::QueueFlags::COMPUTE | vk::QueueFlags::TRANSFER),
    );
    let extra_families: Vec<_> = compute.into_iter().chain([encode]).collect();
    let queue_families = compute.into_iter().chain([render, encode]).collect();
    let priorities = vec![
        1.0_f32;
        families
            .iter()
            .map(|f| f.queue_count as usize)
            .max()
            .unwrap_or(1)
    ];

    let callback: Box<wgpu_hal::vulkan::CreateDeviceCallback<'_>> = Box::new(|args| {
        for extension in extensions {
            if !args.extensions.contains(&extension) {
                args.extensions.push(extension);
            }
        }
        for &(index, count, _) in &extra_families {
            args.queue_create_infos.push(
                vk::DeviceQueueCreateInfo::default()
                    .queue_family_index(index)
                    .queue_priorities(&priorities[..count as usize]),
            );
        }
        synchronization2.synchronization2 = vk::TRUE;
        video_maintenance1.video_maintenance1 = vk::TRUE;
        *args.create_info = std::mem::take(args.create_info)
            .push_next(&mut synchronization2)
            .push_next(&mut video_maintenance1);
        if av1 && av1_encode.video_encode_av1 == vk::TRUE {
            av1_encode.p_next = args.create_info.p_next.cast_mut();
            args.create_info.p_next = (&raw const av1_encode).cast();
        }
    });
    let open_device = unsafe {
        adapter.open_with_callback(
            wgpu::Features::empty(),
            &wgpu::Limits::default(),
            &wgpu::MemoryHints::default(),
            Some(callback),
        )
    }
    .context("create Vulkan device with video encode queues")?;
    Ok((open_device, queue_families))
}
