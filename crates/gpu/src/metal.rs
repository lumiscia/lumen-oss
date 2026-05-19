use anyhow::{Result, bail};
use objc2::rc::Retained;

pub type Objc2MetalDevice = objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>;
pub type Objc2MetalTexture = objc2::runtime::ProtocolObject<dyn objc2_metal::MTLTexture>;

pub fn metal_device_from_wgpu(device: &wgpu::Device) -> Result<Retained<Objc2MetalDevice>> {
    let hal_device = unsafe { device.as_hal::<wgpu::hal::api::Metal>() };
    let Some(hal_device) = hal_device else {
        bail!("wgpu device is not using the Metal backend");
    };
    Ok(hal_device.raw_device().clone())
}

pub fn texture_from_metal(
    device: &wgpu::Device,
    texture: Retained<Objc2MetalTexture>,
    desc: &wgpu::TextureDescriptor<'_>,
) -> wgpu::Texture {
    let hal_texture = unsafe {
        wgpu::hal::metal::Device::texture_from_raw(
            texture,
            desc.format,
            objc2_metal::MTLTextureType::Type2D,
            desc.sample_count,
            desc.mip_level_count,
            wgpu::hal::CopyExtent {
                width: desc.size.width,
                height: desc.size.height,
                depth: desc.size.depth_or_array_layers,
            },
        )
    };
    unsafe { device.create_texture_from_hal::<wgpu::hal::api::Metal>(hal_texture, desc) }
}
