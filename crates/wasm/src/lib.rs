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

/// What is wrong with a composition, without loading or rendering it: an array of diagnostics,
/// errors and warnings both. With `fonts`, the families the page can load, a text family outside
/// them that isn't the bundled one is a `font.fallback` warning; without it, fonts aren't checked.
/// `asset:` references aren't checked, since the page has no asset store to check them against.
#[wasm_bindgen::prelude::wasm_bindgen]
pub fn check(composition_json: &str, fonts: Option<Vec<String>>) -> wasm_bindgen::JsValue {
    let known_font = |family: &str| fonts.iter().flatten().any(|font| font == family);
    let known_font: Option<&dyn Fn(&str) -> bool> = fonts.is_some().then_some(&known_font);
    utils::diagnostics_value(&lumen_engine::json::check(
        composition_json,
        known_font,
        None,
    ))
}

/// What a composition needs before it renders: `{ media, dynamic, fonts }`, as
/// `Composition::media_dependencies` lists them. Throws as `loadComposition` would on a
/// composition the engine rejects, with its `diagnostics`.
#[wasm_bindgen::prelude::wasm_bindgen(js_name = "mediaDependencies")]
pub fn media_dependencies(
    composition_json: &str,
) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    let composition = utils::composition_json_to_composition(composition_json)
        .map_err(|error| utils::parse_failure(&error))?;
    serde_json::to_string(&composition.media_dependencies())
        .ok()
        .and_then(|json| js_sys::JSON::parse(&json).ok())
        .ok_or_else(|| js_sys::Error::new("the composition's media could not be returned").into())
}

/// The animation clips and materials of a binary glTF (`.glb`), which a `model_3d` selects with
/// `animation` and `texture_material`: `{ animations: [{ index, name?, duration }], materials:
/// [{ index, name?, hasBaseColorTexture }] }`, with `duration` in seconds. Throws as a render
/// would on a file that isn't a model it can read.
#[cfg(feature = "3d")]
#[wasm_bindgen::prelude::wasm_bindgen(js_name = "modelInfo")]
pub fn model_info(bytes: &[u8]) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    let info = lumen_3d::model::ModelInfo::from_glb(bytes)
        .map_err(|error| js_sys::Error::new(&format!("the model could not be read: {error}")))?;
    serde_json::to_string(&info)
        .ok()
        .and_then(|json| js_sys::JSON::parse(&json).ok())
        .ok_or_else(|| js_sys::Error::new("the model's info could not be returned").into())
}

/// Throws: this build has no 3D support.
#[cfg(not(feature = "3d"))]
#[wasm_bindgen::prelude::wasm_bindgen(js_name = "modelInfo")]
pub fn model_info(_bytes: &[u8]) -> Result<wasm_bindgen::JsValue, wasm_bindgen::JsValue> {
    Err(js_sys::Error::new("this build of Lumen has no 3D support").into())
}

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
