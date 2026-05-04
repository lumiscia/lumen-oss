use std::{
    os::fd::{AsRawFd, FromRawFd, OwnedFd},
    sync::Arc,
};

use anyhow::{Result, anyhow, bail};
use ash::vk;

use crate::{Renderer, Size};

pub struct ExportableVulkanTexture {
    texture: Arc<wgpu::Texture>,
    memory_fd: OwnedFd,
    allocation_size: u64,
    row_pitch: u64,
}

impl ExportableVulkanTexture {
    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn texture_arc(&self) -> Arc<wgpu::Texture> {
        Arc::clone(&self.texture)
    }

    pub fn memory_fd(&self) -> &OwnedFd {
        &self.memory_fd
    }

    pub fn memory_fd_raw(&self) -> i32 {
        self.memory_fd.as_raw_fd()
    }

    pub fn allocation_size(&self) -> u64 {
        self.allocation_size
    }

    pub fn row_pitch(&self) -> u64 {
        self.row_pitch
    }
}

impl Renderer {
    pub fn create_exportable_vulkan_texture(
        &self,
        label: Option<&str>,
        size: Size,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> Result<ExportableVulkanTexture> {
        let Some(hal_device) = (unsafe { self.device.as_hal::<wgpu_hal::api::Vulkan>() }) else {
            bail!("wgpu device is not using the Vulkan backend");
        };
        let hal_device = &*hal_device;
        let raw_device = hal_device.raw_device();
        let raw_instance = hal_device.shared_instance().raw_instance();

        let vk_format = map_texture_format(format)?;
        let vk_extent = vk::Extent3D {
            width: size.width,
            height: size.height,
            depth: 1,
        };
        let mut external_image = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let image_info = vk::ImageCreateInfo::default()
            .push_next(&mut external_image)
            .image_type(vk::ImageType::TYPE_2D)
            .format(vk_format)
            .extent(vk_extent)
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::OPTIMAL)
            .usage(map_texture_usage(usage))
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);

        let image = unsafe { raw_device.create_image(&image_info, None) }
            .map_err(|err| anyhow!("vkCreateImage for exportable texture failed: {err:?}"))?;
        let requirements = unsafe { raw_device.get_image_memory_requirements(image) };
        let memory_type_index = find_device_local_memory_type(
            raw_instance,
            hal_device.raw_physical_device(),
            requirements.memory_type_bits,
        )?;
        let mut export_allocate = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let allocate_info = vk::MemoryAllocateInfo::default()
            .push_next(&mut export_allocate)
            .allocation_size(requirements.size)
            .memory_type_index(memory_type_index);
        let memory = match unsafe { raw_device.allocate_memory(&allocate_info, None) } {
            Ok(memory) => memory,
            Err(err) => {
                unsafe { raw_device.destroy_image(image, None) };
                return Err(anyhow!(
                    "vkAllocateMemory for exportable texture failed: {err:?}"
                ));
            }
        };
        if let Err(err) = unsafe { raw_device.bind_image_memory(image, memory, 0) } {
            unsafe {
                raw_device.free_memory(memory, None);
                raw_device.destroy_image(image, None);
            }
            return Err(anyhow!(
                "vkBindImageMemory for exportable texture failed: {err:?}"
            ));
        }

        let memory_fd = match export_memory_fd(raw_instance, raw_device, memory) {
            Ok(fd) => fd,
            Err(err) => {
                unsafe {
                    raw_device.free_memory(memory, None);
                    raw_device.destroy_image(image, None);
                }
                return Err(err);
            }
        };

