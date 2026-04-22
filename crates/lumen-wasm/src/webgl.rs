use wasm_bindgen::JsValue;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use std::cell::RefCell;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use js_sys::{Object, Reflect};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use wasm_bindgen::JsCast;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_sys::{CanvasRenderingContext2d, OffscreenCanvas, WebGl2RenderingContext};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use lumen::raster::RasterFrame;

use lumen::raster::ImageFrame;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use crate::{debug_error, debug_log};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
enum WebGlBackendSlot {
    Uninitialized,
    Unavailable,
    Ready(WebGlBackendContext),
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
struct WebGlBackendContext {
    canvas: OffscreenCanvas,
    context: WebGl2RenderingContext,
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
impl WebGlBackendContext {
    fn new() -> Result<Self, JsValue> {
        let canvas = OffscreenCanvas::new(1, 1)?;
        let options = Object::new();
        set_option(&options, "alpha", true)?;
        set_option(&options, "antialias", false)?;
        set_option(&options, "depth", true)?;
        set_option(&options, "stencil", true)?;
        set_option(&options, "premultipliedAlpha", true)?;
        set_option(&options, "preserveDrawingBuffer", false)?;
        let context = canvas
            .get_context_with_context_options("webgl2", &options.into())?
            .ok_or_else(|| JsValue::from_str("WebGL2 context is unavailable"))?
            .dyn_into::<WebGl2RenderingContext>()
            .map_err(|_| JsValue::from_str("expected a WebGl2RenderingContext"))?;
        configure_webgl_pixel_store(&context);
        log_pixel_store_state(&context, "new");

        Ok(Self { canvas, context })
    }

    fn resize(&self, width: u32, height: u32) {
        self.canvas.set_width(width.max(1));
        self.canvas.set_height(height.max(1));
    }
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
thread_local! {
    static WEBGL_BACKEND: RefCell<WebGlBackendSlot> = RefCell::new(WebGlBackendSlot::Uninitialized);
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn ensure_webgl_backend() {
    let _ = with_webgl_backend(|_| Ok(()));
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn draw_output_frame_to_context(
    frame: &RasterFrame,
    context: &CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    with_webgl_backend(|backend| {
        let (width, height) = frame.storage_dimensions();
        debug_log(&format!(
            "[lumen-wasm webgl] draw_output_frame_to_context storage={}x{} format={:?} data={:?}",
            width,
            height,
            frame.format_rect(),
            frame.data_rect(),
        ));
        backend.resize(width, height);
        lumen::present_webgl_image(&frame.image, width, height).map_err(|error| {
            debug_error(&format!(
                "[lumen-wasm webgl] present_webgl_image error storage={}x{}: {error}",
                width, height,
            ));
            JsValue::from_str(&error.to_string())
        })?;
        debug_log("[lumen-wasm webgl] present_webgl_image ok");
        context
            .draw_image_with_offscreen_canvas(&backend.canvas, 0.0, 0.0)
            .map_err(|error| {
                debug_error(&format!(
                    "[lumen-wasm webgl] drawImage(offscreen) error: {:?}",
                    error
                ));
                JsValue::from(error)
            })?;
        debug_log("[lumen-wasm webgl] drawImage(offscreen) ok");
        Ok(())
    })
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn image_frame_from_video_frame(
    video_frame: &web_sys::VideoFrame,
    width: u32,
    height: u32,
) -> Result<ImageFrame, JsValue> {
    with_webgl_backend(|_| {
        lumen::image_frame_from_video_frame(video_frame, width, height)
            .map_err(|error| JsValue::from_str(&error.to_string()))
    })
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn with_webgl_backend<T>(
    f: impl FnOnce(&mut WebGlBackendContext) -> Result<T, JsValue>,
) -> Result<T, JsValue> {
    WEBGL_BACKEND.with(|slot| {
        let mut slot = slot
            .try_borrow_mut()
            .map_err(|_| JsValue::from_str("WebGL backend is busy"))?;

        match &mut *slot {
            WebGlBackendSlot::Ready(backend) => {
                lumen::install_webgl_context(backend.context.clone());
                configure_webgl_pixel_store(&backend.context);
                log_pixel_store_state(&backend.context, "reuse");
                return f(backend);
            }
            WebGlBackendSlot::Unavailable => {
                return Err(JsValue::from_str("WebGL backend is unavailable"));
            }
            WebGlBackendSlot::Uninitialized => {}
        }

        match WebGlBackendContext::new() {
            Ok(backend) => {
                *slot = WebGlBackendSlot::Ready(backend);
            }
            Err(_) => {
                *slot = WebGlBackendSlot::Unavailable;
                return Err(JsValue::from_str("WebGL backend is unavailable"));
            }
        }

        let WebGlBackendSlot::Ready(backend) = &mut *slot else {
            return Err(JsValue::from_str("WebGL backend is unavailable"));
        };
        lumen::install_webgl_context(backend.context.clone());
        configure_webgl_pixel_store(&backend.context);
        log_pixel_store_state(&backend.context, "ready");
        f(backend)
    })
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn configure_webgl_pixel_store(context: &WebGl2RenderingContext) {
    context.pixel_storei(WebGl2RenderingContext::UNPACK_ALIGNMENT, 1);
    context.pixel_storei(WebGl2RenderingContext::PACK_ALIGNMENT, 1);
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn log_pixel_store_state(context: &WebGl2RenderingContext, stage: &str) {
    let unpack_alignment = context
        .get_parameter(WebGl2RenderingContext::UNPACK_ALIGNMENT)
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(-1.0);
    let pack_alignment = context
        .get_parameter(WebGl2RenderingContext::PACK_ALIGNMENT)
        .ok()
        .and_then(|value| value.as_f64())
        .unwrap_or(-1.0);
    debug_log(&format!(
        "[lumen-wasm webgl] pixelStore({stage}) unpack_alignment={unpack_alignment} pack_alignment={pack_alignment}"
    ));
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
fn set_option(options: &Object, name: &str, value: bool) -> Result<(), JsValue> {
    Reflect::set(
        options,
        &JsValue::from_str(name),
        &JsValue::from_bool(value),
    )
    .map(|_| ())
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) fn ensure_webgl_backend() {}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) fn draw_output_frame_to_context(
    _frame: &lumen::raster::RasterFrame,
    _context: &web_sys::CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    Err(JsValue::from_str("WebGL backend is unavailable"))
}

#[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
pub(crate) fn image_frame_from_video_frame(
    _video_frame: &web_sys::VideoFrame,
    _width: u32,
    _height: u32,
) -> Result<ImageFrame, JsValue> {
    Err(JsValue::from_str("WebGL backend is unavailable"))
}
