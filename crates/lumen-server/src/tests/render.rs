use std::{
    collections::BTreeMap,
    fs::File,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
    },
    thread,
};

use ac_ffmpeg::{codec::video::VideoFrame, format::io::IO, time::TimeBase};

use crate::{
    tests::create_test_timeline,
    video::{encode::H264Encoder, render::FFmpegRenderer},
};

#[test]
#[ignore = "expensive ffmpeg integration test"]
fn test_singlethread_render() {
    let tb = TimeBase::new(1, 30);
    let timeline = create_test_timeline();

    let mut renderer =
        FFmpegRenderer::new(1920, 1080, 1000, 40000, tb, Arc::new(timeline)).unwrap();

    let output = File::create("./test_render.mp4").unwrap();

    let mut encoder =
        H264Encoder::new(1920, 1080, tb, IO::from_seekable_write_stream(output)).unwrap();

    for i in 0..2400 {
        let frame = renderer.draw_frame(i).unwrap();
        encoder.encode_frame(frame).unwrap();
    }

    encoder.finish().unwrap();
}

#[test]
#[ignore = "expensive ffmpeg integration test"]
fn test_multithreaded_render() {
    thread::scope(|scope| {
        let tb = TimeBase::new(1, 30);
        let timeline = Arc::new(create_test_timeline());

        let (tx, rx) = crossbeam_channel::bounded::<(usize, VideoFrame)>(128);

        let current_frame = Arc::new(AtomicUsize::new(0));
        for _ in 0..12 {
            let tx = tx.clone();

            let timeline = timeline.clone();

            let current_frame = current_frame.clone();

            scope.spawn(move || {
                let mut renderer =
                    FFmpegRenderer::new(1920, 1080, 80000, 30, tb, timeline).unwrap();

                loop {
                    let frame = current_frame.fetch_add(1, Ordering::SeqCst);

                    if frame >= 2400 {
                        break;
                    }

                    tx.send((frame, renderer.draw_frame(frame).unwrap()))
                        .unwrap();
                }
            });
        }

        drop(tx);

        scope.spawn(move || {
            let output = File::create("./test_render.mp4").unwrap();

            let mut encoder =
                H264Encoder::new(1920, 1080, tb, IO::from_seekable_write_stream(output)).unwrap();

            let mut queued_frames = BTreeMap::new();

            let mut current_frame: usize = 0;

            while let Ok((idx, frame)) = rx.recv() {
                if idx == current_frame {
                    encoder.encode_frame(frame).unwrap();
                    current_frame += 1;

                    while let Some((idx, _)) = queued_frames.first_key_value() {
                        if *idx != current_frame {
                            break;
                        }

                        encoder
                            .encode_frame(queued_frames.pop_first().unwrap().1)
                            .unwrap();
                        current_frame += 1;
                    }

                    continue;
                };

                queued_frames.insert(idx, frame);
            }

            encoder.finish().unwrap();
        });
    });
}
