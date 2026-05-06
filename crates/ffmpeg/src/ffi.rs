use std::{
    ffi::{CStr, CString},
    ptr::NonNull,
    sync::Once,
};

use crate::{FfmpegError, Result};

pub(crate) use ffmpeg_sys_next as sys;

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

    #[cfg(feature = "metal")]
    pub(crate) fn data(&self, plane: usize) -> *mut u8 {
        unsafe { (*self.ptr.as_ptr()).data[plane] }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats_known_error_code() {
        let error = error_from_code("test", sys::AVERROR_EOF);
        assert!(error.message.to_lowercase().contains("end"));
    }

    #[test]
    fn owns_packet_and_frame() {
        let packet = AvPacket::new().expect("packet");
        let frame = AvFrame::new().expect("frame");
        assert!(!packet.as_ptr().is_null());
        assert!(!frame.as_ptr().is_null());
    }
}
