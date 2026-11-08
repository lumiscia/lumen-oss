use std::{
    ffi::CStr,
    os::fd::{FromRawFd, OwnedFd},
    sync::Arc,
};

use anyhow::{Result, anyhow, bail};
use ash::vk;

use crate::Renderer;

/// A wgpu buffer in Vulkan memory exported as an opaque fd, for another API to import.
pub struct ExportableVulkanBuffer {
    buffer: Arc<wgpu::Buffer>,
    memory_fd: OwnedFd,
    allocation_size: u64,
}

impl ExportableVulkanBuffer {
    pub fn buffer_arc(&self) -> Arc<wgpu::Buffer> {
        Arc::clone(&self.buffer)
    }

    pub fn memory_fd(&self) -> &OwnedFd {
        &self.memory_fd
    }

    pub fn allocation_size(&self) -> u64 {
        self.allocation_size
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VulkanDeviceInfo {
    pub name: String,
    pub vendor_id: u32,
    pub device_id: u32,
    pub device_type: String,
    pub device_uuid: [u8; 16],
    pub driver_uuid: [u8; 16],
}

impl Renderer {
    pub fn vulkan_device_info(&self) -> Result<VulkanDeviceInfo> {
        let Some(hal_device) = (unsafe { self.device.as_hal::<wgpu_hal::api::Vulkan>() }) else {
            bail!("wgpu device is not using the Vulkan backend");
        };
        Ok(vulkan_device_info(
            hal_device.shared_instance().raw_instance(),
            hal_device.raw_physical_device(),
        ))
    }

    /// Creates a `size`-byte device-local buffer whose memory is exported as an opaque fd. Only
    /// `STORAGE`, `COPY_SRC` and `COPY_DST` usages are mapped.
    pub fn create_exportable_vulkan_buffer(
        &self,
        label: Option<&str>,
        size: u64,
        usage: wgpu::BufferUsages,
    ) -> Result<ExportableVulkanBuffer> {
        let Some(hal_device) = (unsafe { self.device.as_hal::<wgpu_hal::api::Vulkan>() }) else {
            bail!("wgpu device is not using the Vulkan backend");
        };
        let hal_device = &*hal_device;
        let raw_device = hal_device.raw_device();
        let raw_instance = hal_device.shared_instance().raw_instance();
        let physical_device = hal_device.raw_physical_device();

        let mut external_buffer = vk::ExternalMemoryBufferCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let buffer_info = vk::BufferCreateInfo::default()
            .push_next(&mut external_buffer)
            .size(size)
            .usage(map_buffer_usage(usage))
            .sharing_mode(vk::SharingMode::EXCLUSIVE);
        let buffer = unsafe { raw_device.create_buffer(&buffer_info, None) }
            .map_err(|err| anyhow!("vkCreateBuffer for exportable buffer failed: {err:?}"))?;
        let requirements = unsafe { raw_device.get_buffer_memory_requirements(buffer) };
        let memory_type_index = match find_device_local_memory_type(
            raw_instance,
            physical_device,
            requirements.memory_type_bits,
        ) {
            Ok(index) => index,
            Err(err) => {
                unsafe { raw_device.destroy_buffer(buffer, None) };
                return Err(err);
            }
        };
        let mut export_allocate = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .push_next(&mut export_allocate)
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        let memory = match unsafe { raw_device.allocate_memory(&allocate_info, None) } {
            Ok(memory) => memory,
            Err(err) => {
                unsafe { raw_device.destroy_buffer(buffer, None) };
                return Err(anyhow!(
                    "vkAllocateMemory for exportable buffer failed: {err:?}"
                ));
            }
        };
        let destroy = || unsafe {
            raw_device.free_memory(memory, None);
            raw_device.destroy_buffer(buffer, None);
        };
        if let Err(err) = unsafe { raw_device.bind_buffer_memory(buffer, memory, 0) } {
            destroy();
            return Err(anyhow!(
                "vkBindBufferMemory for exportable buffer failed: {err:?}"
            ));
        }
        let memory_fd = match export_memory_fd(raw_instance, raw_device, memory) {
            Ok(fd) => fd,
            Err(err) => {
                destroy();
                return Err(err);
            }
        };

        let drop_device = raw_device.clone();
        let drop_callback: wgpu_hal::DropCallback = Box::new(move || unsafe {
            drop_device.free_memory(memory, None);
            drop_device.destroy_buffer(buffer, None);
        });
        let hal_buffer =
            unsafe { wgpu_hal::vulkan::Buffer::from_raw_externally_owned(buffer, drop_callback) };
        let buffer = unsafe {
            self.device.create_buffer_from_hal::<wgpu_hal::api::Vulkan>(
                hal_buffer,
                &wgpu::BufferDescriptor {
                    label,
                    size,
                    usage,
                    mapped_at_creation: false,
                },
            )
        };

        Ok(ExportableVulkanBuffer {
            buffer: Arc::new(buffer),
            memory_fd,
            allocation_size: requirements.size,
        })
    }
}

fn vulkan_device_info(
    raw_instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
) -> VulkanDeviceInfo {
    let properties = unsafe { raw_instance.get_physical_device_properties(physical_device) };
    let mut id_properties = vk::PhysicalDeviceIDProperties::default();
    let mut properties2 = vk::PhysicalDeviceProperties2::default().push_next(&mut id_properties);
    unsafe {
        raw_instance.get_physical_device_properties2(physical_device, &mut properties2);
    }

    VulkanDeviceInfo {
        name: unsafe { CStr::from_ptr(properties.device_name.as_ptr()) }
            .to_string_lossy()
            .into_owned(),
        vendor_id: properties.vendor_id,
        device_id: properties.device_id,
        device_type: format!("{:?}", properties.device_type),
        device_uuid: id_properties.device_uuid,
        driver_uuid: id_properties.driver_uuid,
    }
}

fn export_memory_fd(
    raw_instance: &ash::Instance,
    raw_device: &ash::Device,
    memory: vk::DeviceMemory,
) -> Result<OwnedFd> {
    let external_memory_fd = ash::khr::external_memory_fd::Device::new(raw_instance, raw_device);
    let get_fd_info = vk::MemoryGetFdInfoKHR::default()
        .memory(memory)
        .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
    let fd = unsafe { external_memory_fd.get_memory_fd(&get_fd_info) }
        .map_err(|err| anyhow!("vkGetMemoryFdKHR failed: {err:?}"))?;
    Ok(unsafe { OwnedFd::from_raw_fd(fd) })
}

fn find_device_local_memory_type(
    raw_instance: &ash::Instance,
    physical_device: vk::PhysicalDevice,
    type_bits: u32,
) -> Result<u32> {
    let memory_properties =
        unsafe { raw_instance.get_physical_device_memory_properties(physical_device) };
    for index in 0..memory_properties.memory_type_count {
        let supported = (type_bits & (1 << index)) != 0;
        let flags = memory_properties.memory_types[index as usize].property_flags;
        if supported && flags.contains(vk::MemoryPropertyFlags::DEVICE_LOCAL) {
            return Ok(index);
        }
    }
    Err(anyhow!(
        "no DEVICE_LOCAL Vulkan memory type matched exportable memory requirements"
    ))
}

fn map_buffer_usage(usage: wgpu::BufferUsages) -> vk::BufferUsageFlags {
    let mut flags = vk::BufferUsageFlags::empty();
    if usage.contains(wgpu::BufferUsages::COPY_SRC) {
        flags |= vk::BufferUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(wgpu::BufferUsages::COPY_DST) {
        flags |= vk::BufferUsageFlags::TRANSFER_DST;
    }
    if usage.contains(wgpu::BufferUsages::STORAGE) {
        flags |= vk::BufferUsageFlags::STORAGE_BUFFER;
    }
    flags
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_buffer_usage() {
        let usage = map_buffer_usage(wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC);
        assert_eq!(
            usage,
            vk::BufferUsageFlags::STORAGE_BUFFER | vk::BufferUsageFlags::TRANSFER_SRC
        );
    }
}
