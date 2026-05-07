use crate::ffi::AvFrame;
use crate::gpu::GpuBackend;
use crate::video::PixelFormat;

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

pub struct CudaDecodedFrame {
    _frame: AvFrame,
    device_ptr: u64,
    width: u32,
    height: u32,
    pitch: u64,
    pixel_format: PixelFormat,
    pts: Option<i64>,
}

// CUDA decoded frames retain their FFmpeg frame reference, and the exposed device pointer is only
// used by CUDA operations after a context is made current on the consuming thread.
unsafe impl Send for CudaDecodedFrame {}
unsafe impl Sync for CudaDecodedFrame {}

impl std::fmt::Debug for CudaDecodedFrame {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("CudaDecodedFrame")
            .field("device_ptr", &self.device_ptr)
            .field("width", &self.width)
            .field("height", &self.height)
            .field("pitch", &self.pitch)
            .field("pixel_format", &self.pixel_format)
            .field("pts", &self.pts)
            .finish_non_exhaustive()
    }
}

impl CudaDecodedFrame {
    pub(crate) fn from_av_frame(
        frame: AvFrame,
        device_ptr: u64,
        width: u32,
        height: u32,
        pitch: u64,
        pixel_format: PixelFormat,
        pts: Option<i64>,
    ) -> Self {
        Self {
            _frame: frame,
            device_ptr,
            width,
            height,
            pitch,
            pixel_format,
            pts,
        }
    }

    pub fn device_ptr(&self) -> u64 {
        self.device_ptr
    }

    pub fn pitch(&self) -> u64 {
        self.pitch
    }

