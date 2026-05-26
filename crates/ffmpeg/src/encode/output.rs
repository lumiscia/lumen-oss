use std::ptr;

use crate::ffi::{self, sys};
use crate::{FfmpegError, Result};

pub struct OutputContext {
    path: String,
    pub(crate) ptr: *mut sys::AVFormatContext,
    opened_io: bool,
}

unsafe impl Send for OutputContext {}

impl OutputContext {
    pub fn create(path: impl Into<String>) -> Result<Self> {
        ffi::init();
        let path = path.into();
        let c_path = ffi::cstring("avformat_alloc_output_context2", &path)?;
        let mut ptr: *mut sys::AVFormatContext = ptr::null_mut();
        unsafe {
            ffi::check(
                sys::avformat_alloc_output_context2(
                    &mut ptr,
                    ptr::null_mut(),
                    ptr::null(),
                    c_path.as_ptr(),
                ),
                "avformat_alloc_output_context2",
            )
            .map_err(|error| error.with_path(path.clone()))?;
        }
        if ptr.is_null() {
            return Err(FfmpegError::new(
                "avformat_alloc_output_context2",
                "failed to allocate output context",
            )
            .with_path(path));
        }
        Ok(Self {
            path,
            ptr,
            opened_io: false,
        })
    }

    pub fn path(&self) -> &str {
        &self.path
    }

    pub(crate) fn write_header(&mut self) -> Result<()> {
        self.open_io()?;
        unsafe {
            ffi::check(
                sys::avformat_write_header(self.ptr, ptr::null_mut()),
                "avformat_write_header",
            )
            .map_err(|error| error.with_path(self.path.clone()))
        }
    }

    pub(crate) fn write_trailer(&mut self) -> Result<()> {
        unsafe {
            ffi::check(sys::av_write_trailer(self.ptr), "av_write_trailer")
                .map_err(|error| error.with_path(self.path.clone()))
        }
    }

    fn open_io(&mut self) -> Result<()> {
        unsafe {
            if ((*(*self.ptr).oformat).flags & sys::AVFMT_NOFILE) == 0 {
                let c_path = ffi::cstring("avio_open", &self.path)?;
                ffi::check(
                    sys::avio_open(&mut (*self.ptr).pb, c_path.as_ptr(), sys::AVIO_FLAG_WRITE),
                    "avio_open",
                )
                .map_err(|error| error.with_path(self.path.clone()))?;
                self.opened_io = true;
            }
        }
        Ok(())
    }
}

impl Drop for OutputContext {
    fn drop(&mut self) {
        unsafe {
            if self.opened_io && !(*self.ptr).pb.is_null() {
                sys::avio_closep(&mut (*self.ptr).pb);
            }
            sys::avformat_free_context(self.ptr);
        }
    }
}
