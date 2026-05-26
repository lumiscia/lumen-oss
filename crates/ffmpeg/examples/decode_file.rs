use std::{
    env,
    path::PathBuf,
    time::{Duration, Instant},
};

#[cfg(all(feature = "cuda", target_os = "linux"))]
use lumen_ffmpeg::CudaDriver;
use lumen_ffmpeg::{
    DecodeMode, GpuBackend, GpuVideoFrame, InputContext, VideoDecoder, VideoDecoderConfig,
};

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = env::args().skip(1);
    let path = args.next().map(PathBuf::from).ok_or(
        "usage: decode_file <path> [max_frames] [cpu|cuda|metal|vulkan] [gpu|rgba|cuda-rgba]",
    )?;
    let next = args.next();
    let (max_frames, mode_arg) = match next {
        Some(value) => match value.parse::<usize>() {
            Ok(max_frames) => (Some(max_frames), args.next()),
            Err(_) => (None, Some(value)),
        },
        None => (None, None),
    };
    let mode = match mode_arg.as_deref() {
        Some("cuda") => DecodeMode::Gpu(GpuBackend::Cuda),
        Some("metal") => DecodeMode::Gpu(GpuBackend::Metal),
        Some("vulkan") => DecodeMode::Gpu(GpuBackend::Vulkan),
        Some("cpu") | None => DecodeMode::Cpu,
        Some(other) => return Err(format!("unknown decode mode `{other}`").into()),
    };
    let receive = match args.next().as_deref() {
        Some("gpu") => ReceiveMode::Gpu,
        Some("cuda-rgba") => ReceiveMode::CudaRgba,
        Some("rgba") | None => ReceiveMode::Rgba,
        Some(other) => return Err(format!("unknown receive mode `{other}`").into()),
    };

    let started = Instant::now();
    let mut input = InputContext::open(path.to_string_lossy().to_string())?;
    let stream = input.best_video_stream()?;
    let open_elapsed = started.elapsed();
    let mut decoder = VideoDecoder::open(
        &input,
        VideoDecoderConfig {
            stream_index: stream.stream_index,
            mode,
        },
    )?;

    let decode_started = Instant::now();
    let mut cuda = CudaRgbaState::new(receive, stream.width, stream.height)?;
    let mut frames = 0_usize;
    let mut bytes = 0_usize;

    'decode: while let Some(packet) = input.read_packet()? {
        decoder.send_packet(&packet)?;
        while let Some(frame_bytes) = receive_frame(&mut decoder, mode, receive, cuda.as_mut())? {
            frames = frames.saturating_add(1);
            bytes = bytes.saturating_add(frame_bytes);
            if max_frames.is_some_and(|limit| frames >= limit) {
                break 'decode;
            }
        }
    }

    if max_frames.is_none_or(|limit| frames < limit) {
        decoder.send_eof()?;
        while let Some(frame_bytes) = receive_frame(&mut decoder, mode, receive, cuda.as_mut())? {
            frames = frames.saturating_add(1);
            bytes = bytes.saturating_add(frame_bytes);
            if max_frames.is_some_and(|limit| frames >= limit) {
                break;
            }
        }
    }

    let decode_elapsed = decode_started.elapsed();
    let total_elapsed = started.elapsed();
    let usage = usage();
    println!("path={}", path.display());
    println!("codec={:?}", stream.codec);
    println!("dimensions={}x{}", stream.width, stream.height);
    println!("frames={frames}");
    println!("mode={mode:?}");
    println!("receive={receive:?}");
    println!("decoded_frame_bytes={bytes}");
    println!("open_ms={}", millis(open_elapsed));
    println!("decode_ms={}", millis(decode_elapsed));
    println!("total_ms={}", millis(total_elapsed));
    println!(
        "fps={:.2}",
        frames as f64 / decode_elapsed.as_secs_f64().max(1e-9)
    );
    println!("user_cpu_ms={}", millis(usage.user));
    println!("system_cpu_ms={}", millis(usage.system));
    println!("max_rss_platform_units={}", usage.max_rss);
    Ok(())
}

