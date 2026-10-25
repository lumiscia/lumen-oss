use std::{ffi::CStr, sync::Arc};

use ash::vk::{self, Handle};

use crate::encode::hw::AvBufferRef;
use crate::ffi::{self, sys};
use crate::gpu::GpuBackend;
use crate::{FfmpegError, Result};

pub const BACKEND: GpuBackend = GpuBackend::Vulkan;

const UPLOADS_IN_FLIGHT: usize = 4;

/// An NV12 frame in a Vulkan buffer on a [`VulkanDevice`]: [`nv12_pitch`](Self::nv12_pitch)-byte
/// luma rows, followed at [`nv12_chroma_offset`](Self::nv12_chroma_offset) by interleaved CbCr rows
/// of the same pitch.
#[derive(Clone)]
pub struct VulkanVideoFrame {
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
    /// `buffer` must belong to the encoder's [`VulkanDevice`], have `TRANSFER_SRC` usage and hold at
    /// least [`nv12_size`](Self::nv12_size) bytes written by work already submitted to the device's
    /// render queue. It must stay valid while `retention` is alive.
    pub unsafe fn new(
        buffer: vk::Buffer,
        width: u32,
        height: u32,
        pts: Option<i64>,
        retention: Arc<dyn Send + Sync>,
    ) -> Self {
        Self {
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
    /// `(family index, queue count, capabilities)` for each queue family FFmpeg may use.
    pub queue_families: &'a [(u32, u32, vk::QueueFlags)],
    /// The queue frames are rendered on. Frame copies are submitted here so they run after the render.
    pub queue: vk::Queue,
    pub queue_family: u32,
}

/// A Vulkan device shared between the caller's renderer and FFmpeg's Vulkan Video encoders.
#[derive(Clone)]
pub struct VulkanDevice(Arc<SharedDevice>);

struct SharedDevice {
    hw_device: AvBufferRef,
    device: ash::Device,
    queue: vk::Queue,
    queue_family: u32,
}

// FFmpeg device references are refcounted atomically and the Vulkan handles are plain values.
unsafe impl Send for SharedDevice {}
unsafe impl Sync for SharedDevice {}

impl std::fmt::Debug for VulkanDevice {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("VulkanDevice")
            .field("device", &self.0.device.handle())
            .field("queue_family", &self.0.queue_family)
            .finish_non_exhaustive()
    }
}

struct DeviceOwner {
    _extensions: Box<[*const libc::c_char]>,
    _retention: Arc<dyn Send + Sync>,
}

impl VulkanDevice {
    /// Wraps an existing device in an FFmpeg Vulkan device context. FFmpeg does not destroy the
    /// device; `retention` is dropped once FFmpeg releases it.
    ///
    /// # Safety
    ///
    /// The handles in `desc` must be valid and describe the device as created, and `retention` must
    /// keep them alive. Frames must be sent from the thread that submits work to `desc.queue`.
    pub unsafe fn from_raw(
        desc: &VulkanDeviceDesc<'_>,
        retention: Arc<dyn Send + Sync>,
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
                _retention: retention,
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
            queue: desc.queue,
            queue_family: desc.queue_family,
        })))
    }

    pub(crate) fn hw_device(&self) -> Result<AvBufferRef> {
        self.0.hw_device.clone_ref().ok_or_else(|| {
            FfmpegError::new("av_buffer_ref", "failed to reference the Vulkan device")
                .with_backend(BACKEND)
        })
    }
}

unsafe extern "C" fn free_device_owner(context: *mut sys::AVHWDeviceContext) {
    let owner = unsafe { (*context).user_opaque.cast::<DeviceOwner>() };
    if !owner.is_null() {
        drop(unsafe { Box::from_raw(owner) });
    }
}

/// Copies NV12 buffers into encoder frames on the render queue, synchronised through each frame's
/// timeline semaphore.
pub(crate) struct VulkanFrameUploader {
    device: VulkanDevice,
    pool: vk::CommandPool,
    slots: Vec<UploadSlot>,
    next: usize,
}

struct UploadSlot {
    command_buffer: vk::CommandBuffer,
    fence: vk::Fence,
    _in_flight: Option<VulkanVideoFrame>,
}

