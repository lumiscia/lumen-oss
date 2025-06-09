use ac_ffmpeg::codec::video::VideoFrame;
use ac_ffmpeg::format::io::IO;
use ac_ffmpeg::time::TimeBase;
use output::encode::YUV420pEncoder;
use output::worker::RenderWorker;
use parking_lot::RwLock;
use std::collections::BTreeMap;
use std::io::Cursor;
use std::num::NonZero;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

pub mod input;
pub mod output;

const WIDTH: usize = 1920;
const HEIGHT: usize = 1080;
const FRAMES_PER_SECOND: usize = 30;
const FRAME_COUNT: usize = 1800;

pub fn create_video() -> anyhow::Result<Vec<u8>> {
    let (tx, rx) = crossbeam_channel::bounded::<(usize, VideoFrame)>(128);

    let thread_count = std::thread::available_parallelism()?.get();

    let cache = Arc::new(RwLock::new(LruCache::<u64, Dependency>::new(
        NonZero::new(64).unwrap(),
    )));

    let dependency_thread = {
        let cache = cache.clone();
        std::thread::spawn(move || -> anyhow::Result<()> {
            // read frames until they are evicted
            Ok(())
        })
    };

    let encoding_thread = std::thread::spawn(move || -> anyhow::Result<Vec<u8>> {
        let mut encoder = YUV420pEncoder::new(
            WIDTH,
            HEIGHT,
            TimeBase::new(1, FRAMES_PER_SECOND as i32),
            IO::from_seekable_write_stream(Cursor::new(Vec::new())),
        )?;

        let mut current_frame = 0;
        let mut frame_buffer: BTreeMap<usize, VideoFrame> = BTreeMap::new();

        for (idx, frame) in rx {
            if idx == current_frame {
                match encoder.encode_frame(frame) {
                    Ok(_) => {}
                    Err(err) => {
                        eprintln!("Error encoding frame {}: {:?}", idx, err);
                    }
                };

                current_frame += 1;

                while current_frame < FRAME_COUNT {
                    match frame_buffer.remove(&current_frame) {
                        Some(frame) => {
                            match encoder.encode_frame(frame) {
                                Ok(_) => {}
                                Err(err) => {
                                    eprintln!("Error encoding frame {}: {:?}", idx, err);
                                }
                            };
                            current_frame += 1
                        }
                        None => break,
                    }
                }
            } else {
                frame_buffer.insert(idx, frame);
            }
        }

        encoder.finish()?;

        let io = encoder.close()?;

        Ok(io.into_stream().into_inner())
    });

    let current_frame = Arc::new(AtomicUsize::new(0));

    std::thread::scope(|s| {
        for _ in 0..thread_count {
            let tx = tx.clone();
            let current_frame = current_frame.clone();

            s.spawn(move || {
                let mut worker = RenderWorker::new(
                    WIDTH,
                    HEIGHT,
                    FRAME_COUNT,
                    TimeBase::new(1, FRAMES_PER_SECOND as i32),
                )
                .expect("Failed to create RenderWorker");

                loop {
                    let frame_idx = current_frame.fetch_add(1, Ordering::SeqCst);
                    if frame_idx >= FRAME_COUNT {
                        break;
                    }

                    match worker.draw_frame(frame_idx) {
                        Ok(frame) => {
                            if let Err(err) = tx.send((frame_idx, frame)) {
                                eprintln!("Error sending frame {}: {:?}", frame_idx, err);
                                break;
                            }
                        }
                        Err(err) => {
                            eprintln!("Render error at frame {}: {:?}", frame_idx, err);
                        }
                    }
                }
            });
        }
    });

    drop(tx);

    match dependency_thread.join() {
        Ok(_) => {}
        Err(_) => return Err(anyhow::anyhow!("Failed to join handle")),
    };

    match encoding_thread.join() {
        Ok(value) => value,
        Err(_) => Err(anyhow::anyhow!("Failed to join handle")),
    }
}
