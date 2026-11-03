mod media;
mod preview;
mod renderer;
mod types;
mod utils;

use std::sync::Once;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use std::sync::OnceLock;

use tracing::level_filters::LevelFilter;
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use tracing_subscriber::{Layer, Registry, layer::SubscriberExt, reload};
#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
use wasm_bindgen::JsValue;

pub use preview::LumenPreviewController;

static INSTALL_RUNTIME_HOOKS: Once = Once::new();

#[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
static LOG_FILTER: OnceLock<reload::Handle<LevelFilter, Registry>> = OnceLock::new();

/// Sets the most verbose level that reaches the browser console. Logging starts off.
pub(crate) fn set_log_level(level: &str) -> Result<(), String> {
    let filter = match level {
        "off" => LevelFilter::OFF,
        "error" => LevelFilter::ERROR,
        "warn" => LevelFilter::WARN,
        "info" => LevelFilter::INFO,
        "debug" => LevelFilter::DEBUG,
        "trace" => LevelFilter::TRACE,
        _ => {
            return Err(format!(
                "invalid log level '{level}', expected off, error, warn, info, debug, or trace"
            ));
        }
    };
    #[cfg(all(target_arch = "wasm32", target_os = "unknown"))]
    if let Some(handle) = LOG_FILTER.get() {
        let _ = handle.reload(filter);
    }
    #[cfg(not(all(target_arch = "wasm32", target_os = "unknown")))]
    let _ = filter;
    Ok(())
}

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
            let (filter, handle) = reload::Layer::new(LevelFilter::OFF);
            let _ = LOG_FILTER.set(handle);
            let layer = wasm_tracing::WasmLayer::new(tracing_config).with_filter(filter);
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
