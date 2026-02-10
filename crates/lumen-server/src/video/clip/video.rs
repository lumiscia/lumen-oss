use std::{io::Read, num::NonZero};

use ac_ffmpeg::{
    codec::{Decoder, video::VideoDecoder},
    format::{
        demuxer::{Demuxer, DemuxerWithStreamInfo},
        io::IO,
    },
    time::{self, TimeBase, Timestamp},
};
use lumen::{
    clip::{Clip, ClipError},
    render::RenderContext,
    skia::Image,
};
use parking_lot::Mutex;

struct VideoState<T> {
    demuxer: DemuxerWithStreamInfo<T>,
    decoder: VideoDecoder,

    duration: Timestamp,
    time_base: TimeBase,
    stream_index: usize,

    frame_cache: lru::LruCache<i64, Image>,
}

pub struct Video<T> {
    speed: f32,
    start_timestamp: Timestamp,

    inner: Mutex<VideoState<T>>,
}

impl<T: Read> Video<T> {
    pub fn new(io: IO<T>, speed: f32, start_timestamp: Timestamp) -> anyhow::Result<Self> {
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

        let decoder = VideoDecoder::from_stream(stream)?.build()?;

        Ok(Self {
            speed,
            start_timestamp,
            inner: Mutex::new(VideoState {
                demuxer,
                decoder,

                duration,
                time_base,
                stream_index,

                frame_cache: lru::LruCache::new(NonZero::new(32).unwrap()),
            }),
        })
    }
}

impl<T: Read> Clip for Video<T> {
    fn draw(&self, frame: usize, context: &mut RenderContext) -> Result<(), ClipError> {
        let mut state = self.inner.lock();

        let rate_tb = TimeBase::new(1, context.rate as i32);

        let required_ts = Timestamp::from_micros(
            frame as i64 / context.rate as i64 * 1_000_000
                + self.start_timestamp.as_micros().unwrap(),
        )
        .with_time_base(rate_tb)
        .timestamp();

        while let Some(packet) = state
            .demuxer
            .take()
            .map_err(|_| ClipError::Message("Error taking packet from demuxer".into()))?
        {
            packet;
        }

        let frame = if let Some(cached) = state.frame_cache.get(&required_ts) {
            cached
        } else {
            return Ok(());
            /*
            let frames = state
                .decoder
                .take()
                .decode(None, time::Timestamp::from_micros(1_000_000))
                .map_err(|_| ClipError::Message("Error decoding".into()))?;

            if frames.is_empty() {
                return Ok(());
            }

            for frame in frames.into_iter() {
                let ts = frame.pts().with_time_base(rate_tb).timestamp();

                state.frame_cache.push(ts, frame);
            }

            match state.frame_cache.get(&required_ts).or(None) {
                Some(frame) => frame,
                None => {
                    return Ok(());
                }
            }
            */
        };

        Ok(())
    }
}
