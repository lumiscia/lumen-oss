use crate::{FfmpegError, Result, ffi::sys};

use super::output::OutputContext;

pub(crate) fn refresh_stream_time_base(
    output: &OutputContext,
    stream_index: usize,
    operation: &'static str,
) -> Result<sys::AVRational> {
    unsafe {
        let stream_count = (*output.ptr).nb_streams as usize;
        if stream_index >= stream_count {
            return Err(FfmpegError::new(
                operation,
                format!(
                    "stream index {stream_index} is outside output stream count {stream_count}"
                ),
            )
            .with_path(output.path().to_string()));
        }
        let stream = *(*output.ptr).streams.add(stream_index);
        if stream.is_null() {
            return Err(FfmpegError::new(operation, "output stream is null")
                .with_path(output.path().to_string()));
        }
        Ok((*stream).time_base)
    }
}
