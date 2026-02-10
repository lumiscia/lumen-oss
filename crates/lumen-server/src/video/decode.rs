use std::io::Read;

use ac_ffmpeg::{
    codec::{
        Decoder,
        video::{VideoDecoder as FVideoDecoder, VideoFrame},
    },
    format::{
        demuxer::{Demuxer, DemuxerWithStreamInfo, SeekTarget},
        io::IO,
    },
    time::{TimeBase, Timestamp},
};

pub struct VideoDecoder<T> {
    current: Timestamp,
    demuxer: DemuxerWithStreamInfo<T>,
    decoder: FVideoDecoder,

    pub duration: Timestamp,
    pub time_base: TimeBase,
    stream_index: usize,
}

impl<T: Read> VideoDecoder<T> {
    pub fn new(io: IO<T>) -> anyhow::Result<Self> {
        let demuxer = Demuxer::builder()
            .build(io)?
            .find_stream_info(None)
            .map_err(|(_, err)| err)?;

        let (stream_index, (stream, _)) = demuxer
            .streams()
            .iter()
            .map(|stream| (stream, stream.codec_parameters()))
            .enumerate()
            .find(|(_, (_, params))| params.is_video_codec())
            .ok_or_else(|| anyhow::anyhow!("no video stream"))?;

        let duration = stream.duration();
        let time_base = stream.time_base();

        let decoder = FVideoDecoder::from_stream(stream)?.build()?;

        Ok(Self {
            current: Timestamp::from_micros(0),
            demuxer,
            decoder,

            duration,
            time_base,
            stream_index,
        })
    }

    // Seeks to, then returns any previous frames and frames after for the specified duration, usually + 1
    pub fn decode(
        &mut self,
        seek_to: Option<Timestamp>,
        duration: Timestamp,
    ) -> anyhow::Result<Vec<VideoFrame>> {
        if duration.is_null() {
            return Err(anyhow::anyhow!("duration was null"));
        }

        let mut frames = vec![];

        if let Some(seek_to) = seek_to {
            if seek_to.is_null() {
                return Err(anyhow::anyhow!("seek_to was null"));
            }

            self.demuxer.seek_to_timestamp(seek_to, SeekTarget::UpTo)?;

            while let Some(packet) = self.demuxer.take()? {
                if packet.stream_index() != self.stream_index {
                    continue;
                }

                let pts = packet.pts();
                self.current = pts;
                self.decoder.push(packet)?;

                while let Some(frame) = self.decoder.take()? {
                    frames.push(frame);
                }

                if pts > seek_to {
                    break;
                }
            }
        }

        let current_micros = self.current.as_micros().unwrap_or(0);
        let duration_micros = duration.as_micros().unwrap_or(0);
        let end_micros = current_micros.saturating_add(duration_micros);

        while let Some(packet) = self.demuxer.take()? {
            if packet.stream_index() != self.stream_index {
                continue;
            }

            let pts = packet.pts();

            if pts.is_null() {
                break;
            }

            self.current = pts;
            self.decoder.push(packet)?;

            while let Some(frame) = self.decoder.take()? {
                frames.push(frame);
            }

            if pts.as_micros().unwrap_or(0) > end_micros {
                break;
            }
        }

        Ok(frames)
    }
}
