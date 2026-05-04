use crate::gpu::GpuBackend;

pub const BACKEND: GpuBackend = GpuBackend::Cuda;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CudaExternalMemoryHandle {
    OpaqueFd(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CudaExternalSemaphoreHandle {
    OpaqueFd(i32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VulkanToCudaExport {
    pub memory: CudaExternalMemoryHandle,
    pub ready_semaphore: Option<CudaExternalSemaphoreHandle>,
    pub complete_semaphore: Option<CudaExternalSemaphoreHandle>,
    pub allocation_size: u64,
    pub row_pitch: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CudaVideoFrame {
    device_ptr: u64,
    width: u32,
    height: u32,
    pitch: u64,
    pts: Option<i64>,
}

impl CudaVideoFrame {
    pub fn from_device_ptr(
        device_ptr: u64,
        width: u32,
        height: u32,
        pitch: u64,
        pts: Option<i64>,
    ) -> Self {
        Self {
            device_ptr,
            width,
            height,
            pitch,
            pts,
        }
    }

    pub fn device_ptr(&self) -> u64 {
        self.device_ptr
    }

    pub fn pitch(&self) -> u64 {
        self.pitch
    }

    pub fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn pts(&self) -> Option<i64> {
        self.pts
    }

    pub fn backend(&self) -> GpuBackend {
        GpuBackend::Cuda
    }
}

#[cfg(target_os = "linux")]
mod driver {
    use std::{ffi::c_void, mem::MaybeUninit, os::fd::OwnedFd};

    use libloading::Library;

    use super::CudaExternalMemoryHandle;

    type CuResult = i32;
    type CuDevice = i32;
    type CuContext = *mut c_void;
    type CuDevicePtr = u64;
    type CuExternalMemory = *mut c_void;
    type CuMipmappedArray = *mut c_void;
    type CuArray = *mut c_void;

    const CUDA_SUCCESS: CuResult = 0;
    const CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD: u32 = 1;
    const CU_MEMORYTYPE_DEVICE: u32 = 0x02;
    const CU_MEMORYTYPE_ARRAY: u32 = 0x03;
    const CU_AD_FORMAT_UNSIGNED_INT8: u32 = 0x01;
    const CUDA_ARRAY3D_COLOR_ATTACHMENT: u32 = 0x20;

    #[repr(C)]
    union CudaExternalMemoryHandleUnion {
        fd: i32,
        win32: CudaWin32Handle,
        nv_sci_buf_object: *const c_void,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CudaWin32Handle {
        handle: *mut c_void,
        name: *const c_void,
    }

    #[repr(C)]
    struct CudaExternalMemoryHandleDesc {
        type_: u32,
        handle: CudaExternalMemoryHandleUnion,
        size: u64,
        flags: u32,
        reserved: [u32; 16],
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CudaArray3dDescriptor {
        width: usize,
        height: usize,
        depth: usize,
        format: u32,
        num_channels: u32,
        flags: u32,
    }

    #[repr(C)]
    struct CudaExternalMemoryMipmappedArrayDesc {
        offset: u64,
        array_desc: CudaArray3dDescriptor,
        num_levels: u32,
        reserved: [u32; 16],
    }

    #[repr(C)]
    struct CudaMemcpy2d {
        src_x_in_bytes: usize,
        src_y: usize,
        src_memory_type: u32,
        src_host: *const c_void,
        src_device: CuDevicePtr,
        src_array: CuArray,
        src_pitch: usize,
        dst_x_in_bytes: usize,
        dst_y: usize,
        dst_memory_type: u32,
        dst_host: *mut c_void,
        dst_device: CuDevicePtr,
        dst_array: CuArray,
        dst_pitch: usize,
        width_in_bytes: usize,
        height: usize,
    }

    pub struct CudaDriver {
        _library: Library,
        cu_init: unsafe extern "C" fn(u32) -> CuResult,
        cu_device_get: unsafe extern "C" fn(*mut CuDevice, i32) -> CuResult,
        cu_ctx_create: unsafe extern "C" fn(*mut CuContext, u32, CuDevice) -> CuResult,
        cu_ctx_destroy: unsafe extern "C" fn(CuContext) -> CuResult,
        cu_ctx_set_current: unsafe extern "C" fn(CuContext) -> CuResult,
        cu_device_primary_ctx_retain: unsafe extern "C" fn(*mut CuContext, CuDevice) -> CuResult,
        cu_device_primary_ctx_release: unsafe extern "C" fn(CuDevice) -> CuResult,
        cu_mem_alloc_pitch:
            unsafe extern "C" fn(*mut CuDevicePtr, *mut usize, usize, usize, u32) -> CuResult,
        cu_mem_free: unsafe extern "C" fn(CuDevicePtr) -> CuResult,
        cu_memset_d8: unsafe extern "C" fn(CuDevicePtr, u8, usize) -> CuResult,
        cu_memcpy_2d: unsafe extern "C" fn(*const CudaMemcpy2d) -> CuResult,
        cu_import_external_memory: unsafe extern "C" fn(
            *mut CuExternalMemory,
            *const CudaExternalMemoryHandleDesc,
        ) -> CuResult,
        cu_destroy_external_memory: unsafe extern "C" fn(CuExternalMemory) -> CuResult,
        cu_external_memory_get_mapped_mipmapped_array: unsafe extern "C" fn(
            *mut CuMipmappedArray,
            CuExternalMemory,
            *const CudaExternalMemoryMipmappedArrayDesc,
        ) -> CuResult,
        cu_mipmapped_array_get_level:
            unsafe extern "C" fn(*mut CuArray, CuMipmappedArray, u32) -> CuResult,
    }

    unsafe impl Send for CudaDriver {}
    unsafe impl Sync for CudaDriver {}

    impl CudaDriver {
        pub fn load() -> Result<Self, String> {
            let library = unsafe { Library::new("libcuda.so.1") }
                .or_else(|_| unsafe { Library::new("libcuda.so") })
                .map_err(|error| format!("failed to load libcuda: {error}"))?;

            unsafe {
                let cu_init = *library
                    .get::<unsafe extern "C" fn(u32) -> CuResult>(b"cuInit\0")
                    .map_err(|error| format!("failed loading cuInit: {error}"))?;
                let cu_device_get = *library
                    .get::<unsafe extern "C" fn(*mut CuDevice, i32) -> CuResult>(b"cuDeviceGet\0")
                    .map_err(|error| format!("failed loading cuDeviceGet: {error}"))?;
                let cu_ctx_create = *library
                    .get::<unsafe extern "C" fn(*mut CuContext, u32, CuDevice) -> CuResult>(
                        b"cuCtxCreate_v2\0",
                    )
                    .map_err(|error| format!("failed loading cuCtxCreate_v2: {error}"))?;
                let cu_ctx_destroy = *library
                    .get::<unsafe extern "C" fn(CuContext) -> CuResult>(b"cuCtxDestroy_v2\0")
                    .map_err(|error| format!("failed loading cuCtxDestroy_v2: {error}"))?;
                let cu_ctx_set_current = *library
                    .get::<unsafe extern "C" fn(CuContext) -> CuResult>(b"cuCtxSetCurrent\0")
                    .map_err(|error| format!("failed loading cuCtxSetCurrent: {error}"))?;
                let cu_device_primary_ctx_retain = *library
                    .get::<unsafe extern "C" fn(*mut CuContext, CuDevice) -> CuResult>(
                        b"cuDevicePrimaryCtxRetain\0",
                    )
                    .map_err(|error| format!("failed loading cuDevicePrimaryCtxRetain: {error}"))?;
                let cu_device_primary_ctx_release = *library
                    .get::<unsafe extern "C" fn(CuDevice) -> CuResult>(
                        b"cuDevicePrimaryCtxRelease_v2\0",
                    )
                    .or_else(|_| {
                        library.get::<unsafe extern "C" fn(CuDevice) -> CuResult>(
                            b"cuDevicePrimaryCtxRelease\0",
                        )
                    })
                    .map_err(|error| {
                        format!("failed loading cuDevicePrimaryCtxRelease: {error}")
                    })?;
                let cu_mem_alloc_pitch = *library
                    .get::<unsafe extern "C" fn(
                        *mut CuDevicePtr,
                        *mut usize,
                        usize,
                        usize,
                        u32,
                    ) -> CuResult>(b"cuMemAllocPitch_v2\0")
                    .map_err(|error| format!("failed loading cuMemAllocPitch_v2: {error}"))?;
                let cu_mem_free = *library
                    .get::<unsafe extern "C" fn(CuDevicePtr) -> CuResult>(b"cuMemFree_v2\0")
                    .map_err(|error| format!("failed loading cuMemFree_v2: {error}"))?;
                let cu_memset_d8 = *library
                    .get::<unsafe extern "C" fn(CuDevicePtr, u8, usize) -> CuResult>(
                        b"cuMemsetD8_v2\0",
                    )
                    .or_else(|_| {
                        library.get::<unsafe extern "C" fn(CuDevicePtr, u8, usize) -> CuResult>(
                            b"cuMemsetD8\0",
                        )
                    })
                    .map_err(|error| format!("failed loading cuMemsetD8: {error}"))?;
                let cu_memcpy_2d = *library
                    .get::<unsafe extern "C" fn(*const CudaMemcpy2d) -> CuResult>(
                        b"cuMemcpy2D_v2\0",
                    )
                    .map_err(|error| format!("failed loading cuMemcpy2D_v2: {error}"))?;
                let cu_import_external_memory = *library
                    .get::<unsafe extern "C" fn(
                        *mut CuExternalMemory,
                        *const CudaExternalMemoryHandleDesc,
                    ) -> CuResult>(b"cuImportExternalMemory\0")
                    .map_err(|error| format!("failed loading cuImportExternalMemory: {error}"))?;
                let cu_destroy_external_memory = *library
                    .get::<unsafe extern "C" fn(CuExternalMemory) -> CuResult>(
                        b"cuDestroyExternalMemory\0",
                    )
                    .map_err(|error| format!("failed loading cuDestroyExternalMemory: {error}"))?;
                let cu_external_memory_get_mapped_mipmapped_array = *library
                    .get::<unsafe extern "C" fn(
                        *mut CuMipmappedArray,
                        CuExternalMemory,
                        *const CudaExternalMemoryMipmappedArrayDesc,
                    ) -> CuResult>(b"cuExternalMemoryGetMappedMipmappedArray\0")
                    .map_err(|error| {
                        format!("failed loading cuExternalMemoryGetMappedMipmappedArray: {error}")
                    })?;
                let cu_mipmapped_array_get_level = *library
                    .get::<unsafe extern "C" fn(*mut CuArray, CuMipmappedArray, u32) -> CuResult>(
                        b"cuMipmappedArrayGetLevel\0",
                    )
                    .map_err(|error| format!("failed loading cuMipmappedArrayGetLevel: {error}"))?;

                Ok(Self {
                    _library: library,
                    cu_init,
                    cu_device_get,
                    cu_ctx_create,
                    cu_ctx_destroy,
                    cu_ctx_set_current,
                    cu_device_primary_ctx_retain,
                    cu_device_primary_ctx_release,
                    cu_mem_alloc_pitch,
                    cu_mem_free,
                    cu_memset_d8,
                    cu_memcpy_2d,
                    cu_import_external_memory,
                    cu_destroy_external_memory,
                    cu_external_memory_get_mapped_mipmapped_array,
                    cu_mipmapped_array_get_level,
                })
            }
        }

        pub fn create_primary_context(&self) -> Result<CudaContext<'_>, String> {
            check(unsafe { (self.cu_init)(0) }, "cuInit")?;
            let mut device = MaybeUninit::<CuDevice>::uninit();
            check(
                unsafe { (self.cu_device_get)(device.as_mut_ptr(), 0) },
                "cuDeviceGet",
            )?;
            let device = unsafe { device.assume_init() };
            let mut context = MaybeUninit::<CuContext>::uninit();
            check(
                unsafe { (self.cu_device_primary_ctx_retain)(context.as_mut_ptr(), device) },
                "cuDevicePrimaryCtxRetain",
            )?;
            let raw = unsafe { context.assume_init() };
            check(unsafe { (self.cu_ctx_set_current)(raw) }, "cuCtxSetCurrent")?;
            Ok(CudaContext {
                driver: self,
                raw,
                device,
                release_primary: true,
            })
        }

        #[allow(dead_code)]
        pub fn create_context(&self) -> Result<CudaContext<'_>, String> {
            check(unsafe { (self.cu_init)(0) }, "cuInit")?;
            let mut device = MaybeUninit::<CuDevice>::uninit();
            check(
                unsafe { (self.cu_device_get)(device.as_mut_ptr(), 0) },
                "cuDeviceGet",
            )?;
            let device = unsafe { device.assume_init() };
            let mut context = MaybeUninit::<CuContext>::uninit();
            check(
                unsafe { (self.cu_ctx_create)(context.as_mut_ptr(), 0, device) },
                "cuCtxCreate_v2",
            )?;
            Ok(CudaContext {
                driver: self,
                raw: unsafe { context.assume_init() },
                device,
                release_primary: false,
            })
        }

        pub fn allocate_rgba_frame(
            &self,
            width: u32,
            height: u32,
        ) -> Result<CudaDeviceAllocation<'_>, String> {
            let width_bytes = width as usize * 4;
            let mut device_ptr = MaybeUninit::<CuDevicePtr>::uninit();
            let mut pitch = MaybeUninit::<usize>::uninit();
            check(
                unsafe {
                    (self.cu_mem_alloc_pitch)(
                        device_ptr.as_mut_ptr(),
                        pitch.as_mut_ptr(),
                        width_bytes,
                        height as usize,
                        16,
                    )
                },
                "cuMemAllocPitch_v2",
            )?;
            let device_ptr = unsafe { device_ptr.assume_init() };
            let pitch = unsafe { pitch.assume_init() };
            Ok(CudaDeviceAllocation {
                driver: self,
                device_ptr,
                width,
                height,
                pitch,
            })
        }

        pub fn copy_image_to_rgba_frame(
            &self,
            source: &ImportedCudaExternalImage<'_>,
            destination: &CudaDeviceAllocation<'_>,
        ) -> Result<(), String> {
            let copy = CudaMemcpy2d {
                src_x_in_bytes: 0,
                src_y: 0,
                src_memory_type: CU_MEMORYTYPE_ARRAY,
                src_host: std::ptr::null(),
                src_device: 0,
                src_array: source.level_zero,
                src_pitch: 0,
                dst_x_in_bytes: 0,
                dst_y: 0,
                dst_memory_type: CU_MEMORYTYPE_DEVICE,
                dst_host: std::ptr::null_mut(),
                dst_device: destination.device_ptr,
                dst_array: std::ptr::null_mut(),
                dst_pitch: destination.pitch,
                width_in_bytes: destination.width as usize * 4,
                height: destination.height as usize,
            };
            check(unsafe { (self.cu_memcpy_2d)(&copy) }, "cuMemcpy2D_v2")
        }
    }

    pub struct CudaContext<'a> {
        driver: &'a CudaDriver,
        raw: CuContext,
        device: CuDevice,
        release_primary: bool,
    }

    impl CudaContext<'_> {
        pub fn set_current(&self) -> Result<(), String> {
            check(
                unsafe { (self.driver.cu_ctx_set_current)(self.raw) },
                "cuCtxSetCurrent",
            )
        }
    }

    impl Drop for CudaContext<'_> {
        fn drop(&mut self) {
            if self.release_primary {
                let _ = unsafe { (self.driver.cu_device_primary_ctx_release)(self.device) };
            } else {
                let _ = unsafe { (self.driver.cu_ctx_destroy)(self.raw) };
            }
        }
    }

    pub struct CudaDeviceAllocation<'a> {
        driver: &'a CudaDriver,
        device_ptr: CuDevicePtr,
        width: u32,
        height: u32,
        pitch: usize,
    }

    impl CudaDeviceAllocation<'_> {
        pub fn as_video_frame(&self, pts: Option<i64>) -> super::CudaVideoFrame {
            super::CudaVideoFrame::from_device_ptr(
                self.device_ptr,
                self.width,
                self.height,
                self.pitch as u64,
                pts,
            )
        }

        pub fn clear(&self, value: u8) -> Result<(), String> {
            check(
                unsafe {
                    (self.driver.cu_memset_d8)(
                        self.device_ptr,
                        value,
                        self.pitch.saturating_mul(self.height as usize),
                    )
                },
                "cuMemsetD8",
            )
        }
    }

    impl Drop for CudaDeviceAllocation<'_> {
        fn drop(&mut self) {
            let _ = unsafe { (self.driver.cu_mem_free)(self.device_ptr) };
        }
    }

    pub struct ImportedCudaExternalImage<'a> {
        driver: &'a CudaDriver,
        external_memory: CuExternalMemory,
        mipmapped_array: CuMipmappedArray,
        level_zero: CuArray,
    }

    impl ImportedCudaExternalImage<'_> {
        pub fn mipmapped_array_raw(&self) -> usize {
            self.mipmapped_array as usize
        }

        pub fn level_zero_raw(&self) -> usize {
            self.level_zero as usize
        }
    }

    impl Drop for ImportedCudaExternalImage<'_> {
        fn drop(&mut self) {
            let _ = unsafe { (self.driver.cu_destroy_external_memory)(self.external_memory) };
        }
    }

    pub fn import_vulkan_opaque_fd_image<'a>(
        driver: &'a CudaDriver,
        memory: CudaExternalMemoryHandle,
        allocation_size: u64,
        width: u32,
        height: u32,
    ) -> Result<ImportedCudaExternalImage<'a>, String> {
        let CudaExternalMemoryHandle::OpaqueFd(fd) = memory;
        let handle_desc = CudaExternalMemoryHandleDesc {
            type_: CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD,
            handle: CudaExternalMemoryHandleUnion { fd },
            size: allocation_size,
            flags: 0,
            reserved: [0; 16],
        };
        let mut external_memory = MaybeUninit::<CuExternalMemory>::uninit();
        check(
            unsafe {
                (driver.cu_import_external_memory)(external_memory.as_mut_ptr(), &handle_desc)
            },
            "cuImportExternalMemory",
        )?;
        let external_memory = unsafe { external_memory.assume_init() };
        let array_desc = CudaExternalMemoryMipmappedArrayDesc {
            offset: 0,
            array_desc: CudaArray3dDescriptor {
                width: width as usize,
                height: height as usize,
                depth: 0,
                format: CU_AD_FORMAT_UNSIGNED_INT8,
                num_channels: 4,
                flags: CUDA_ARRAY3D_COLOR_ATTACHMENT,
            },
            num_levels: 1,
            reserved: [0; 16],
        };
        let mut mipmapped_array = MaybeUninit::<CuMipmappedArray>::uninit();
        if let Err(error) = check(
            unsafe {
                (driver.cu_external_memory_get_mapped_mipmapped_array)(
                    mipmapped_array.as_mut_ptr(),
                    external_memory,
                    &array_desc,
                )
            },
            "cuExternalMemoryGetMappedMipmappedArray",
        ) {
            let _ = unsafe { (driver.cu_destroy_external_memory)(external_memory) };
            return Err(error);
        }
        let mipmapped_array = unsafe { mipmapped_array.assume_init() };
        let mut level_zero = MaybeUninit::<CuArray>::uninit();
        if let Err(error) = check(
            unsafe {
                (driver.cu_mipmapped_array_get_level)(level_zero.as_mut_ptr(), mipmapped_array, 0)
            },
            "cuMipmappedArrayGetLevel",
        ) {
            let _ = unsafe { (driver.cu_destroy_external_memory)(external_memory) };
            return Err(error);
        }
        Ok(ImportedCudaExternalImage {
            driver,
            external_memory,
            mipmapped_array,
            level_zero: unsafe { level_zero.assume_init() },
        })
    }

    pub fn import_owned_vulkan_opaque_fd_image<'a>(
        driver: &'a CudaDriver,
        fd: OwnedFd,
        allocation_size: u64,
        width: u32,
        height: u32,
    ) -> Result<ImportedCudaExternalImage<'a>, String> {
        use std::os::fd::IntoRawFd;

        import_vulkan_opaque_fd_image(
            driver,
            CudaExternalMemoryHandle::OpaqueFd(fd.into_raw_fd()),
            allocation_size,
            width,
            height,
        )
    }

    fn check(result: CuResult, operation: &str) -> Result<(), String> {
        if result == CUDA_SUCCESS {
            Ok(())
        } else {
            Err(format!("{operation} failed with CUDA result {result}"))
        }
    }
}

#[cfg(target_os = "linux")]
pub use driver::{
    CudaContext, CudaDeviceAllocation, CudaDriver, ImportedCudaExternalImage,
    import_owned_vulkan_opaque_fd_image, import_vulkan_opaque_fd_image,
};
