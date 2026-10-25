use std::{ffi::CStr, sync::Arc};

use ash::vk::{self, Handle};

use crate::encode::hw::AvBufferRef;
use crate::ffi::{self, sys};
use crate::gpu::GpuBackend;
use crate::{FfmpegError, Result};

pub const BACKEND: GpuBackend = GpuBackend::Vulkan;

/// An NV12 frame in a Vulkan buffer on a [`VulkanDevice`]: [`nv12_pitch`](Self::nv12_pitch)-byte
/// luma rows, followed at [`nv12_chroma_offset`](Self::nv12_chroma_offset) by interleaved CbCr rows
/// of the same pitch.
#[derive(Clone)]
pub struct VulkanVideoFrame {
    device: vk::Device,
    buffer: vk::Buffer,
    width: u32,
    height: u32,
    pts: Option<i64>,
    // Keeps the buffer alive until the GPU copy that reads it has finished.
    _retention: Arc<dyn Send + Sync>,
}

impl std::fmt::Debug for VulkanVideoFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VulkanVideoFrame")
            .field("device", &self.device)
            .field("buffer", &self.buffer)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pts", &self.pts)
            .finish_non_exhaustive()
    }
}

impl VulkanVideoFrame {
    /// # Safety
    ///
    /// `buffer` must belong to `device`, have `TRANSFER_SRC` usage and hold at least
    /// [`nv12_size`](Self::nv12_size) bytes written by work already submitted to the device's render
    /// queue. `retention` must keep `device` and `buffer` alive.
    pub unsafe fn new(
        device: vk::Device,
        buffer: vk::Buffer,
        width: u32,
        height: u32,
        pts: Option<i64>,
        retention: Arc<dyn Send + Sync>,
    ) -> Self {
        Self {
            device,
            buffer,
            width,
            height,
            pts,
            _retention: retention,
        }
    }

    pub fn nv12_pitch(width: u32) -> u32 {
        width.next_multiple_of(4)
    }

    pub fn nv12_chroma_offset(width: u32, height: u32) -> u64 {
        u64::from(Self::nv12_pitch(width)) * u64::from(height)
    }

    pub fn nv12_size(width: u32, height: u32) -> u64 {
        u64::from(Self::nv12_pitch(width)) * u64::from(height + height.div_ceil(2))
    }

    pub fn device(&self) -> vk::Device {
        self.device
    }

    pub fn buffer(&self) -> vk::Buffer {
        self.buffer
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pts(&self) -> Option<i64> {
        self.pts
    }

    pub fn backend(&self) -> GpuBackend {
        GpuBackend::Vulkan
    }
}

/// Raw handles of a Vulkan 1.3 device created with `synchronization2`, timeline semaphores, Vulkan
/// Video encode extensions and an encode queue.
pub struct VulkanDeviceDesc<'a> {
    pub get_instance_proc_addr: vk::PFN_vkGetInstanceProcAddr,
    pub instance: vk::Instance,
    pub physical_device: vk::PhysicalDevice,
    pub device: vk::Device,
    pub device_extensions: &'a [&'static CStr],
    /// `(family index, queue count, capabilities)` for each queue family FFmpeg may use. FFmpeg
    /// submits to queues of every family listed with capabilities.
    pub queue_families: &'a [(u32, u32, vk::QueueFlags)],
}

/// Submits work to the queue frames are rendered on, through the renderer's own submission path so
/// that queue stays externally synchronised.
pub trait VulkanRenderQueue: Send + Sync {
    /// Calls `record` once with a command buffer in the recording state, then submits it after the
    /// work already submitted to the render queue. The submission must wait for and signal the given
    /// timeline semaphore values, and `retain` must be dropped only once it has finished executing.
    fn submit(
        &self,
        record: &mut dyn FnMut(vk::CommandBuffer),
        waits: &[(vk::Semaphore, u64)],
        signals: &[(vk::Semaphore, u64)],
        retain: Box<dyn Send>,
    ) -> std::result::Result<(), String>;
}

/// A Vulkan device shared between the caller's renderer and FFmpeg's Vulkan Video encoders.
#[derive(Clone)]
pub struct VulkanDevice(Arc<SharedDevice>);

