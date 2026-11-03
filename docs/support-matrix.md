# Lumen Support Matrix

Lumen is still a heavy-beta renderer. This matrix describes what is supported today, what is experimental, and what is not expected to work yet. It is intentionally conservative: support means the path is either part of the intended native renderer surface or covered by CI.

## Summary

| Area                       | Status                 | Notes                                                                                                                                                                                                              |
| -------------------------- | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Linux / Vulkan             | Supported              | Primary open native render path. CI runs on Ubuntu with Vulkan packages installed and lavapipe selected for integration tests.                                                                                     |
| macOS / Metal              | Supported              | Primary Apple render path. Metal rendering and VideoToolbox encoding are expected paths, but CI only compiles them; it does not run them.                                                                          |
| Windows                    | Basic development only | CI builds TypeScript packages and checks pure Rust crates. Native rendering, FFmpeg, server, and hardware encoding paths remain unsupported.                                                                       |
| NVIDIA / CUDA              | Experimental fast path | Linux CUDA/NVENC interop exists for hardware video paths. CPU-backed encoding remains the baseline fallback.                                                                                                       |
| AMD / Intel Vulkan         | Experimental           | Expected to use the Vulkan renderer without CUDA-specific interop. Coverage depends on local hardware because CI currently uses software Vulkan.                                                                   |
| Software Vulkan / lavapipe | CI-covered baseline    | CI selects lavapipe for SDK/server integration tests with `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` and `WGPU_BACKEND=vulkan`. This validates baseline rendering behavior, not hardware performance. |
| CPU encoding               | Supported baseline     | The portable path is CPU-backed FFmpeg encoding with `libx264`. It is what the Linux default falls back to when `h264_vulkan` won't open.                                                                          |
| Hardware encoding          | Experimental           | `h264_vulkan` is the Linux default and VideoToolbox the macOS default. CUDA/NVENC is available by name. Availability depends on OS, driver, FFmpeg, and adapter support.                                           |

## Operating Systems

### Linux

Linux with Vulkan is the most visible open development path. The CI workflow installs FFmpeg development libraries, Vulkan loader/tools, Mesa Vulkan drivers, and runs TypeScript, Rust, generated-definition, and SDK/server integration checks.

Known required packages on Debian/Ubuntu-style systems:

- `libavcodec-dev`
- `libavdevice-dev`
- `libavfilter-dev`
- `libavformat-dev`
- `libavutil-dev`
- `libswresample-dev`
- `libswscale-dev`
- `libvulkan1`
- `mesa-vulkan-drivers`
- `pkg-config`
- `vulkan-tools`

Native audio decodes in-process through the same FFmpeg libraries as video, so no `ffmpeg` executable is needed at runtime. Tests that generate fixtures with it skip when it is missing.

Hardware Vulkan rendering on AMD, Intel, and NVIDIA should use the same renderer family, but hardware-specific behavior is not fully represented by CI today.

### macOS

macOS is an intended native renderer target through Metal. Hardware video encoding is expected to use VideoToolbox where available.

The macOS CI job only compiles `lumen-server` and `lumen-local` with the `metal` feature. Nothing renders or encodes on macOS in CI, so runtime behaviour relies on local development and release validation.

### Windows

Windows is a basic development target for TypeScript package builds and pure Rust crates. A `windows-latest` CI job covers those paths, while native rendering, FFmpeg-backed media, `lumen-server`, WASM generation scripts, and hardware encoding remain unsupported. See [windows.md](windows.md) for setup instructions and the exact CI scope.

## GPU And Adapter Paths

### NVIDIA / CUDA

NVIDIA on Linux has the clearest hardware fast path through CUDA/NVENC interop. Treat this as experimental until your exact driver, FFmpeg build, and adapter combination has been validated.

### AMD / Intel Vulkan

AMD and Intel adapters should use the Vulkan renderer path without CUDA-specific interop. This path should be kept reliable, but it is still experimental until basic render tests are regularly run against non-NVIDIA hardware.

### Software Vulkan

Software Vulkan through lavapipe is the deterministic CI baseline. It is useful for catching portability issues and running integration tests on GitHub-hosted Linux runners. It is not a performance target.

## Encoding

`lumen-local` and `lumen-server` pick their encoder from `--encoder` (`lumen-local`) or the render options (`lumen-server`), then `LUMEN_VIDEO_ENCODER`, then the platform default:

| Platform | Default             | If it won't open                                       |
| -------- | ------------------- | ------------------------------------------------------ |
| Linux    | `h264_vulkan`       | One warning with the reason, then `libx264` on the CPU |
| macOS    | `h264_videotoolbox` | The render fails                                       |
| Other    | `libx264`           | The render fails                                       |

Only the automatic default falls back. An encoder you name either opens or fails the render, and `av1_vulkan` needs FFmpeg 8.0 or newer. The fallback covers everything that stops the encoder opening: FFmpeg built without `h264_vulkan`, no Vulkan driver, a device without `VK_KHR_video_encode_queue`, no H.264 encode support, a canvas size the hardware rejects, and `--crf` or `--preset`, which `h264_vulkan` doesn't support. A failure after the encoder has opened stays an error.

`lumen-local` renders and encodes `h264_vulkan` on one Vulkan device, so frames never leave the GPU. `lumen-server` reads frames back and uploads them to the encoder. Each logs the encoder it opened and how frames reach it.

### Hardware decode

`LUMEN_HARDWARE_DECODE=1` asks for hardware video decode. On Linux with `--features vulkan`, H.264, HEVC and AV1 at 8-bit 4:2:0 decode through FFmpeg Vulkan on the renderer's own device, which needs FFmpeg 8.0 or newer. A source moves to CPU decode for the rest of the render, with one warning, if its hardware decoder won't open, fails while decoding, or delivers a frame the renderer can't import. That covers older FFmpeg, VP9, 10-bit, other pixel formats and hardware without the codec. Rotated sources and sources with an alpha channel always decode on the CPU, also with one warning.

## CI Coverage

The current repository CI covers:

- Ubuntu TypeScript checks, tests, and builds.
- Rust formatting, checks, and tests.
- Generated TypeScript artifacts and definition freshness.
- SDK/server integration tests using software Vulkan through lavapipe.
- Windows TypeScript package builds and pure Rust crate checks.
- A macOS compile check of the Metal feature, without running tests.
- A compile check of the hosted CUDA and Vulkan server build.

The current repository CI does not cover:

- Running anything on macOS / Metal.
- Native Windows rendering, FFmpeg, server, and hardware encoding paths.
- Physical NVIDIA, AMD, or Intel GPU runners.
- Hardware encoder paths.
