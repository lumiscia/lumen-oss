use skia_safe::Surface;

use super::create_gpu_surface;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use std::cell::{Cell, RefCell};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use skia_safe::{
    Color, ColorType,
    gpu::{self, SurfaceOrigin, gl::FramebufferInfo},
};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_sys::WebGl2RenderingContext;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
thread_local! {
    static WEBGL_CONTEXT_ID: Cell<Option<u32>> = const { Cell::new(None) };
    static WEBGL_STATE: RefCell<WebGlStateSlot> = const { RefCell::new(WebGlStateSlot::Uninitialized) };
}

pub(crate) struct WebGlSurfaceFactory;

enum WebGlStateSlot {
    Uninitialized,
    Unavailable,
    Ready(WebGlState),
}

impl WebGlSurfaceFactory {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn create_surface(&mut self, width: u32, height: u32) -> Option<Surface> {
        with_state_mut(|state| {
            state.make_current();
            create_gpu_surface(&mut state.context, width, height)
        })?
    }

    pub(crate) fn flush(&mut self) {
        let _ = with_state_mut(|state| {
            state.make_current();
            state.context.flush_and_submit();
        });
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
struct WebGlState {
    context: skia_safe::gpu::DirectContext,
    context_id: u32,
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn install_webgl_context(context: WebGl2RenderingContext) {
    let context_id = WEBGL_CONTEXT_ID.with(|slot| {
        if let Some(context_id) = slot.get() {
            context_id
        } else {
            let context_id = skia_safe::gpu::gl::register_gl_context(context);
            slot.set(Some(context_id));
            context_id
        }
    });
    skia_safe::gpu::gl::set_gl_context(context_id);
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub fn present_webgl_image(image: &skia_safe::Image, width: u32, height: u32) -> crate::Result<()> {
    use crate::error::RenderError;

    with_state_mut(|state| {
        state.make_current();

        let size = (
            i32::try_from(width.max(1))
                .map_err(|_| RenderError::SurfaceAllocation { width, height })?,
            i32::try_from(height.max(1))
                .map_err(|_| RenderError::SurfaceAllocation { width, height })?,
        );
        let target = gpu::backend_render_targets::make_gl(
            size,
            0,
            8,
            FramebufferInfo {
                fboid: 0,
                format: gpu::gl::Format::RGBA8.into(),
                protected: gpu::Protected::No,
            },
        );
        let mut surface = gpu::surfaces::wrap_backend_render_target(
            &mut state.context,
            &target,
            SurfaceOrigin::BottomLeft,
            ColorType::RGBA8888,
            None,
            None,
        )
        .ok_or(RenderError::SurfaceAllocation { width, height })?;

        let canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        canvas.draw_image(image, (0.0, 0.0), None);
        state.context.flush_and_submit_surface(&mut surface, None);
        Ok(())
    })
    .unwrap_or_else(|| Err(RenderError::SurfaceAllocation { width, height }.into()))
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl WebGlState {
    fn try_create() -> Option<Self> {
        use skia_safe::gpu;

        let context_id = WEBGL_CONTEXT_ID.with(Cell::get)?;
        skia_safe::gpu::gl::set_gl_context(context_id);
        let interface = gpu::gl::Interface::new_web_sys()?;
        if !interface.validate() {
            return None;
        }
        let context = gpu::direct_contexts::make_gl(interface, None)?;

        Some(Self {
            context,
            context_id,
        })
    }

    fn make_current(&self) {
        skia_safe::gpu::gl::set_gl_context(self.context_id);
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn with_state_mut<T>(f: impl FnOnce(&mut WebGlState) -> T) -> Option<T> {
    WEBGL_STATE.with(|slot| {
        let mut slot = slot.borrow_mut();
        if matches!(*slot, WebGlStateSlot::Uninitialized) {
            *slot = WebGlState::try_create()
                .map(WebGlStateSlot::Ready)
                .unwrap_or(WebGlStateSlot::Unavailable);
        }

        match &mut *slot {
            WebGlStateSlot::Ready(state) => Some(f(state)),
            WebGlStateSlot::Uninitialized | WebGlStateSlot::Unavailable => None,
        }
    })
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
struct WebGlState {
    context: skia_safe::gpu::DirectContext,
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
impl WebGlState {
    fn try_create() -> Option<Self> {
        None
    }

    fn make_current(&self) {}
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub fn present_webgl_image(
    _image: &skia_safe::Image,
    width: u32,
    height: u32,
) -> crate::Result<()> {
    Err(crate::error::RenderError::SurfaceAllocation { width, height }.into())
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
fn with_state_mut<T>(_f: impl FnOnce(&mut WebGlState) -> T) -> Option<T> {
    None
}
