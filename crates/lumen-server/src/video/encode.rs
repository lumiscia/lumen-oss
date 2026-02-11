use std::{io::Write, time::Duration};

use ac_ffmpeg::{
    codec::{
        Encoder,
        video::{VideoEncoder, VideoFrame},
    },
    format::{
        io::IO,
        muxer::{Muxer, OutputFormat},
    },
    time::TimeBase,
};
use anyhow::anyhow;

pub struct H264Encoder<T: Write> {
    encoder: VideoEncoder,
    time_base: TimeBase,
    muxer: Muxer<T>,
}

impl<T: Write> H264Encoder<T> {
    pub fn new(
        width: usize,
        height: usize,
        time_base: TimeBase,
        io: IO<T>,
    ) -> anyhow::Result<Self> {
        let default_preset = if cfg!(debug_assertions) {
            "ultrafast"
        } else {
            "medium"
        };
        let preset = std::env::var("LUMEN_X264_PRESET")
            .ok()
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default_preset.to_string());

        let encoder = VideoEncoder::builder("libx264")?
            .pixel_format(ac_ffmpeg::codec::video::frame::get_pixel_format("yuv420p"))
            .width(width)
            .height(height)
            .time_base(time_base)
            .set_option("preset", &preset)
            .set_option("tune", "zerolatency")
            .build()?;

        let mut muxer_builder = Muxer::builder();
        muxer_builder.add_stream(&encoder.codec_parameters().into())?;

        let output_format = match OutputFormat::find_by_mime_type("video/mp4") {
            Some(format) => format,
            None => return Err(anyhow!("Mime type video/mp4 is invalid")),
        };

        Ok(Self {
            encoder,
            time_base,
            muxer: muxer_builder.build(io, output_format)?,
        })
    }

    pub fn encode_frame(&mut self, video_frame: VideoFrame) -> anyhow::Result<()> {
        self.encoder.push(video_frame)?;

        while let Some(packet) = self.encoder.take()? {
            self.push_packet(packet)?;
        }

        Ok(())
    }

    pub fn finish(&mut self) -> anyhow::Result<()> {
        self.encoder.flush()?;

        while let Some(packet) = self.encoder.take()? {
            self.push_packet(packet)?;
        }

        self.muxer.flush()?;

        Ok(())
    }

    pub fn close(self) -> anyhow::Result<IO<T>> {
        self.muxer.close().map_err(|err| err.into())
    }

    fn push_packet(&mut self, packet: ac_ffmpeg::packet::Packet) -> anyhow::Result<()> {
        self.muxer.push(
            packet
                .with_duration(Duration::from_micros(
                    ((self.time_base.num() * 1_000_000) / self.time_base.den()) as u64,
                ))
                .with_stream_index(0),
        )?;
        Ok(())
    }
}
