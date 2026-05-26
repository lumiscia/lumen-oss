#![cfg(target_os = "linux")]

use std::sync::Arc;

use cudarc::{
    driver::{CudaContext, CudaFunction, CudaModule, CudaStream, LaunchConfig, PushKernelArg},
    nvrtc::Ptx,
};

use super::{CudaDecodedFrame, CudaDeviceAllocation};

pub struct CudaNv12ToRgbaConverter {
    context: Arc<CudaContext>,
    stream: Arc<CudaStream>,
    function: CudaFunction,
    _module: Arc<CudaModule>,
}

impl CudaNv12ToRgbaConverter {
    pub fn new(context: Arc<CudaContext>) -> Result<Self, String> {
        let ptx_src = std::fs::read_to_string(env!("LUMEN_CUDA_NV12_PTX"))
            .map_err(|error| format!("failed to read NV12 conversion PTX: {error}"))?;
        let module = context
            .load_module(Ptx::from_src(ptx_src))
            .map_err(|error| error.to_string())?;
        let function = module
            .load_function("nv12_to_rgba8")
            .map_err(|error| error.to_string())?;
        let stream = context.default_stream();
        Ok(Self {
            context,
            stream,
            function,
            _module: module,
        })
    }

    pub fn convert(
        &self,
        source: &CudaDecodedFrame,
        destination: &CudaDeviceAllocation,
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

        self.context
            .bind_to_thread()
            .map_err(|error| error.to_string())?;

        let src = source.device_ptr();
        let dst = destination.device_ptr();
        let src_pitch = source.pitch() as u32;
        let dst_pitch = destination.pitch() as u32;
        let (width, height) = source.dimensions();

        let block_x = 16;
        let block_y = 16;
        let config = LaunchConfig {
            grid_dim: (width.div_ceil(block_x), height.div_ceil(block_y), 1),
            block_dim: (block_x, block_y, 1),
            shared_mem_bytes: 0,
        };

        let mut builder = self.stream.launch_builder(&self.function);
        builder.arg(&src);
        builder.arg(&dst);
        builder.arg(&src_pitch);
        builder.arg(&dst_pitch);
        builder.arg(&width);
        builder.arg(&height);
        unsafe { builder.launch(config) }
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}
