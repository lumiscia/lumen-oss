use std::cell::{Cell, RefCell};

use skia_safe::{
    AlphaType, Color, ColorType, Surface,
    gpu::{
        self, Mipmapped, SurfaceOrigin,
        gl::{self, FramebufferInfo},
    },
};
use web_sys::WebGl2RenderingContext;

use super::create_gpu_surface;
use crate::error::RenderError;
use crate::gpu_image::GpuImageFrame;

thread_local! {
    static WEBGL_CONTEXT_ID: Cell<Option<gl::glemu::ContextId>> = const { Cell::new(None) };
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

struct WebGlState {
    context: skia_safe::gpu::DirectContext,
    context_id: gl::glemu::ContextId,
    presentation_surface: Option<PresentationSurface>,
}

struct PresentationSurface {
    width: u32,
    height: u32,
    surface: Surface,
}

pub fn install_webgl_context(context: WebGl2RenderingContext) {
    configure_webgl_pixel_store(&context);
    let context_id = WEBGL_CONTEXT_ID.with(|slot| {
        if let Some(context_id) = slot.get() {
            context_id
        } else {
            let context_id = gl::glemu::register_gl_context(context);
            slot.set(Some(context_id));
            context_id
        }
    });
    gl::glemu::set_gl_context(context_id);
}

pub fn present_webgl_image(image: &skia_safe::Image, width: u32, height: u32) -> crate::Result<()> {
    match with_state_mut(|state| state.present_image(image, width, height))? {
        Some(result) => Ok(result?),
        None => Err(RenderError::SurfaceAllocation { width, height }.into()),
    }
}

pub fn with_webgl_surface_context<T>(
    width: u32,
    height: u32,
    f: impl FnOnce(&mut skia_safe::gpu::DirectContext) -> crate::Result<T>,
) -> crate::Result<T> {
    match with_state_mut(|state| {
        state.make_current();
        f(&mut state.context)
    })? {
        Some(result) => result,
        None => Err(RenderError::SurfaceAllocation { width, height }.into()),
    }
}

pub fn image_frame_from_video_frame(
    video_frame: &web_sys::VideoFrame,
    width: u32,
    height: u32,
) -> crate::Result<GpuImageFrame> {
    match with_state_mut(|state| state.image_frame_from_video_frame(video_frame, width, height))? {
        Some(result) => result,
        None => Err(RenderError::SurfaceAllocation { width, height }.into()),
    }
}

impl WebGlState {
    fn try_create() -> Option<Self> {
        let context_id = WEBGL_CONTEXT_ID.with(Cell::get)?;
        gl::glemu::set_gl_context(context_id);
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
        gl::glemu::set_gl_context(self.context_id);
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

    fn image_frame_from_video_frame(
        &mut self,
        video_frame: &web_sys::VideoFrame,
        width: u32,
        height: u32,
    ) -> crate::Result<GpuImageFrame> {
        self.make_current();

        let texture_width = i32::try_from(width.max(1))
            .map_err(|_| RenderError::SurfaceAllocation { width, height })?;
        let texture_height = i32::try_from(height.max(1))
            .map_err(|_| RenderError::SurfaceAllocation { width, height })?;
        let gl =
            glemu::Context::current().ok_or(RenderError::SurfaceAllocation { width, height })?;
        let texture = gl
            .create_texture()
            .ok_or(RenderError::SurfaceAllocation { width, height })?;
        let webgl = gl.webgl2_context();

        gl.bind_texture(WebGl2RenderingContext::TEXTURE_2D, Some(texture));
        webgl.pixel_storei(WebGl2RenderingContext::UNPACK_FLIP_Y_WEBGL, 0);
        gl.tex_image_2d_with_video_frame_and_width_and_height(
            WebGl2RenderingContext::TEXTURE_2D,
            0,
            WebGl2RenderingContext::RGBA as i32,
            texture_width,
            texture_height,
            WebGl2RenderingContext::RGBA,
            WebGl2RenderingContext::UNSIGNED_BYTE,
            video_frame,
        )
        .map_err(|_| RenderError::SurfaceAllocation { width, height })?;

        let texture_info = gpu::gl::TextureInfo {
            target: WebGl2RenderingContext::TEXTURE_2D,
            id: texture.raw_id(),
            format: gpu::gl::Format::RGBA8.into(),
            protected: gpu::Protected::No,
        };
        let backend_texture = unsafe {
            gpu::backend_textures::make_gl(
                (texture_width, texture_height),
                Mipmapped::No,
                texture_info,
                "lumen-video-frame",
            )
        };
        let image = gpu::images::adopt_texture_from(
            &mut self.context,
            &backend_texture,
            SurfaceOrigin::TopLeft,
            ColorType::RGBA8888,
            AlphaType::Premul,
            None,
        )
        .ok_or(RenderError::SurfaceAllocation { width, height })?;

        self.context.flush_and_submit();
        Ok(GpuImageFrame::image(image, width, height))
    }
}

fn configure_webgl_pixel_store(context: &WebGl2RenderingContext) {
    context.pixel_storei(WebGl2RenderingContext::UNPACK_ALIGNMENT, 1);
    context.pixel_storei(WebGl2RenderingContext::PACK_ALIGNMENT, 1);
}

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
