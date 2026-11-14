//! The one device the library's GPU tests share. Instances created and dropped on parallel test
//! threads race inside the Vulkan loader, which crashes in `vkSetDebugUtilsObjectNameEXT` that
//! debug builds call on every command buffer (<https://github.com/gfx-rs/wgpu/issues/5270>).

use std::sync::{LazyLock, Mutex, MutexGuard, PoisonError};

static GPU: LazyLock<Result<Mutex<lumen_gpu::Renderer>, String>> = LazyLock::new(|| {
    pollster::block_on(lumen_gpu::Renderer::new())
        .map(Mutex::new)
        .map_err(|error| format!("{error:#}"))
});

/// Waits for the shared device, which holds a test alone on it, or says why there is none.
pub(crate) fn shared() -> Result<MutexGuard<'static, lumen_gpu::Renderer>, String> {
    match &*GPU {
        Ok(gpu) => Ok(gpu.lock().unwrap_or_else(PoisonError::into_inner)),
        Err(error) => Err(error.clone()),
    }
}

/// The shared device, or `None` after saying why on a machine with no adapter. Panics instead
/// when `LUMEN_REQUIRE_GPU` is `1`, so a run that must exercise the GPU can't pass without it.
pub(crate) fn shared_or_skip(test: &str) -> Option<MutexGuard<'static, lumen_gpu::Renderer>> {
    match shared() {
        Ok(gpu) => Some(gpu),
        Err(error) => {
            assert!(
                std::env::var_os("LUMEN_REQUIRE_GPU").is_none_or(|value| value != "1"),
                "a GPU is required (LUMEN_REQUIRE_GPU=1) for the {test}, but there is none: {error}"
            );
            eprintln!("no GPU ({error}); skipping {test}");
            None
        }
    }
}
