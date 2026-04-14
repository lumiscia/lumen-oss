#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use std::cell::RefCell;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use js_sys::{Object, Reflect};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use wasm_bindgen::{JsCast, JsValue};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use web_sys::{CanvasRenderingContext2d, OffscreenCanvas, WebGl2RenderingContext};

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use lumen::raster::RasterFrame;

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
    WEBGL_BACKEND.with(|slot| {
        let mut slot = slot.borrow_mut();
        match &*slot {
            WebGlBackendSlot::Ready(backend) => {
                lumen::install_webgl_context(backend.context.clone());
            }
            WebGlBackendSlot::Unavailable => {}
            WebGlBackendSlot::Uninitialized => match WebGlBackendContext::new() {
                Ok(backend) => {
                    lumen::install_webgl_context(backend.context.clone());
                    *slot = WebGlBackendSlot::Ready(backend);
                }
                Err(_) => {
                    *slot = WebGlBackendSlot::Unavailable;
                }
            },
        }
    });
}

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
pub(crate) fn draw_output_frame_to_context(
    frame: &RasterFrame,
    context: &CanvasRenderingContext2d,
) -> Result<(), JsValue> {
    ensure_webgl_backend();
    WEBGL_BACKEND.with(|slot| {
        let mut slot = slot.borrow_mut();
        let WebGlBackendSlot::Ready(backend) = &mut *slot else {
            return Err(JsValue::from_str("WebGL backend is unavailable"));
        };

        let (width, height) = frame.storage_dimensions();
        backend.resize(width, height);
        lumen::present_webgl_image(&frame.image, width, height)
            .map_err(|error| JsValue::from_str(&error.to_string()))?;
        context
            .draw_image_with_offscreen_canvas(&backend.canvas, 0.0, 0.0)
            .map_err(|error| JsValue::from(error))?;
        Ok(())
    })
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