        let hal_desc = wgpu_hal::TextureDescriptor {
            label,
            size: wgpu::Extent3d {
                width: size.width,
                height: size.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: map_hal_texture_usage(usage),
            memory_flags: wgpu_hal::MemoryFlags::empty(),
            view_formats: Vec::new(),
        };
        let wgpu_desc = wgpu::TextureDescriptor {
            label,
            size: hal_desc.size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage,
            view_formats: &[],
        };
        let drop_device = raw_device.clone();
        let drop_callback: wgpu_hal::DropCallback = Box::new(move || unsafe {
            drop_device.free_memory(memory, None);
            drop_device.destroy_image(image, None);
        });
        let hal_texture = unsafe {
            hal_device.texture_from_raw(
                image,
                &hal_desc,
                Some(drop_callback),
                wgpu_hal::vulkan::TextureMemory::External,
            )
        };
        let texture = unsafe {
            self.device
                .create_texture_from_hal::<wgpu_hal::api::Vulkan>(hal_texture, &wgpu_desc)
        };

        Ok(ExportableVulkanTexture {
            texture: Arc::new(texture),
            memory_fd,
            allocation_size: requirements.size,
            row_pitch: u64::from(size.width).saturating_mul(4),
        })
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
        "no DEVICE_LOCAL Vulkan memory type matched exportable image requirements"
    ))
}

fn map_texture_format(format: wgpu::TextureFormat) -> Result<vk::Format> {
    match format {
        wgpu::TextureFormat::Rgba8Unorm => Ok(vk::Format::R8G8B8A8_UNORM),
        wgpu::TextureFormat::Bgra8Unorm => Ok(vk::Format::B8G8R8A8_UNORM),
        other => Err(anyhow!(
            "exportable Vulkan texture format {other:?} is not mapped"
        )),
    }
}

fn map_texture_usage(usage: wgpu::TextureUsages) -> vk::ImageUsageFlags {
    let mut flags = vk::ImageUsageFlags::empty();
    if usage.contains(wgpu::TextureUsages::COPY_SRC) {
        flags |= vk::ImageUsageFlags::TRANSFER_SRC;
    }
    if usage.contains(wgpu::TextureUsages::COPY_DST) {
        flags |= vk::ImageUsageFlags::TRANSFER_DST;
    }
    if usage.contains(wgpu::TextureUsages::TEXTURE_BINDING) {
        flags |= vk::ImageUsageFlags::SAMPLED;
    }
    if usage.contains(wgpu::TextureUsages::STORAGE_BINDING) {
        flags |= vk::ImageUsageFlags::STORAGE;
    }
    if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        flags |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    flags
}

fn map_hal_texture_usage(usage: wgpu::TextureUsages) -> wgpu::TextureUses {
    let mut uses = wgpu::TextureUses::empty();
    if usage.contains(wgpu::TextureUsages::COPY_SRC) {
        uses |= wgpu::TextureUses::COPY_SRC;
    }
    if usage.contains(wgpu::TextureUsages::COPY_DST) {
        uses |= wgpu::TextureUses::COPY_DST;
    }
    if usage.contains(wgpu::TextureUsages::TEXTURE_BINDING) {
        uses |= wgpu::TextureUses::RESOURCE;
    }
    if usage.contains(wgpu::TextureUsages::STORAGE_BINDING) {
        uses |= wgpu::TextureUses::STORAGE_WRITE_ONLY;
    }
    if usage.contains(wgpu::TextureUsages::RENDER_ATTACHMENT) {
        uses |= wgpu::TextureUses::COLOR_TARGET;
    }
    uses
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_rgba8_exportable_format() {
        assert_eq!(
            map_texture_format(wgpu::TextureFormat::Rgba8Unorm).unwrap(),
            vk::Format::R8G8B8A8_UNORM
        );
    }

    #[test]
    fn maps_copy_destination_usage() {
        let usage = map_texture_usage(
            wgpu::TextureUsages::COPY_DST
                | wgpu::TextureUsages::COPY_SRC
                | wgpu::TextureUsages::TEXTURE_BINDING,
        );
        assert!(usage.contains(vk::ImageUsageFlags::TRANSFER_DST));
        assert!(usage.contains(vk::ImageUsageFlags::TRANSFER_SRC));
        assert!(usage.contains(vk::ImageUsageFlags::SAMPLED));
    }
}
