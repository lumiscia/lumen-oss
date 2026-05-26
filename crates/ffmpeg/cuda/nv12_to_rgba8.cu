// NV12 (device linear) -> RGBA8 conversion for Vulkan/CUDA interop and NVENC paths.
// Compiled to PTX by crates/ffmpeg/build.rs (nvcc) when the cuda feature is enabled on Linux.

extern "C" __global__ void nv12_to_rgba8(
    const unsigned char* __restrict__ src,
    unsigned char* __restrict__ dst,
    unsigned int src_pitch,
    unsigned int dst_pitch,
    unsigned int width,
    unsigned int height)
{
    const unsigned int x = blockIdx.x * blockDim.x + threadIdx.x;
    const unsigned int y = blockIdx.y * blockDim.y + threadIdx.y;
    if (x >= width || y >= height) {
        return;
    }

    const unsigned int y_index = y * src_pitch + x;
    const unsigned char y_value = src[y_index];

    const unsigned int chroma_y = y >> 1;
    const unsigned int chroma_x = x & ~1u;
    const unsigned int uv_base = src_pitch * height;
    const unsigned int uv_index = uv_base + chroma_y * src_pitch + chroma_x;
    const unsigned char u_value = src[uv_index];
    const unsigned char v_value = src[uv_index + 1];

    int y_scaled = static_cast<int>(y_value) - 16;
    if (y_scaled < 0) {
        y_scaled = 0;
    }
    const int u_scaled = static_cast<int>(u_value) - 128;
    const int v_scaled = static_cast<int>(v_value) - 128;

    int r = ((298 * y_scaled) + (409 * v_scaled) + 128) >> 8;
    int g = ((298 * y_scaled) - (100 * u_scaled) - (208 * v_scaled) + 128) >> 8;
    int b = ((298 * y_scaled) + (516 * u_scaled) + 128) >> 8;

    r = r < 0 ? 0 : (r > 255 ? 255 : r);
    g = g < 0 ? 0 : (g > 255 ? 255 : g);
    b = b < 0 ? 0 : (b > 255 ? 255 : b);

    const unsigned int rgba =
        0xff000000u | (static_cast<unsigned int>(b) << 16) |
        (static_cast<unsigned int>(g) << 8) | static_cast<unsigned int>(r);

    *reinterpret_cast<unsigned int*>(dst + y * dst_pitch + (x << 2)) = rgba;
}
