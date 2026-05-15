# Lumen Support Matrix

Lumen is still a heavy-beta renderer. This matrix describes what is supported today, what is experimental, and what is not expected to work yet. It is intentionally conservative: support means the path is either part of the intended native renderer surface or covered by CI.

## Summary

| Area                       | Status                 | Notes                                                                                                                                                                                                              |
| -------------------------- | ---------------------- | ------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------------ |
| Linux / Vulkan             | Supported              | Primary open native render path. CI runs on Ubuntu with Vulkan packages installed and lavapipe selected for integration tests.                                                                                     |
| macOS / Metal              | Supported              | Primary Apple render path. Metal rendering and VideoToolbox encoding are expected paths, but they are not covered by the current GitHub Actions CI workflow.                                                       |
| Windows                    | Basic development only | CI builds TypeScript packages and checks pure Rust crates. Native rendering, FFmpeg, server, and hardware encoding paths remain unsupported.                                                                       |
| NVIDIA / CUDA              | Experimental fast path | Linux CUDA/NVENC interop exists for hardware video paths. CPU-backed encoding remains the baseline fallback.                                                                                                       |
| AMD / Intel Vulkan         | Experimental           | Expected to use the Vulkan renderer without CUDA-specific interop. Coverage depends on local hardware because CI currently uses software Vulkan.                                                                   |
| Software Vulkan / lavapipe | CI-covered baseline    | CI selects lavapipe for SDK/server integration tests with `VK_ICD_FILENAMES=/usr/share/vulkan/icd.d/lvp_icd.json` and `WGPU_BACKEND=vulkan`. This validates baseline rendering behavior, not hardware performance. |
| CPU encoding               | Supported baseline     | The portable path is CPU-backed FFmpeg encoding. It is the expected fallback when hardware encoder interop is unavailable.                                                                                         |
| Hardware encoding          | Experimental           | CUDA/NVENC on Linux and VideoToolbox on macOS are the intended hardware paths. Availability depends on OS, driver, FFmpeg, and adapter support.                                                                    |

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

Hardware Vulkan rendering on AMD, Intel, and NVIDIA should use the same renderer family, but hardware-specific behavior is not fully represented by CI today.

### macOS

macOS is an intended native renderer target through Metal. Hardware video encoding is expected to use VideoToolbox where available.

The current GitHub Actions CI workflow does not run macOS jobs, so macOS support relies on local development and release validation rather than repository CI coverage.

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

CPU-backed FFmpeg encoding is the baseline portable encoder path. Hardware encoding paths are opportunistic and should fall back clearly when unavailable.

When the renderer uses Vulkan but no compatible hardware encoder interop path exists, CPU encoding is the expected fallback.

## CI Coverage

The current repository CI covers:

- Ubuntu TypeScript checks, tests, and builds.
- Rust formatting, checks, and tests.
- Generated TypeScript artifacts and definition freshness.
- SDK/server integration tests using software Vulkan through lavapipe.
- Windows TypeScript package builds and pure Rust crate checks.

The current repository CI does not cover:

- macOS / Metal.
- Native Windows rendering, FFmpeg, server, and hardware encoding paths.
- Physical NVIDIA, AMD, or Intel GPU runners.
- Hardware encoder paths.
