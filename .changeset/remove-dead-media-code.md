---
"@lumiscia/lumen-bindings": minor
---

Remove unused Rust items from the media crates: `CudaVideoFrame::from_device_ptr` (build CUDA frames with `CudaDeviceAllocation::as_video_frame`), `MetalPixelBufferFrame::retain_pixel_buffer`, `VulkanVideoFrame::nv12_pitch`, `nv12_chroma_offset` and `nv12_size` (use `VulkanFrameLayout::nv12`), `CudaDriver::driver_version` and the engine's `duration_samples`.