    pub fn pixel_format(&self) -> PixelFormat {
        self.pixel_format
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
    use std::{
        ffi::{CStr, c_char, c_void},
        mem::MaybeUninit,
        os::fd::OwnedFd,
    };

    use libloading::Library;

    use super::CudaExternalMemoryHandle;

    type CuResult = i32;
    type CuDevice = i32;
    type CuContext = *mut c_void;
    type CuDevicePtr = u64;
    type CuExternalMemory = *mut c_void;
    type CuMipmappedArray = *mut c_void;
    type CuArray = *mut c_void;
    type CuModule = *mut c_void;
    type CuFunction = *mut c_void;

    const CUDA_SUCCESS: CuResult = 0;
    const CU_EXTERNAL_MEMORY_HANDLE_TYPE_OPAQUE_FD: u32 = 1;
    const CU_MEMORYTYPE_DEVICE: u32 = 0x02;
    const CU_MEMORYTYPE_ARRAY: u32 = 0x03;
    const CU_AD_FORMAT_UNSIGNED_INT8: u32 = 0x01;
    const CUDA_ARRAY3D_COLOR_ATTACHMENT: u32 = 0x20;
    const NV12_TO_RGBA8_PTX: &str = concat!(
        r#"
.version 6.4
.target sm_50
.address_size 64

.visible .entry nv12_to_rgba8(
    .param .u64 src,
    .param .u64 dst,
    .param .u32 src_pitch,
    .param .u32 dst_pitch,
    .param .u32 width,
    .param .u32 height
)
{
    .reg .pred %p<6>;
    .reg .b32 %r<80>;
    .reg .b64 %rd<24>;

    ld.param.u64 %rd1, [src];
    ld.param.u64 %rd2, [dst];
    ld.param.u32 %r1, [src_pitch];
    ld.param.u32 %r2, [dst_pitch];
    ld.param.u32 %r3, [width];
    ld.param.u32 %r4, [height];

    mov.u32 %r5, %ctaid.x;
    mov.u32 %r6, %ntid.x;
    mov.u32 %r7, %tid.x;
    mad.lo.u32 %r8, %r5, %r6, %r7;
    mov.u32 %r9, %ctaid.y;
    mov.u32 %r10, %ntid.y;
    mov.u32 %r11, %tid.y;
    mad.lo.u32 %r12, %r9, %r10, %r11;

    setp.ge.u32 %p1, %r8, %r3;
    setp.ge.u32 %p2, %r12, %r4;
    or.pred %p3, %p1, %p2;
    @%p3 ret;

    mul.lo.u32 %r13, %r12, %r1;
    add.u32 %r14, %r13, %r8;
    cvt.u64.u32 %rd3, %r14;
    add.u64 %rd4, %rd1, %rd3;
    ld.global.u8 %r15, [%rd4];

    mul.lo.u32 %r16, %r1, %r4;
    shr.u32 %r17, %r12, 1;
    mul.lo.u32 %r18, %r17, %r1;
    and.b32 %r19, %r8, 4294967294;
    add.u32 %r20, %r16, %r18;
    add.u32 %r21, %r20, %r19;
    cvt.u64.u32 %rd5, %r21;
    add.u64 %rd6, %rd1, %rd5;
    ld.global.u8 %r22, [%rd6];
    add.u64 %rd7, %rd6, 1;
    ld.global.u8 %r23, [%rd7];

    sub.s32 %r24, %r15, 16;
    max.s32 %r24, %r24, 0;
    sub.s32 %r25, %r22, 128;
    sub.s32 %r26, %r23, 128;

    mul.lo.s32 %r27, %r24, 298;
    mul.lo.s32 %r28, %r26, 409;
    add.s32 %r29, %r27, %r28;
    add.s32 %r29, %r29, 128;
    shr.s32 %r30, %r29, 8;

    mul.lo.s32 %r31, %r25, 100;
    mul.lo.s32 %r32, %r26, 208;
    sub.s32 %r33, %r27, %r31;
    sub.s32 %r33, %r33, %r32;
    add.s32 %r33, %r33, 128;
    shr.s32 %r34, %r33, 8;

    mul.lo.s32 %r35, %r25, 516;
    add.s32 %r36, %r27, %r35;
    add.s32 %r36, %r36, 128;
    shr.s32 %r37, %r36, 8;

    max.s32 %r30, %r30, 0;
    min.s32 %r30, %r30, 255;
    max.s32 %r34, %r34, 0;
    min.s32 %r34, %r34, 255;
    max.s32 %r37, %r37, 0;
    min.s32 %r37, %r37, 255;

    shl.b32 %r38, %r34, 8;
    shl.b32 %r39, %r37, 16;
    or.b32 %r40, %r30, %r38;
    or.b32 %r40, %r40, %r39;
    or.b32 %r40, %r40, 4278190080;

    mul.lo.u32 %r41, %r12, %r2;
    shl.b32 %r42, %r8, 2;
    add.u32 %r43, %r41, %r42;
    cvt.u64.u32 %rd8, %r43;
    add.u64 %rd9, %rd2, %rd8;
    st.global.u32 [%rd9], %r40;
    ret;
}
"#,
        "\0"
    );

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct CuUuid {
        bytes: [c_char; 16],
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct CudaDeviceInfo {
        pub ordinal: i32,
        pub name: String,
        pub uuid: [u8; 16],
        pub pci_bus_id: String,
    }

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
        cu_driver_get_version: unsafe extern "C" fn(*mut i32) -> CuResult,
        cu_get_error_name: unsafe extern "C" fn(CuResult, *mut *const c_char) -> CuResult,
        cu_get_error_string: unsafe extern "C" fn(CuResult, *mut *const c_char) -> CuResult,
        cu_device_get_count: unsafe extern "C" fn(*mut i32) -> CuResult,
        cu_device_get: unsafe extern "C" fn(*mut CuDevice, i32) -> CuResult,
        cu_device_get_name: unsafe extern "C" fn(*mut c_char, i32, CuDevice) -> CuResult,
        cu_device_get_uuid: unsafe extern "C" fn(*mut CuUuid, CuDevice) -> CuResult,
        cu_device_get_pci_bus_id: unsafe extern "C" fn(*mut c_char, i32, CuDevice) -> CuResult,
        cu_ctx_create: unsafe extern "C" fn(*mut CuContext, u32, CuDevice) -> CuResult,
        cu_ctx_destroy: unsafe extern "C" fn(CuContext) -> CuResult,
        cu_ctx_set_current: unsafe extern "C" fn(CuContext) -> CuResult,
        cu_ctx_synchronize: unsafe extern "C" fn() -> CuResult,
        cu_device_primary_ctx_retain: unsafe extern "C" fn(*mut CuContext, CuDevice) -> CuResult,
        cu_device_primary_ctx_release: unsafe extern "C" fn(CuDevice) -> CuResult,
        cu_mem_alloc_pitch:
            unsafe extern "C" fn(*mut CuDevicePtr, *mut usize, usize, usize, u32) -> CuResult,
        cu_mem_free: unsafe extern "C" fn(CuDevicePtr) -> CuResult,
        cu_memset_d8: unsafe extern "C" fn(CuDevicePtr, u8, usize) -> CuResult,
        cu_memcpy_2d: unsafe extern "C" fn(*const CudaMemcpy2d) -> CuResult,
        cu_module_load_data: unsafe extern "C" fn(*mut CuModule, *const c_void) -> CuResult,
        cu_module_get_function:
            unsafe extern "C" fn(*mut CuFunction, CuModule, *const c_char) -> CuResult,
        cu_module_unload: unsafe extern "C" fn(CuModule) -> CuResult,
        cu_launch_kernel: unsafe extern "C" fn(
            CuFunction,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            u32,
            *mut c_void,
            *mut *mut c_void,
            *mut *mut c_void,
        ) -> CuResult,
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
                let cu_driver_get_version = *library
                    .get::<unsafe extern "C" fn(*mut i32) -> CuResult>(b"cuDriverGetVersion\0")
                    .map_err(|error| format!("failed loading cuDriverGetVersion: {error}"))?;
                let cu_get_error_name = *library
                    .get::<unsafe extern "C" fn(CuResult, *mut *const c_char) -> CuResult>(
                        b"cuGetErrorName\0",
                    )
                    .map_err(|error| format!("failed loading cuGetErrorName: {error}"))?;
                let cu_get_error_string = *library
                    .get::<unsafe extern "C" fn(CuResult, *mut *const c_char) -> CuResult>(
                        b"cuGetErrorString\0",
                    )
                    .map_err(|error| format!("failed loading cuGetErrorString: {error}"))?;
                let cu_device_get_count = *library
                    .get::<unsafe extern "C" fn(*mut i32) -> CuResult>(b"cuDeviceGetCount\0")
                    .map_err(|error| format!("failed loading cuDeviceGetCount: {error}"))?;
                let cu_device_get = *library
                    .get::<unsafe extern "C" fn(*mut CuDevice, i32) -> CuResult>(b"cuDeviceGet\0")
                    .map_err(|error| format!("failed loading cuDeviceGet: {error}"))?;
                let cu_device_get_name = *library
                    .get::<unsafe extern "C" fn(*mut c_char, i32, CuDevice) -> CuResult>(
                        b"cuDeviceGetName\0",
                    )
                    .map_err(|error| format!("failed loading cuDeviceGetName: {error}"))?;
                let cu_device_get_uuid = *library
                    .get::<unsafe extern "C" fn(*mut CuUuid, CuDevice) -> CuResult>(
                        b"cuDeviceGetUuid\0",
                    )
                    .map_err(|error| format!("failed loading cuDeviceGetUuid: {error}"))?;
                let cu_device_get_pci_bus_id = *library
                    .get::<unsafe extern "C" fn(*mut c_char, i32, CuDevice) -> CuResult>(
                        b"cuDeviceGetPCIBusId\0",
                    )
                    .map_err(|error| format!("failed loading cuDeviceGetPCIBusId: {error}"))?;
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
                let cu_ctx_synchronize = *library
                    .get::<unsafe extern "C" fn() -> CuResult>(b"cuCtxSynchronize\0")
                    .map_err(|error| format!("failed loading cuCtxSynchronize: {error}"))?;
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
                let cu_module_load_data = *library
                    .get::<unsafe extern "C" fn(*mut CuModule, *const c_void) -> CuResult>(
                        b"cuModuleLoadData\0",
                    )
                    .map_err(|error| format!("failed loading cuModuleLoadData: {error}"))?;
                let cu_module_get_function = *library
                    .get::<unsafe extern "C" fn(
                        *mut CuFunction,
                        CuModule,
                        *const c_char,
                    ) -> CuResult>(b"cuModuleGetFunction\0")
                    .map_err(|error| format!("failed loading cuModuleGetFunction: {error}"))?;
                let cu_module_unload = *library
                    .get::<unsafe extern "C" fn(CuModule) -> CuResult>(b"cuModuleUnload\0")
                    .map_err(|error| format!("failed loading cuModuleUnload: {error}"))?;
                let cu_launch_kernel = *library
                    .get::<unsafe extern "C" fn(
                        CuFunction,
                        u32,
                        u32,
                        u32,
                        u32,
                        u32,
                        u32,
                        u32,
                        *mut c_void,
                        *mut *mut c_void,
                        *mut *mut c_void,
                    ) -> CuResult>(b"cuLaunchKernel\0")
                    .map_err(|error| format!("failed loading cuLaunchKernel: {error}"))?;
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
                    cu_driver_get_version,
                    cu_get_error_name,
                    cu_get_error_string,
                    cu_device_get_count,
                    cu_device_get,
                    cu_device_get_name,
                    cu_device_get_uuid,
                    cu_device_get_pci_bus_id,
                    cu_ctx_create,
                    cu_ctx_destroy,
                    cu_ctx_set_current,
                    cu_ctx_synchronize,
                    cu_device_primary_ctx_retain,
                    cu_device_primary_ctx_release,
                    cu_mem_alloc_pitch,
                    cu_mem_free,
                    cu_memset_d8,
                    cu_memcpy_2d,
                    cu_module_load_data,
                    cu_module_get_function,
                    cu_module_unload,
                    cu_launch_kernel,
                    cu_import_external_memory,
                    cu_destroy_external_memory,
                    cu_external_memory_get_mapped_mipmapped_array,
                    cu_mipmapped_array_get_level,
                })
            }
        }

        pub fn create_primary_context(&self) -> Result<CudaContext<'_>, String> {
            self.create_primary_context_for_ordinal(0)
        }

        pub fn create_primary_context_for_ordinal(
            &self,
            ordinal: i32,
        ) -> Result<CudaContext<'_>, String> {
            check(unsafe { (self.cu_init)(0) }, "cuInit")?;
            let mut device = MaybeUninit::<CuDevice>::uninit();
            check(
                unsafe { (self.cu_device_get)(device.as_mut_ptr(), ordinal) },
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
                ordinal,
                release_primary: true,
            })
        }

        pub fn driver_version(&self) -> Result<i32, String> {
            check(unsafe { (self.cu_init)(0) }, "cuInit")?;
            let mut version = MaybeUninit::<i32>::uninit();
            check(
                unsafe { (self.cu_driver_get_version)(version.as_mut_ptr()) },
                "cuDriverGetVersion",
            )?;
            Ok(unsafe { version.assume_init() })
        }

        pub fn devices(&self) -> Result<Vec<CudaDeviceInfo>, String> {
            check(unsafe { (self.cu_init)(0) }, "cuInit")?;
            let mut count = MaybeUninit::<i32>::uninit();
            check(
                unsafe { (self.cu_device_get_count)(count.as_mut_ptr()) },
                "cuDeviceGetCount",
            )?;
            let count = unsafe { count.assume_init() };
            let mut devices = Vec::new();
            for ordinal in 0..count {
                let mut device = MaybeUninit::<CuDevice>::uninit();
                check(
                    unsafe { (self.cu_device_get)(device.as_mut_ptr(), ordinal) },
                    "cuDeviceGet",
                )?;
                let device = unsafe { device.assume_init() };

                let mut name = [0 as c_char; 256];
                check(
                    unsafe {
                        (self.cu_device_get_name)(name.as_mut_ptr(), name.len() as i32, device)
                    },
                    "cuDeviceGetName",
                )?;

                let mut uuid = MaybeUninit::<CuUuid>::uninit();
                check(
                    unsafe { (self.cu_device_get_uuid)(uuid.as_mut_ptr(), device) },
                    "cuDeviceGetUuid",
                )?;
                let uuid = unsafe { uuid.assume_init() };

                let mut pci_bus_id = [0 as c_char; 64];
                check(
                    unsafe {
                        (self.cu_device_get_pci_bus_id)(
                            pci_bus_id.as_mut_ptr(),
                            pci_bus_id.len() as i32,
                            device,
                        )
                    },
                    "cuDeviceGetPCIBusId",
                )?;

                devices.push(CudaDeviceInfo {
                    ordinal,
                    name: unsafe { CStr::from_ptr(name.as_ptr()) }
                        .to_string_lossy()
                        .into_owned(),
                    uuid: uuid.bytes.map(|byte| byte as u8),
                    pci_bus_id: unsafe { CStr::from_ptr(pci_bus_id.as_ptr()) }
                        .to_string_lossy()
                        .into_owned(),
                });
            }
            Ok(devices)
        }

        pub fn describe_error(&self, result: CuResult) -> String {
            let mut name = std::ptr::null();
            let mut description = std::ptr::null();
            let name = if unsafe { (self.cu_get_error_name)(result, &mut name) } == CUDA_SUCCESS
                && !name.is_null()
            {
                unsafe { CStr::from_ptr(name) }
                    .to_string_lossy()
                    .into_owned()
            } else {
                "UNKNOWN_CUDA_ERROR".to_string()
            };
            let description = if unsafe { (self.cu_get_error_string)(result, &mut description) }
                == CUDA_SUCCESS
                && !description.is_null()
            {
                unsafe { CStr::from_ptr(description) }
                    .to_string_lossy()
                    .into_owned()
            } else {
                "no CUDA error description available".to_string()
            };
            format!("{name} ({result}): {description}")
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
                ordinal: 0,
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

        pub fn copy_rgba_frame_to_image(
            &self,
            source: &CudaDeviceAllocation<'_>,
            destination: &ImportedCudaExternalImage<'_>,
        ) -> Result<(), String> {
            let copy = CudaMemcpy2d {
                src_x_in_bytes: 0,
                src_y: 0,
                src_memory_type: CU_MEMORYTYPE_DEVICE,
                src_host: std::ptr::null(),
                src_device: source.device_ptr,
                src_array: std::ptr::null_mut(),
                src_pitch: source.pitch,
                dst_x_in_bytes: 0,
                dst_y: 0,
                dst_memory_type: CU_MEMORYTYPE_ARRAY,
                dst_host: std::ptr::null_mut(),
                dst_device: 0,
                dst_array: destination.level_zero,
                dst_pitch: 0,
                width_in_bytes: source.width as usize * 4,
                height: source.height as usize,
            };
            check(unsafe { (self.cu_memcpy_2d)(&copy) }, "cuMemcpy2D_v2")
        }

        pub fn synchronize_context(&self) -> Result<(), String> {
            check(unsafe { (self.cu_ctx_synchronize)() }, "cuCtxSynchronize")
        }

        pub fn create_nv12_to_rgba_converter(&self) -> Result<CudaNv12ToRgbaConverter<'_>, String> {
            let mut module = MaybeUninit::<CuModule>::uninit();
            let result = unsafe {
                (self.cu_module_load_data)(
                    module.as_mut_ptr(),
                    NV12_TO_RGBA8_PTX.as_ptr().cast::<c_void>(),
                )
            };
            if result != CUDA_SUCCESS {
                return Err(format!(
                    "cuModuleLoadData for NV12 conversion failed with {}",
                    self.describe_error(result)
                ));
            }
            let module = unsafe { module.assume_init() };
            let mut function = MaybeUninit::<CuFunction>::uninit();
            let result = unsafe {
                (self.cu_module_get_function)(
                    function.as_mut_ptr(),
                    module,
                    c"nv12_to_rgba8".as_ptr(),
                )
            };
            if result != CUDA_SUCCESS {
                let _ = unsafe { (self.cu_module_unload)(module) };
                return Err(format!(
                    "cuModuleGetFunction for NV12 conversion failed with {}",
                    self.describe_error(result)
                ));
            }
            Ok(CudaNv12ToRgbaConverter {
                driver: self,
                module,
                function: unsafe { function.assume_init() },
            })
        }
    }

    pub struct CudaNv12ToRgbaConverter<'a> {
        driver: &'a CudaDriver,
        module: CuModule,
        function: CuFunction,
    }

    impl CudaNv12ToRgbaConverter<'_> {
        pub fn convert(
            &self,
            source: &super::CudaDecodedFrame,
            destination: &CudaDeviceAllocation<'_>,
        ) -> Result<(), String> {
            if source.pixel_format() != crate::video::PixelFormat::Nv12 {
                return Err(format!(
                    "CUDA RGBA conversion currently supports NV12 only, got {:?}",
                    source.pixel_format()
                ));
            }
            if source.dimensions() != destination.dimensions() {
                return Err(format!(
                    "CUDA RGBA conversion size mismatch: source {:?}, destination {:?}",
                    source.dimensions(),
                    destination.dimensions()
                ));
            }

            let mut src = source.device_ptr();
            let mut dst = destination.device_ptr;
            let mut src_pitch = source.pitch() as u32;
            let mut dst_pitch = destination.pitch as u32;
            let (width, height) = source.dimensions();
            let mut width = width;
            let mut height = height;
            let mut params = [
                (&mut src as *mut u64).cast::<c_void>(),
                (&mut dst as *mut u64).cast::<c_void>(),
                (&mut src_pitch as *mut u32).cast::<c_void>(),
                (&mut dst_pitch as *mut u32).cast::<c_void>(),
                (&mut width as *mut u32).cast::<c_void>(),
                (&mut height as *mut u32).cast::<c_void>(),
            ];
            let block_x = 16;
            let block_y = 16;
            let grid_x = width.div_ceil(block_x);
            let grid_y = height.div_ceil(block_y);
            let result = unsafe {
                (self.driver.cu_launch_kernel)(
                    self.function,
                    grid_x,
                    grid_y,
                    1,
                    block_x,
                    block_y,
                    1,
                    0,
                    std::ptr::null_mut(),
                    params.as_mut_ptr(),
                    std::ptr::null_mut(),
                )
            };
            if result != CUDA_SUCCESS {
                return Err(format!(
                    "cuLaunchKernel for NV12 conversion failed with {}",
                    self.driver.describe_error(result)
                ));
            }
            Ok(())
        }
    }

    impl Drop for CudaNv12ToRgbaConverter<'_> {
        fn drop(&mut self) {
            let _ = unsafe { (self.driver.cu_module_unload)(self.module) };
        }
    }

    pub struct CudaContext<'a> {
        driver: &'a CudaDriver,
        raw: CuContext,
        device: CuDevice,
        ordinal: i32,
        release_primary: bool,
    }

    impl CudaContext<'_> {
        pub fn ordinal(&self) -> i32 {
            self.ordinal
        }

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

        pub fn dimensions(&self) -> (u32, u32) {
            (self.width, self.height)
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
        let result = unsafe {
            (driver.cu_import_external_memory)(external_memory.as_mut_ptr(), &handle_desc)
        };
        if result != CUDA_SUCCESS {
            return Err(format!(
                "cuImportExternalMemory failed with {}",
                driver.describe_error(result)
            ));
        }
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
    CudaContext, CudaDeviceAllocation, CudaDeviceInfo, CudaDriver, CudaNv12ToRgbaConverter,
    ImportedCudaExternalImage, import_owned_vulkan_opaque_fd_image, import_vulkan_opaque_fd_image,
};
