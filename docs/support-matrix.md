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
| Vulkan Video encoding      | Experimental           | `h264_vulkan` is the Linux default; `hevc_vulkan` and `av1_vulkan` are available by name. CI has no hardware encoder, so none of them is tested there. See [Vulkan encoders](#vulkan-encoders).                    |

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

A render asks for a codec (`h264`, `hevc` or `av1`, default `h264`) and a quality preset (`draft`, `standard`, `high` or `lossless`, default `standard`). Each codec tries its encoders in order, and the first that opens wins, with one warning and the reason for each that didn't:

| Platform | `h264`                              | `hevc`                              | `av1`                          |
| -------- | ----------------------------------- | ----------------------------------- | ------------------------------ |
| Linux    | `h264_vulkan`, then `libx264`       | `hevc_vulkan`, then `libx265`       | `av1_vulkan`, then `libsvtav1` |
| macOS    | `h264_videotoolbox`, then `libx264` | `hevc_videotoolbox`, then `libx265` | `libsvtav1`                    |
| Other    | `libx264`                           | `libx265`                           | `libsvtav1`                    |

A render can also ask for 10 bits per sample with `hevc` or `av1` (`bit_depth`, `--bit-depth`); the default is 8. The renderer quantises its float colour straight to 10-bit BT.709 limited range. `libx265` and `libsvtav1` encode `yuv420p10le`, and `hevc_vulkan`, `av1_vulkan` and NVENC take P010 where the device can; an AMD 780M with Mesa 25.2 encoded 10-bit `av1_vulkan`. H.264 and VideoToolbox are always 8-bit, so asking them for 10 bits is an error. An encoder that can't write 10 bits is skipped like one that won't open, never used at 8.

An operator's quality config can change the order and the presets' settings; see the [server README](../crates/server/README.md#quality-config). A preset only tries the encoders it has settings for, so `lossless` uses `libx264`, `libx265` or NVENC. `--encoder` (`lumen-local`) or `LUMEN_VIDEO_ENCODER` pins one encoder, which either opens or fails the render.

Encoder names are FFmpeg's own, such as `libx264` or `hevc_nvenc`, and are checked against your FFmpeg build before rendering starts. `*_vaapi` encoders aren't supported, and `av1_vulkan` needs FFmpeg 8.0 or newer. Falling through covers everything that stops an encoder opening: FFmpeg built without it, no Vulkan driver, a device without `VK_KHR_video_encode_queue` or without the codec, a canvas size the hardware rejects, and settings the encoder doesn't support. A failure after the encoder has opened stays an error. The output container must hold the codec; `.mp4` holds all three.

`lumen-local` and `lumen-server` render and encode `h264_vulkan` on one Vulkan device, so frames never leave the GPU. If that device can't open they warn and upload frames to the encoder instead. Each logs the encoder it opened and how frames reach it.

### Vulkan encoders

| Encoder       | Status                        | Notes                                                                                                                                                                                                           |
| ------------- | ----------------------------- | --------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------- |
| `h264_vulkan` | Linux default, experimental   | Renders and encodes on one Vulkan device. Used on NVIDIA GPUs (RTX 4090, Blackwell); the NVIDIA stall below applies.                                                                                            |
| `hevc_vulkan` | By name, untested             | Shares `h264_vulkan`'s code path. No one has run it on hardware for this project and no test renders with it.                                                                                                   |
| `av1_vulkan`  | By name, unreliable on NVIDIA | Needs FFmpeg 8.0 or newer. On an RTX 4090 most frames of the output were wrong (24 dB mean PSNR against the source) with FFmpeg 9.0.2 and master. An AMD 780M encoded it with `RADV_PERFTEST=video_encode` set. |

What they all need:

- A Vulkan device with `VK_KHR_video_encode_queue` that supports the codec, and an FFmpeg built with Vulkan. Otherwise the encoder fails to open: the render then moves to the codec's next encoder, and a pinned encoder fails the render. Distribution Mesa builds that leave out patent-encumbered codecs have no H.264 or HEVC encode. A Rocky Linux 10 box with Mesa 25.2 and an AMD 780M offered only AV1.
- Bitrate or `qp` rate control. `crf` and encoder presets aren't supported. `av1_vulkan`'s `qp` is an AV1 qindex, 0-255.
- On NVIDIA, `h264_vulkan` can stall forever at a GOP boundary when FFmpeg encodes on more than one queue. `n8.0` and `n9.0.2` do, and no other release was measured. `lumen-local` and `lumen-server` avoid it, with one encode queue when frames stay on the GPU and `async_depth=1` when they are uploaded, which is slower. FFmpeg `master` and `release/9.1` fix it.
- NVIDIA's driver limits the Vulkan contexts one process can hold, 63 on the cards measured, so `lumen-server` makes `h264_vulkan` renders wait: at most 14 when frames stay on the GPU, 9 when they are uploaded, or 6 uploaded with hardware decode. See the [server README](../crates/server/README.md).

### Hardware decode

`LUMEN_HARDWARE_DECODE=1` asks for hardware video decode. On Linux with `--features vulkan`, H.264, HEVC and AV1 decode through FFmpeg Vulkan on the renderer's own device, which needs FFmpeg 8.0 or newer, in every profile the device's driver reports it can decode: for example 10-bit HEVC as iPhones record it, 10-bit AV1, and 4:2:2 or 4:4:4 HEVC where the hardware has them. A source moves to CPU decode for the whole render, with one warning, if its hardware decoder won't open, or fails or delivers a frame the renderer can't import before any of its frames were drawn. That covers older FFmpeg, VP9, profiles the device can't decode and hardware without the codec. Once frames from the GPU were drawn, a failure stops the render with an error, because moving to the CPU then would change the source's pixels partway through. Decoded frames convert to RGBA on the GPU, following the stream's colour tags and display rotation: BT.2020 and P3 primaries convert to BT.709, and PQ and HLG (HDR) sources are tone-mapped to SDR, on every decode path (see the [engine README](../crates/engine/README.md#colour)). Sources with an alpha channel always decode on the CPU, also with one warning.

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
