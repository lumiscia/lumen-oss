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
