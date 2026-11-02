---
"@lumiscia/lumen-bindings": minor
---

Failed audio setup and failed file reads no longer leak FFmpeg codec contexts, resamplers or format contexts, and a failure to close the output file now fails the render instead of being ignored. On multi-GPU machines, `lumen-server` and `lumen-bench` now run NVENC on the GPU that rendered the frames, and CUDA interop frees its GPU resources in the right order and context, where it used to leak a mapped array on every NVENC render.

For Rust: `CudaContext` and `CudaDeviceAllocation` no longer borrow a `CudaDriver`. Allocation, copy, synchronize and converter creation are methods on `CudaContext`, and `import_owned_vulkan_opaque_fd_image` takes a `&CudaContext`. `import_vulkan_opaque_fd_image`, `CudaExternalMemoryHandle`, `CudaExternalSemaphoreHandle` and `VulkanToCudaExport` are removed, and `VideoEncoderConfig` gains `cuda_ordinal`.
