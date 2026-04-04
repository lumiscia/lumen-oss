use skia_safe::Surface;

use super::create_gpu_surface;

pub(crate) struct WebGlSurfaceFactory {
    state: WebGlStateSlot,
}

enum WebGlStateSlot {
    Uninitialized,
    Unavailable,
    Ready(WebGlState),
}

impl WebGlSurfaceFactory {
    pub(crate) fn new() -> Self {
        Self {
            state: WebGlStateSlot::Uninitialized,
        }
    }

    pub(crate) fn create_surface(&mut self, width: u32, height: u32) -> Option<Surface> {
        let state = self.ensure_state()?;
        create_gpu_surface(&mut state.context, width, height)
    }

    fn ensure_state(&mut self) -> Option<&mut WebGlState> {
        if matches!(self.state, WebGlStateSlot::Uninitialized) {
            self.state = WebGlState::try_create()
                .map(WebGlStateSlot::Ready)
                .unwrap_or(WebGlStateSlot::Unavailable)
        }

        match &mut self.state {
            WebGlStateSlot::Ready(state) => Some(state),
            WebGlStateSlot::Uninitialized | WebGlStateSlot::Unavailable => None,
        }
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "emscripten"))]
struct WebGlState {
    context: skia_safe::gpu::DirectContext,
}

#[cfg(all(target_arch = "wasm32", target_os = "emscripten"))]
impl WebGlState {
    fn try_create() -> Option<Self> {
        use std::ffi::CString;

        use skia_safe::gpu;

        unsafe extern "C" {
            fn emscripten_GetProcAddress(
                name: *const std::os::raw::c_char,
            ) -> *const std::ffi::c_void;
        }

        let interface = gpu::gl::Interface::new_load_with(|name| {
            let Ok(name) = CString::new(name) else {
                return std::ptr::null();
            };
            unsafe { emscripten_GetProcAddress(name.as_ptr()) }
        })?;
        if !interface.validate() {
            return None;
        }
        let context = gpu::direct_contexts::make_gl(interface, None)?;

        Some(Self { context })
    }
}

#[cfg(not(all(target_arch = "wasm32", target_os = "emscripten")))]
struct WebGlState {
    context: skia_safe::gpu::DirectContext,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "emscripten")))]
impl WebGlState {
    fn try_create() -> Option<Self> {
        None
    }
}
