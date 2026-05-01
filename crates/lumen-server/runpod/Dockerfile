FROM rust:1.90-bookworm AS builder
WORKDIR /workspace
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        clang \
        cmake \
        pkg-config \
        python3 \
        libavcodec-dev \
        libavdevice-dev \
        libavfilter-dev \
        libavformat-dev \
        libavutil-dev \
        libswresample-dev \
        libswscale-dev \
        libvulkan-dev \
        vulkan-headers \
    && rm -rf /var/lib/apt/lists/*
COPY lumen ./lumen
COPY rust-skia ./rust-skia
WORKDIR /workspace/lumen
RUN cargo build --release --bin lumen-runpod --features vulkan

FROM python:3.12-slim-bookworm
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg libavcodec59 libavdevice59 libavfilter8 libavformat59 libavutil57 libswresample4 libswscale6 libvulkan1 mesa-vulkan-drivers vulkan-tools \
    && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY lumen/crates/lumen-server/runpod/requirements.txt .
RUN pip install --no-cache-dir -r requirements.txt
COPY lumen/crates/lumen-server/runpod/handler.py .
COPY --from=builder /workspace/lumen/target/release/lumen-runpod /usr/local/bin/lumen-runpod
CMD ["python", "/app/handler.py"]
