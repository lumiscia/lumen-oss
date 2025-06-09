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

pub struct YUV420pEncoder<T: Write> {
    encoder: VideoEncoder,
    time_base: TimeBase,
    muxer: Muxer<T>,
}

impl<T: Write> YUV420pEncoder<T> {
    pub fn new(
        width: usize,
        height: usize,
        time_base: TimeBase,
        io: IO<T>,
    ) -> anyhow::Result<Self> {
        let pixel_format = ac_ffmpeg::codec::video::frame::get_pixel_format("yuv420p");

        let encoder = VideoEncoder::builder("libx264")?
            .pixel_format(pixel_format.clone())
            .width(width)
            .height(height)
            .time_base(time_base.clone())
            .set_option("preset", "ultrafast")
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
            self.muxer.push(
                packet
                    .with_duration(Duration::from_micros(
                        ((self.time_base.num() * 1_000_000) / self.time_base.den()) as u64,
                    ))
                    .with_stream_index(0),
            )?;
        }

        Ok(())
    }

    pub fn finish(&mut self) -> anyhow::Result<()> {
        self.encoder.flush()?;

        loop {
            match self.encoder.take()? {
                Some(packet) => {
                    self.muxer.push(packet)?;
                }
                None => break,
            }
        }

        self.muxer.flush()?;

        Ok(())
    }

    pub fn close(self) -> anyhow::Result<IO<T>> {
        self.muxer.close().map_err(|err| err.into())
    }
}