struct SharedDevice {
    hw_device: AvBufferRef,
    device: ash::Device,
    render_queue: Arc<dyn VulkanRenderQueue>,
}

// FFmpeg device references are refcounted atomically and the Vulkan handles are plain values.
unsafe impl Send for SharedDevice {}
unsafe impl Sync for SharedDevice {}

impl std::fmt::Debug for VulkanDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VulkanDevice")
            .field("device", &self.handle())
            .finish_non_exhaustive()
    }
}

struct DeviceOwner {
    _extensions: Box<[*const libc::c_char]>,
    _render_queue: Arc<dyn VulkanRenderQueue>,
}

impl VulkanDevice {
    /// Wraps an existing device in an FFmpeg Vulkan device context. FFmpeg does not destroy the
    /// device; it holds `render_queue` until it releases the device.
    ///
    /// # Safety
    ///
    /// The handles in `desc` must be valid and describe the device as created, and `render_queue`
    /// must keep them alive. No listed queue family with capabilities may contain a queue that is
    /// used outside FFmpeg, and `render_queue` must submit to a queue of a listed family as its
    /// documentation requires.
    pub unsafe fn from_raw(
        desc: &VulkanDeviceDesc<'_>,
        render_queue: Arc<dyn VulkanRenderQueue>,
    ) -> Result<Self> {
        ffi::init();
        let ptr =
            unsafe { sys::av_hwdevice_ctx_alloc(sys::AVHWDeviceType::AV_HWDEVICE_TYPE_VULKAN) };
        if ptr.is_null() {
            return Err(FfmpegError::new(
                "av_hwdevice_ctx_alloc",
                "failed to allocate a Vulkan device context",
            )
            .with_backend(BACKEND));
        }
        let hw_device = AvBufferRef { ptr };
        let extensions: Box<[*const libc::c_char]> = desc
            .device_extensions
            .iter()
            .map(|name| name.as_ptr())
            .collect();
        unsafe {
            let context = (*ptr).data.cast::<sys::AVHWDeviceContext>();
            let hwctx = (*context).hwctx.cast::<sys::AVVulkanDeviceContext>();
            (*hwctx).get_proc_addr = std::mem::transmute::<
                vk::PFN_vkGetInstanceProcAddr,
                sys::PFN_vkGetInstanceProcAddr,
            >(desc.get_instance_proc_addr);
            (*hwctx).inst = desc.instance.as_raw() as usize as _;
            (*hwctx).phys_dev = desc.physical_device.as_raw() as usize as _;
            (*hwctx).act_dev = desc.device.as_raw() as usize as _;
            (*hwctx).enabled_dev_extensions = extensions.as_ptr();
            (*hwctx).nb_enabled_dev_extensions = extensions.len() as i32;
            if desc.queue_families.len() > (*hwctx).qf.len() {
                return Err(FfmpegError::new(
                    "VulkanDevice::from_raw",
                    "too many queue families for FFmpeg",
                )
                .with_backend(BACKEND));
            }
            for (slot, &(index, count, flags)) in desc.queue_families.iter().enumerate() {
                (*hwctx).qf[slot] = sys::AVVulkanDeviceQueueFamily {
                    idx: index as i32,
                    num: count as i32,
                    flags: flags.as_raw(),
                    video_caps: 0,
                };
            }
            (*hwctx).nb_qf = desc.queue_families.len() as i32;
            (*context).user_opaque = Box::into_raw(Box::new(DeviceOwner {
                _extensions: extensions,
                _render_queue: render_queue.clone(),
            }))
            .cast();
            (*context).free = Some(free_device_owner);
            ffi::check(sys::av_hwdevice_ctx_init(ptr), "av_hwdevice_ctx_init")
                .map_err(|error| error.with_backend(BACKEND))?;
        }

        let static_fn = ash::StaticFn {
            get_instance_proc_addr: desc.get_instance_proc_addr,
        };
        let instance = unsafe { ash::Instance::load(&static_fn, desc.instance) };
        let device = unsafe { ash::Device::load(instance.fp_v1_0(), desc.device) };
        Ok(Self(Arc::new(SharedDevice {
            hw_device,
            device,
            render_queue,
        })))
    }

