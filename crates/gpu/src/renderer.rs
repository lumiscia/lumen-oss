use std::{env, sync::Arc};

use anyhow::{Context, Result};
#[cfg(not(target_arch = "wasm32"))]
use anyhow::{anyhow, bail};

#[cfg(not(target_arch = "wasm32"))]
use crate::Size;

pub struct Renderer {
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    adapter_info: Option<wgpu::AdapterInfo>,
}

#[derive(Debug, Clone)]
pub struct ExternalTexture {
    texture: Arc<wgpu::Texture>,
}

impl ExternalTexture {
    /// Wraps `texture` as a render target.
    pub fn new(texture: Arc<wgpu::Texture>) -> Self {
        Self { texture }
    }

    pub fn from_texture(texture: wgpu::Texture) -> Self {
        Self::new(Arc::new(texture))
    }

    pub fn texture_arc(&self) -> Arc<wgpu::Texture> {
        Arc::clone(&self.texture)
    }
}

#[derive(Debug, Clone)]
pub struct SubmittedExternalTexture {
    submission: wgpu::SubmissionIndex,
    texture: Arc<wgpu::Texture>,
}

impl SubmittedExternalTexture {
    pub fn new(submission: wgpu::SubmissionIndex, texture: Arc<wgpu::Texture>) -> Self {
        Self {
            submission,
            texture,
        }
    }

    pub fn texture(&self) -> &wgpu::Texture {
        &self.texture
    }

    pub fn wait(&self, device: &wgpu::Device) -> Result<()> {
        device
            .poll(wgpu::PollType::Wait {
                submission_index: Some(self.submission.clone()),
                timeout: None,
            })
            .context("wait for external texture submission")?;
        Ok(())
    }
}

/// Builds the instance options from the build config and environment. HAL labels are discarded
/// unless `WGPU_DISCARD_HAL_LABELS` says otherwise: concurrent devices from separate instances
/// crash inside the Vulkan loader in `vkSetDebugUtilsObjectNameEXT` more often with them
/// (https://github.com/gfx-rs/wgpu/issues/5270). Set `WGPU_DISCARD_HAL_LABELS=0` to keep
/// resource names in GPU captures.
pub(crate) fn instance_descriptor() -> wgpu::InstanceDescriptor {
    let mut descriptor = wgpu::InstanceDescriptor::new_without_display_handle_from_env();
    if std::env::var_os("WGPU_DISCARD_HAL_LABELS").is_none() {
        descriptor.flags |= wgpu::InstanceFlags::DISCARD_HAL_LABELS;
    }
    descriptor
}

impl Renderer {
    pub async fn new() -> Result<Self> {
        let instance = wgpu::Instance::new(instance_descriptor());
        let power_preference =
            wgpu::PowerPreference::from_env().unwrap_or(wgpu::PowerPreference::HighPerformance);
        let force_fallback_adapter = env_flag("LUMEN_GPU_FORCE_FALLBACK_ADAPTER");
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference,
                compatible_surface: None,
                force_fallback_adapter,
                apply_limit_buckets: false,
            })
            .await
            .context("no compatible wgpu adapter")?;
        let adapter_info = adapter.get_info();
        tracing::info!(
            target: "lumen_gpu",
            adapter = %adapter_info.name,
            backend = ?adapter_info.backend,
            device_type = ?adapter_info.device_type,
            vendor = adapter_info.vendor,
            device = adapter_info.device,
            driver = %adapter_info.driver,
            driver_info = %adapter_info.driver_info,
            "selected wgpu adapter"
        );
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                required_limits: adapter.limits(),
                ..Default::default()
            })
            .await
            .context("create wgpu device")?;
        Ok(Self::from_device_with_adapter_info(
            device,
            queue,
            Some(adapter_info),
        ))
    }

    pub fn from_device(device: wgpu::Device, queue: wgpu::Queue) -> Self {
        Self::from_device_with_adapter_info(device, queue, None)
    }

    pub fn from_device_with_adapter_info(
        device: wgpu::Device,
        queue: wgpu::Queue,
        adapter_info: Option<wgpu::AdapterInfo>,
    ) -> Self {
        Self {
            device,
            queue,
            adapter_info,
        }
    }

    /// `None` for a device created outside this crate.
    pub fn adapter_info(&self) -> Option<&wgpu::AdapterInfo> {
        self.adapter_info.as_ref()
    }

    /// Copies the `size` texels at the origin of an `Rgba8` `texture` to the CPU, blocking until
    /// the copy completes. Rows are tightly packed.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn read_texture_rgba8(&self, texture: &wgpu::Texture, size: Size) -> Result<Vec<u8>> {
        let row = size.width as usize * 4;
        let padded_row = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
        let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("lumen readback"),
            size: (padded_row * size.height as usize).max(1) as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen readback encoder"),
            });
        encoder.copy_texture_to_buffer(
            texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &buffer,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(padded_row as u32),
                    rows_per_image: Some(size.height),
                },
            },
            size.as_extent(),
        );
        self.queue.submit([encoder.finish()]);

        let (sender, receiver) = std::sync::mpsc::channel();
        buffer
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                let _ = sender.send(result);
            });
        self.device
            .poll(wgpu::PollType::wait_indefinitely())
            .context("GPU readback poll failed")?;
        receiver
            .recv()
            .map_err(|_| anyhow!("GPU readback callback closed"))?
            .context("GPU readback map failed")?;
        let mapped = buffer
            .slice(..)
            .get_mapped_range()
            .context("GPU readback map range failed")?;
        let mut pixels = Vec::with_capacity(row * size.height as usize);
        for padded in mapped.chunks(padded_row) {
            pixels.extend_from_slice(&padded[..row]);
        }
        Ok(pixels)
    }

    /// Copies all of `source` to the origin of `destination`.
    pub fn copy_texture_to_external(
        &self,
        source: &wgpu::Texture,
        destination: &wgpu::Texture,
    ) -> wgpu::SubmissionIndex {
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen-gpu external texture copy"),
            });
        encoder.copy_texture_to_texture(
            source.as_image_copy(),
            destination.as_image_copy(),
            source.size(),
        );
        self.queue.submit([encoder.finish()])
    }
}

/// Captures wgpu validation errors raised on this thread, which native wgpu would otherwise panic on.
///
/// Browser WebGPU only resolves error scopes asynchronously, so there the scope is not pushed and the
/// browser reports errors as uncaptured.
pub(crate) struct ValidationScope {
    #[cfg(not(target_arch = "wasm32"))]
    guard: wgpu::ErrorScopeGuard,
}

impl ValidationScope {
    #[cfg_attr(target_arch = "wasm32", allow(unused_variables))]
    pub(crate) fn push(device: &wgpu::Device) -> Self {
        Self {
            #[cfg(not(target_arch = "wasm32"))]
            guard: device.push_error_scope(wgpu::ErrorFilter::Validation),
        }
    }

    pub(crate) fn finish(self) -> Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            use std::{
                future::Future,
                task::{Context, Poll, Waker},
            };
            // wgpu-core resolves the pop immediately.
            let mut pop = std::pin::pin!(self.guard.pop());
            if let Poll::Ready(Some(error)) =
                pop.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            {
                bail!("GPU validation failed: {error}");
            }
        }
        Ok(())
    }
}

pub(crate) fn env_flag(name: &str) -> bool {
    env::var(name)
        .map(|value| {
            matches!(
                value.to_ascii_lowercase().as_str(),
                "1" | "true" | "yes" | "on"
            )
        })
        .unwrap_or(false)
}
