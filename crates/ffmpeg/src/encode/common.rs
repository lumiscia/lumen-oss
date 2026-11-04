use crate::ffi::{self, AvPacket, sys};
use crate::{FfmpegError, Result};

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

/// Sends `frame` to the encoder, or flushes it when `frame` is null, and writes every packet it has
/// ready. Packets the encoder left without a duration get `unset_duration`, in the encoder's time
/// base.
pub(crate) fn send_frame_and_write_packets(
    context: *mut sys::AVCodecContext,
    frame: *const sys::AVFrame,
    output: &mut OutputContext,
    stream_index: usize,
    stream_time_base: sys::AVRational,
    unset_duration: i64,
) -> Result<()> {
    unsafe {
        ffi::check(
            sys::avcodec_send_frame(context, frame),
            "avcodec_send_frame",
        )?;
    }
    loop {
        let mut packet = AvPacket::new()?;
        let result = unsafe { sys::avcodec_receive_packet(context, packet.as_mut_ptr()) };
        if result == sys::AVERROR(libc::EAGAIN) || result == sys::AVERROR_EOF {
            return Ok(());
        }
        if result < 0 {
            return Err(ffi::error_from_code("avcodec_receive_packet", result));
        }
        unsafe {
            (*packet.as_mut_ptr()).stream_index = stream_index as i32;
            if (*packet.as_mut_ptr()).duration == 0 {
                (*packet.as_mut_ptr()).duration = unset_duration;
            }
            sys::av_packet_rescale_ts(packet.as_mut_ptr(), (*context).time_base, stream_time_base);
            ffi::check(
                sys::av_interleaved_write_frame(output.ptr, packet.as_mut_ptr()),
                "av_interleaved_write_frame",
            )
            .map_err(|error| error.with_path(output.path().to_string()))?;
        }
    }
}
