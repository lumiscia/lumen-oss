mod media;
mod preview;
mod renderer;
mod types;
mod utils;

use std::sync::Once;

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use tracing_subscriber::{Layer, filter::filter_fn, layer::SubscriberExt};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use wasm_bindgen::JsValue;

pub use media::LumenMediaStore;
pub use preview::LumenPreviewController;
pub use renderer::LumenRenderer;

static INSTALL_RUNTIME_HOOKS: Once = Once::new();

pub(crate) fn debug_error(message: &str) {
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    web_sys::console::error_1(&JsValue::from_str(message));
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    let _ = message;
}

pub(crate) fn install_panic_hook() {
    INSTALL_RUNTIME_HOOKS.call_once(|| {
        #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
        {
            let mut tracing_config = wasm_tracing::WasmLayerConfig::new();
            tracing_config
                .set_max_level(tracing::Level::TRACE)
                .set_show_fields(true);
            let layer =
                wasm_tracing::WasmLayer::new(tracing_config).with_filter(filter_fn(|metadata| {
                    lumen_engine::log_level_enabled(*metadata.level())
                }));
            let subscriber = tracing_subscriber::registry().with(layer);
            let _ = tracing::subscriber::set_global_default(subscriber);
        }

        std::panic::set_hook(Box::new(|panic_info| {
            let location = panic_info
                .location()
                .map(|location| {
                    format!(
                        "{}:{}:{}",
                        location.file(),
                        location.line(),
                        location.column()
                    )
                })
                .unwrap_or_else(|| "unknown location".to_string());
            debug_error(&format!("[lumen-wasm panic] {panic_info} @ {location}"));
        }));
    });
}