fn receive_frame(
    decoder: &mut VideoDecoder,
    mode: DecodeMode,
    receive: ReceiveMode,
    cuda: Option<&mut CudaRgbaState>,
) -> lumen_ffmpeg::Result<Option<usize>> {
    match receive {
        ReceiveMode::Rgba => Ok(decoder.receive_rgba_frame()?.map(|frame| frame.data.len())),
        ReceiveMode::CudaRgba => match decoder.receive_gpu_frame()? {
            Some(frame) => {
                let Some(cuda) = cuda else {
                    return Err(lumen_ffmpeg::FfmpegError::new(
                        "decode_file",
                        "cuda-rgba receive mode requires a CUDA conversion state",
                    ));
                };
                cuda.convert(&frame)?;
                Ok(Some(frame.estimated_rgba_bytes() as usize))
            }
            None => Ok(None),
        },
        ReceiveMode::Gpu => match mode {
            DecodeMode::Cpu => Ok(decoder.receive_cpu_frame()?.map(|frame| frame.data.len())),
            DecodeMode::Gpu(_) => Ok(decoder.receive_gpu_frame()?.map(|_| 0)),
        },
    }
}

#[derive(Debug, Clone, Copy)]
enum ReceiveMode {
    Rgba,
    Gpu,
    CudaRgba,
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
struct CudaRgbaState {
    converter: lumen_ffmpeg::CudaNv12ToRgbaConverter<'static>,
    destination: lumen_ffmpeg::CudaDeviceAllocation<'static>,
    _context: lumen_ffmpeg::CudaContext<'static>,
}

#[cfg(all(feature = "cuda", target_os = "linux"))]
impl CudaRgbaState {
    fn new(
        receive: ReceiveMode,
        width: u32,
        height: u32,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        if !matches!(receive, ReceiveMode::CudaRgba) {
            return Ok(None);
        }
        let driver = Box::leak(Box::new(CudaDriver::load()?));
        let context = driver.create_primary_context()?;
        let converter = driver.create_nv12_to_rgba_converter(&context)?;
        let destination = driver.allocate_rgba_frame(width, height)?;
        Ok(Some(Self {
            converter,
            destination,
            _context: context,
        }))
    }

    fn convert(&self, frame: &GpuVideoFrame) -> lumen_ffmpeg::Result<()> {
        let GpuVideoFrame::Cuda(frame) = frame else {
            return Err(lumen_ffmpeg::FfmpegError::new(
                "decode_file",
                "cuda-rgba receive mode requires CUDA decoded frames",
            ));
        };
        self.converter
            .convert(frame, &self.destination)
            .map_err(|error| lumen_ffmpeg::FfmpegError::new("nv12_to_rgba8", error))
    }
}

#[cfg(not(all(feature = "cuda", target_os = "linux")))]
struct CudaRgbaState;

#[cfg(not(all(feature = "cuda", target_os = "linux")))]
impl CudaRgbaState {
    fn new(
        receive: ReceiveMode,
        _width: u32,
        _height: u32,
    ) -> Result<Option<Self>, Box<dyn std::error::Error>> {
        if matches!(receive, ReceiveMode::CudaRgba) {
            return Err(
                "cuda-rgba receive mode requires a Linux build with the cuda feature".into(),
            );
        }
        Ok(None)
    }

    fn convert(&self, _frame: &GpuVideoFrame) -> lumen_ffmpeg::Result<()> {
        Err(lumen_ffmpeg::FfmpegError::new(
            "decode_file",
            "cuda-rgba receive mode requires a Linux build with the cuda feature",
        ))
    }
}

fn millis(duration: Duration) -> u128 {
    duration.as_millis()
}

struct Usage {
    user: Duration,
    system: Duration,
    max_rss: i64,
}

fn usage() -> Usage {
    unsafe {
        let mut value = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_SELF, &mut value) != 0 {
            return Usage {
                user: Duration::ZERO,
                system: Duration::ZERO,
                max_rss: 0,
            };
        }
        Usage {
            user: timeval_to_duration(value.ru_utime),
            system: timeval_to_duration(value.ru_stime),
            max_rss: value.ru_maxrss,
        }
    }
}

fn timeval_to_duration(value: libc::timeval) -> Duration {
    Duration::new(
        value.tv_sec.max(0) as u64,
        (value.tv_usec.max(0) as u32) * 1_000,
    )
}