    pub(crate) fn hw_device(&self) -> Result<AvBufferRef> {
        self.0.hw_device.clone_ref().ok_or_else(|| {
            FfmpegError::new("av_buffer_ref", "failed to reference the Vulkan device")
                .with_backend(BACKEND)
        })
    }

    pub fn handle(&self) -> vk::Device {
        self.0.device.handle()
    }

    /// Copies `frame` into `target` on the render queue, after the work that wrote `frame`.
    ///
    /// # Safety
    ///
    /// `frame` must be on this device and `target` must be a locked, single-image NV12 frame of the
    /// same size from a frames context on this device.
    pub(crate) unsafe fn upload(
        &self,
        frame: &VulkanVideoFrame,
        target: *mut sys::AVVkFrame,
    ) -> Result<()> {
        let raw = &self.0.device;
        let (image, semaphore, wait_value, old_layout, old_access) = unsafe {
            if (*target).img[1] != 0 {
                return Err(FfmpegError::new(
                    "VulkanDevice::upload",
                    "expected encoder frames to be a single multi-planar NV12 image",
                )
                .with_backend(BACKEND));
            }
            (
                vk::Image::from_raw((*target).img[0]),
                vk::Semaphore::from_raw((*target).sem[0]),
                (*target).sem_value[0],
                vk::ImageLayout::from_raw((*target).layout[0]),
                vk::AccessFlags2::from_raw((*target).access[0] as u64),
            )
        };

        let (width, height) = frame.dimensions();
        let pitch = VulkanVideoFrame::nv12_pitch(width);
        let plane = |aspect, offset, row_length, width, height| vk::BufferImageCopy {
            buffer_offset: offset,
            buffer_row_length: row_length,
            buffer_image_height: 0,
            image_subresource: vk::ImageSubresourceLayers {
                aspect_mask: aspect,
                mip_level: 0,
                base_array_layer: 0,
                layer_count: 1,
            },
            image_offset: vk::Offset3D::default(),
            image_extent: vk::Extent3D {
                width,
                height,
                depth: 1,
            },
        };
        let regions = [
            plane(vk::ImageAspectFlags::PLANE_0, 0, pitch, width, height),
            plane(
                vk::ImageAspectFlags::PLANE_1,
                VulkanVideoFrame::nv12_chroma_offset(width, height),
                pitch / 2,
                width / 2,
                height / 2,
            ),
        ];
        // The buffer was written by earlier submissions on the render queue.
        let buffer_written = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)];
        let to_transfer = [vk::ImageMemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
            .src_access_mask(old_access)
            .dst_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
            .old_layout(old_layout)
            .new_layout(vk::ImageLayout::TRANSFER_DST_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: 1,
            })];
        // Later submissions on the render queue rewrite the buffer.
        let buffer_read = [vk::MemoryBarrier2::default()
            .src_stage_mask(vk::PipelineStageFlags2::COPY)
            .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
        let mut record = |command_buffer| unsafe {
            raw.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default()
                    .memory_barriers(&buffer_written)
                    .image_memory_barriers(&to_transfer),
            );
            raw.cmd_copy_buffer_to_image(
                command_buffer,
                frame.buffer(),
                image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &regions,
            );
            raw.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().memory_barriers(&buffer_read),
            );
        };
        self.0
            .render_queue
            .submit(
                &mut record,
                &[(semaphore, wait_value)],
                &[(semaphore, wait_value + 1)],
                Box::new(frame.clone()),
            )
            .map_err(|details| {
                FfmpegError::new("VulkanRenderQueue::submit", details).with_backend(BACKEND)
            })?;

        unsafe {
            (*target).layout[0] = vk::ImageLayout::TRANSFER_DST_OPTIMAL.as_raw() as _;
            (*target).access[0] = vk::AccessFlags::TRANSFER_WRITE.as_raw() as _;
            (*target).queue_family[0] = vk::QUEUE_FAMILY_IGNORED;
            (*target).sem_value[0] = wait_value + 1;
        }
        Ok(())
    }
}

unsafe extern "C" fn free_device_owner(context: *mut sys::AVHWDeviceContext) {
    let owner = unsafe { (*context).user_opaque.cast::<DeviceOwner>() };
    if !owner.is_null() {
        drop(unsafe { Box::from_raw(owner) });
    }
}
