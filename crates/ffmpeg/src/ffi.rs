use std::{
    ffi::{CStr, CString},
    ptr::NonNull,
    sync::Once,
};

use crate::{FfmpegError, Result};

pub(crate) use ffmpeg_sys_next as sys;

// libswscale's `SWS_BILINEAR` flag; bindings expose it as a constant or an enum depending on the FFmpeg version.
pub(crate) const SWS_BILINEAR: std::ffi::c_int = 1 << 1;

static INIT: Once = Once::new();

pub(crate) fn init() {
    INIT.call_once(|| unsafe {
        sys::av_log_set_level(sys::AV_LOG_ERROR);
    });
}

pub(crate) fn cstring(operation: &'static str, value: &str) -> Result<CString> {
    CString::new(value)
        .map_err(|_| FfmpegError::new(operation, "string contains an interior NUL byte"))
}

pub(crate) fn check(code: i32, operation: &'static str) -> Result<()> {
    if code < 0 {
        Err(error_from_code(operation, code))
    } else {
        Ok(())
    }
}

pub(crate) fn error_from_code(operation: &'static str, code: i32) -> FfmpegError {
    FfmpegError {
        operation,
        path: None,
        code: Some(code),
        message: av_error_string(code),
        backend: None,
        codec: None,
        stream_index: None,
    }
}

pub(crate) fn av_error_string(code: i32) -> String {
    let mut buffer = [0 as libc::c_char; 256];
    unsafe {
        let result = sys::av_strerror(code, buffer.as_mut_ptr(), buffer.len());
        if result < 0 {
            return format!("FFmpeg error {code}");
        }
        CStr::from_ptr(buffer.as_ptr())
            .to_string_lossy()
            .into_owned()
    }
}

/// Frees the codec context on early returns until ownership moves into a decoder or encoder.
pub(crate) struct CodecContextGuard(pub(crate) *mut sys::AVCodecContext);

impl CodecContextGuard {
    pub(crate) fn release(&mut self) -> *mut sys::AVCodecContext {
        std::mem::replace(&mut self.0, std::ptr::null_mut())
    }
}

impl Drop for CodecContextGuard {
    fn drop(&mut self) {
        unsafe { sys::avcodec_free_context(&mut self.0) };
    }
}

pub(crate) struct AvPacket {
    ptr: NonNull<sys::AVPacket>,
}

impl AvPacket {
    pub(crate) fn new() -> Result<Self> {
        let ptr = unsafe { sys::av_packet_alloc() };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| FfmpegError::new("av_packet_alloc", "failed to allocate packet"))?;
        Ok(Self { ptr })
    }

    pub(crate) fn as_ptr(&self) -> *const sys::AVPacket {
        self.ptr.as_ptr()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut sys::AVPacket {
        self.ptr.as_ptr()
    }

    pub(crate) fn stream_index(&self) -> usize {
        unsafe { (*self.ptr.as_ptr()).stream_index as usize }
    }

    pub(crate) fn pts(&self) -> Option<i64> {
        let pts = unsafe { (*self.ptr.as_ptr()).pts };
        (pts != sys::AV_NOPTS_VALUE).then_some(pts)
    }

    pub(crate) fn dts(&self) -> Option<i64> {
        let dts = unsafe { (*self.ptr.as_ptr()).dts };
        (dts != sys::AV_NOPTS_VALUE).then_some(dts)
    }

    pub(crate) fn is_discarded(&self) -> bool {
        unsafe { (*self.ptr.as_ptr()).flags & sys::AV_PKT_FLAG_DISCARD != 0 }
    }

    pub(crate) fn clone_without_discard(&self) -> Result<Self> {
        let ptr = unsafe { sys::av_packet_clone(self.ptr.as_ptr()) };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| FfmpegError::new("av_packet_clone", "failed to clone packet"))?;
        unsafe { (*ptr.as_ptr()).flags &= !sys::AV_PKT_FLAG_DISCARD };
        Ok(Self { ptr })
    }
}

impl Drop for AvPacket {
    fn drop(&mut self) {
        unsafe {
            let mut ptr = self.ptr.as_ptr();
            sys::av_packet_free(&mut ptr);
        }
    }
}

pub(crate) struct AvFrame {
    ptr: NonNull<sys::AVFrame>,
}

impl AvFrame {
    pub(crate) fn new() -> Result<Self> {
        let ptr = unsafe { sys::av_frame_alloc() };
        let ptr = NonNull::new(ptr)
            .ok_or_else(|| FfmpegError::new("av_frame_alloc", "failed to allocate frame"))?;
        Ok(Self { ptr })
    }

    /// A frame of `format` and size with its own buffers allocated.
    pub(crate) fn with_buffers(
        format: sys::AVPixelFormat,
        width: i32,
        height: i32,
    ) -> Result<Self> {
        let mut frame = Self::new()?;
        unsafe {
            let raw = frame.as_mut_ptr();
            (*raw).format = format as i32;
            (*raw).width = width;
            (*raw).height = height;
            check(sys::av_frame_get_buffer(raw, 0), "av_frame_get_buffer")?;
        }
        Ok(frame)
    }

    pub(crate) fn as_ptr(&self) -> *const sys::AVFrame {
        self.ptr.as_ptr()
    }

    pub(crate) fn as_mut_ptr(&mut self) -> *mut sys::AVFrame {
        self.ptr.as_ptr()
    }

    pub(crate) fn format(&self) -> sys::AVPixelFormat {
        unsafe { std::mem::transmute::<i32, sys::AVPixelFormat>((*self.ptr.as_ptr()).format) }
    }

    pub(crate) fn sample_format(&self) -> sys::AVSampleFormat {
        unsafe { std::mem::transmute::<i32, sys::AVSampleFormat>((*self.ptr.as_ptr()).format) }
    }

    pub(crate) fn pts(&self) -> Option<i64> {
        let pts = unsafe { (*self.ptr.as_ptr()).pts };
        (pts != sys::AV_NOPTS_VALUE).then_some(pts)
    }

    pub(crate) fn colorspace(&self) -> sys::AVColorSpace {
        unsafe { (*self.ptr.as_ptr()).colorspace }
    }

    pub(crate) fn color_range(&self) -> sys::AVColorRange {
        unsafe { (*self.ptr.as_ptr()).color_range }
    }

    #[cfg(feature = "metal")]
    pub(crate) fn data(&self, plane: usize) -> *mut u8 {
        unsafe { (*self.ptr.as_ptr()).data[plane] }
    }

    #[cfg(feature = "vulkan")]
    pub(crate) fn hw_sw_format(&self) -> Option<sys::AVPixelFormat> {
        let frame = unsafe { self.ptr.as_ref() };
        if frame.hw_frames_ctx.is_null() {
            return None;
        }
        let frames = unsafe { (*frame.hw_frames_ctx).data }.cast::<sys::AVHWFramesContext>();
        if frames.is_null() {
            return None;
        }
        Some(unsafe { (*frames).sw_format })
    }

    pub(crate) fn width(&self) -> u32 {
        unsafe { (*self.ptr.as_ptr()).width.max(0) as u32 }
    }

    pub(crate) fn height(&self) -> u32 {
        unsafe { (*self.ptr.as_ptr()).height.max(0) as u32 }
    }

    pub(crate) fn nb_samples(&self) -> usize {
        unsafe { (*self.ptr.as_ptr()).nb_samples.max(0) as usize }
    }
}

impl Drop for AvFrame {
    fn drop(&mut self) {
        unsafe {
            let mut ptr = self.ptr.as_ptr();
            sys::av_frame_free(&mut ptr);
        }
    }
}