impl VulkanFrameUploader {
    pub(crate) fn new(device: VulkanDevice) -> Result<Self> {
        let raw = &device.0.device;
        let pool = unsafe {
            raw.create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER)
                    .queue_family_index(device.0.queue_family),
                None,
            )
        }
        .map_err(vk_error("vkCreateCommandPool"))?;
        let mut uploader = Self {
            device: device.clone(),
            pool,
            slots: Vec::new(),
            next: 0,
        };
        let command_buffers = unsafe {
            raw.allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(pool)
                    .command_buffer_count(UPLOADS_IN_FLIGHT as u32),
            )
        }
        .map_err(vk_error("vkAllocateCommandBuffers"))?;
        for command_buffer in command_buffers {
            let fence = unsafe {
                raw.create_fence(
                    &vk::FenceCreateInfo::default().flags(vk::FenceCreateFlags::SIGNALED),
                    None,
                )
            }
            .map_err(vk_error("vkCreateFence"))?;
            uploader.slots.push(UploadSlot {
                command_buffer,
                fence,
                _in_flight: None,
            });
        }
        Ok(uploader)
    }

    /// # Safety
    ///
    /// `target` must be a locked, single-image NV12 frame from a frames context on this device.
    pub(crate) unsafe fn upload(
        &mut self,
        frame: &VulkanVideoFrame,
        target: *mut sys::AVVkFrame,
    ) -> Result<()> {
        let shared = &self.device.0;
        let raw = &shared.device;
        let slot_count = self.slots.len();
        let slot = &mut self.slots[self.next];
        self.next = (self.next + 1) % slot_count;

        unsafe {
            raw.wait_for_fences(&[slot.fence], true, u64::MAX)
                .map_err(vk_error("vkWaitForFences"))?;
            slot._in_flight = Some(frame.clone());

            if (*target).img[1] != 0 {
                return Err(FfmpegError::new(
                    "VulkanFrameUploader::upload",
                    "expected encoder frames to be a single multi-planar NV12 image",
                )
                .with_backend(BACKEND));
            }
            let image = vk::Image::from_raw((*target).img[0]);
            let semaphore = vk::Semaphore::from_raw((*target).sem[0]);
            let wait_value = (*target).sem_value[0];

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

            let command_buffer = slot.command_buffer;
            raw.begin_command_buffer(
                command_buffer,
                &vk::CommandBufferBeginInfo::default()
                    .flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(vk_error("vkBeginCommandBuffer"))?;
            // The buffer was written by earlier submissions on this queue.
            let buffer_written = [vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::MEMORY_WRITE)
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_READ)];
            let to_transfer = [vk::ImageMemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)
                .src_access_mask(vk::AccessFlags2::from_raw((*target).access[0] as u64))
                .dst_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_access_mask(vk::AccessFlags2::TRANSFER_WRITE)
                .old_layout(vk::ImageLayout::from_raw((*target).layout[0] as i32))
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
            // Later submissions on this queue rewrite the buffer.
            let buffer_read = [vk::MemoryBarrier2::default()
                .src_stage_mask(vk::PipelineStageFlags2::COPY)
                .dst_stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            raw.cmd_pipeline_barrier2(
                command_buffer,
                &vk::DependencyInfo::default().memory_barriers(&buffer_read),
            );
            raw.end_command_buffer(command_buffer)
                .map_err(vk_error("vkEndCommandBuffer"))?;

            let waits = [vk::SemaphoreSubmitInfo::default()
                .semaphore(semaphore)
                .value(wait_value)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let signals = [vk::SemaphoreSubmitInfo::default()
                .semaphore(semaphore)
                .value(wait_value + 1)
                .stage_mask(vk::PipelineStageFlags2::ALL_COMMANDS)];
            let command_buffers =
                [vk::CommandBufferSubmitInfo::default().command_buffer(command_buffer)];
            raw.reset_fences(&[slot.fence])
                .map_err(vk_error("vkResetFences"))?;
            let submitted = raw.queue_submit2(
                shared.queue,
                &[vk::SubmitInfo2::default()
                    .wait_semaphore_infos(&waits)
                    .command_buffer_infos(&command_buffers)
                    .signal_semaphore_infos(&signals)],
                slot.fence,
            );
            if let Err(result) = submitted {
                // Signal the fence anyway so later waits don't block on work that never ran.
                let _ = raw.queue_submit2(shared.queue, &[], slot.fence);
                return Err(vk_error("vkQueueSubmit2")(result));
            }

            (*target).layout[0] = vk::ImageLayout::TRANSFER_DST_OPTIMAL.as_raw() as _;
            (*target).access[0] = vk::AccessFlags::TRANSFER_WRITE.as_raw() as _;
            (*target).queue_family[0] = vk::QUEUE_FAMILY_IGNORED;
            (*target).sem_value[0] = wait_value + 1;
        }
        Ok(())
    }
}

impl Drop for VulkanFrameUploader {
    fn drop(&mut self) {
        let raw = &self.device.0.device;
        let fences: Vec<_> = self.slots.iter().map(|slot| slot.fence).collect();
        unsafe {
            if !fences.is_empty() {
                let _ = raw.wait_for_fences(&fences, true, u64::MAX);
            }
            for fence in fences {
                raw.destroy_fence(fence, None);
            }
            raw.destroy_command_pool(self.pool, None);
        }
    }
}

fn vk_error(operation: &'static str) -> impl Fn(vk::Result) -> FfmpegError {
    move |result| FfmpegError::new(operation, format!("{result:?}")).with_backend(BACKEND)
}
