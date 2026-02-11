# Stage 1: Build Rust workspace binaries
FROM rust:1.87-slim AS builder

RUN apt-get update && apt-get install -y \
    build-essential \
    pkg-config \
    curl \
    git \
    libavcodec-dev \
    libavformat-dev \
    libavutil-dev \
    libswscale-dev \
    libfontconfig1-dev \
    libfreetype6-dev && \
    rm -rf /var/lib/apt/lists/*

WORKDIR /workspace
COPY . .

ENV FFMPEG_INCLUDE_DIR=/usr/include
RUN cargo build --release -p lumen-server

# Stage 2: Runtime image with glibc-compatible ffmpeg stack
FROM ghcr.io/jrottenberg/ffmpeg:7.1-ubuntu2404

WORKDIR /
COPY --from=builder /workspace/target/release/lumen-server /lumen-server

ENTRYPOINT ["/lumen-server"]
