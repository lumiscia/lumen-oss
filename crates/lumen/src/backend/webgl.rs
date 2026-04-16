use skia_safe::Surface;

use super::create_gpu_surface;
use crate::error::RenderError;

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
        })
        .ok()?
        .flatten()
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
    presentation_surface: Option<PresentationSurface>,
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
struct PresentationSurface {
    width: u32,
    height: u32,
    surface: Surface,
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
    match with_state_mut(|state| state.present_image(image, width, height))? {
        Some(result) => Ok(result?),
        None => Err(RenderError::SurfaceAllocation { width, height }.into()),
    }
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
            presentation_surface: None,
        })
    }

    fn make_current(&self) {
        skia_safe::gpu::gl::set_gl_context(self.context_id);
    }

    fn present_image(
        &mut self,
        image: &skia_safe::Image,
        width: u32,
        height: u32,
    ) -> Result<(), RenderError> {
        self.make_current();

        let WebGlState {
            context,
            presentation_surface,
            ..
        } = self;

        let needs_new_surface = presentation_surface
            .as_ref()
            .map(|surface| surface.width != width || surface.height != height)
            .unwrap_or(true);

        if needs_new_surface {
            *presentation_surface = Some(PresentationSurface {
                width,
                height,
                surface: create_presentation_surface(context, width, height)?,
            });
        }

        let surface = presentation_surface
            .as_mut()
            .map(|surface| &mut surface.surface)
            .ok_or(RenderError::SurfaceAllocation { width, height })?;
        let canvas = surface.canvas();
        canvas.clear(Color::TRANSPARENT);
        canvas.draw_image(image, (0.0, 0.0), None);
        context.flush_and_submit_surface(surface, None);
        Ok(())
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn create_presentation_surface(
    context: &mut skia_safe::gpu::DirectContext,
    width: u32,
    height: u32,
) -> Result<Surface, RenderError> {
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

    gpu::surfaces::wrap_backend_render_target(
        context,
        &target,
        SurfaceOrigin::BottomLeft,
        ColorType::RGBA8888,
        None,
        None,
    )
    .ok_or(RenderError::SurfaceAllocation { width, height })
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn with_state_mut<T>(f: impl FnOnce(&mut WebGlState) -> T) -> Result<Option<T>, RenderError> {
    WEBGL_STATE.with(|slot| {
        let mut slot = slot
            .try_borrow_mut()
            .map_err(|_| RenderError::BackendBusy { backend: "webgl" })?;
        if matches!(*slot, WebGlStateSlot::Uninitialized) {
            *slot = WebGlState::try_create()
                .map(WebGlStateSlot::Ready)
                .unwrap_or(WebGlStateSlot::Unavailable);
        }

        match &mut *slot {
            WebGlStateSlot::Ready(state) => Ok(Some(f(state))),
            WebGlStateSlot::Uninitialized | WebGlStateSlot::Unavailable => Ok(None),
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
fn with_state_mut<T>(_f: impl FnOnce(&mut WebGlState) -> T) -> Result<Option<T>, RenderError> {
    Ok(None)
}
