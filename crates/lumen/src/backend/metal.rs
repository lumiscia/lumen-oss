use skia_safe::Surface;

use super::create_gpu_surface;

pub(crate) struct MetalSurfaceFactory {
    state: MetalStateSlot,
}

enum MetalStateSlot {
    Uninitialized,
    Unavailable,
    Ready(MetalState),
}

impl MetalSurfaceFactory {
    pub(crate) fn new() -> Self {
        Self {
            state: MetalStateSlot::Uninitialized,
        }
    }

    pub(crate) fn create_surface(&mut self, width: u32, height: u32) -> Option<Surface> {
        let state = self.ensure_state()?;
        create_gpu_surface(&mut state.context, width, height)
    }

    fn ensure_state(&mut self) -> Option<&mut MetalState> {
        if matches!(self.state, MetalStateSlot::Uninitialized) {
            self.state = MetalState::try_create()
                .map(MetalStateSlot::Ready)
                .unwrap_or(MetalStateSlot::Unavailable)
        }

        match &mut self.state {
            MetalStateSlot::Ready(state) => Some(state),
            MetalStateSlot::Uninitialized | MetalStateSlot::Unavailable => None,
        }
    }
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
struct MetalState {
    context: skia_safe::gpu::DirectContext,
    _device: objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLDevice>>,
    _queue: objc2::rc::Retained<objc2::runtime::ProtocolObject<dyn objc2_metal::MTLCommandQueue>>,
}

#[cfg(any(target_os = "macos", target_os = "ios"))]
impl MetalState {
    fn try_create() -> Option<Self> {
        use objc2::rc::Retained;
        use objc2_metal::{MTLCreateSystemDefaultDevice, MTLDevice};
        use skia_safe::gpu;

        let device = MTLCreateSystemDefaultDevice()?;
        let queue = device.newCommandQueue()?;

        let backend = unsafe {
            gpu::mtl::BackendContext::new(
                Retained::as_ptr(&device) as gpu::mtl::Handle,
                Retained::as_ptr(&queue) as gpu::mtl::Handle,
            )
        };

        let context = gpu::direct_contexts::make_metal(&backend, None)?;
        Some(Self {
            context,
            _device: device,
            _queue: queue,
        })
    }
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
struct MetalState {
    context: skia_safe::gpu::DirectContext,
}

#[cfg(not(any(target_os = "macos", target_os = "ios")))]
impl MetalState {
    fn try_create() -> Option<Self> {
        None
    }
}
