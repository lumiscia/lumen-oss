use objc2::rc::Retained;
use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice, MTLCommandQueue};
use skia_safe::gpu;

use super::{GpuBackend, GpuState, create_gpu_surface};

pub(super) struct MetalState {
    pub _device: Retained<objc2::runtime::ProtocolObject<dyn MTLDevice>>,
    pub _queue: Retained<objc2::runtime::ProtocolObject<dyn MTLCommandQueue>>,
}

pub(super) fn try_create(width: u32, height: u32) -> Option<(skia_safe::Surface, GpuState)> {
    let device = MTLCreateSystemDefaultDevice()?;
    let queue = device.newCommandQueue()?;

    let backend = unsafe {
        gpu::mtl::BackendContext::new(
            Retained::as_ptr(&device) as gpu::mtl::Handle,
            Retained::as_ptr(&queue) as gpu::mtl::Handle,
        )
    };

    let mut context = gpu::direct_contexts::make_metal(&backend, None)?;
    let surface = create_gpu_surface(&mut context, width, height).ok()?;

    Some((
        surface,
        GpuState {
            context,
            _backend: GpuBackend::Metal(MetalState {
                _device: device,
                _queue: queue,
            }),
        },
    ))
}
