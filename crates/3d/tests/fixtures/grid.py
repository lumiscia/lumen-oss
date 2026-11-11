"""Writes grid.glb: a 9 x 9 grid of vertices with normals, texture coordinates, a two-joint skin,
a clip that bends it, and a 16 x 16 RGBA texture. Run with `python3 grid.py`."""

import json
import math
import struct
import zlib

N = 9


def png(width, height, pixel):
    rows = b"".join(
        b"\0" + b"".join(bytes(pixel(x, y)) for x in range(width)) for y in range(height)
    )

    def chunk(kind, data):
        body = kind + data
        return struct.pack(">I", len(data)) + body + struct.pack(">I", zlib.crc32(body))

    header = struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0)
    return (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", header)
        + chunk(b"IDAT", zlib.compress(rows, 9))
        + chunk(b"IEND", b"")
    )


positions, normals, uvs, joints, weights = [], [], [], [], []
for j in range(N):
    for i in range(N):
        u, v = i / (N - 1), j / (N - 1)
        positions += [u * 2 - 1, v * 2, math.sin(u * 3) * 0.2]
        normal = [-math.cos(u * 3) * 0.6, 0.0, 1.0]
        length = math.sqrt(sum(c * c for c in normal))
        normals += [c / length for c in normal]
        uvs += [u, 1 - v]
        joints += [0, 1, 0, 0]
        weights += [1 - v, v, 0.0, 0.0]
indices = []
for j in range(N - 1):
    for i in range(N - 1):
        a = j * N + i
        indices += [a, a + 1, a + N, a + 1, a + N + 1, a + N]

texture = png(16, 16, lambda x, y: (x * 16, y * 16, 255 - x * 8, 128 + y * 8))
times = [0.0, 0.5, 1.0]
bend = [0.0, 0.0, math.sin(0.25), math.cos(0.25)]
rotations = [0.0, 0.0, 0.0, 1.0] + bend + [0.0, 0.0, 0.0, 1.0]
translations = [0.0, 0.0, 0.0, 0.25, 0.0, 0.0, 0.0, 0.0, 0.0]
inverse_binds = [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1]
inverse_binds += [1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 1, 0, 0, -1, 0, 1]

bin_chunk = b""
views, accessors = [], []


def add(data, component, kind, count, **extra):
    global bin_chunk
    bin_chunk += b"\0" * (-len(bin_chunk) % 4)
    views.append({"buffer": 0, "byteOffset": len(bin_chunk), "byteLength": len(data)})
    bin_chunk += data
    accessors.append(
        {"bufferView": len(views) - 1, "componentType": component, "type": kind, "count": count}
        | extra
    )
    return len(accessors) - 1


def f32(values):
    return struct.pack(f"<{len(values)}f", *values)


position = add(
    f32(positions),
    5126,
    "VEC3",
    N * N,
    min=[min(positions[k::3]) for k in range(3)],
    max=[max(positions[k::3]) for k in range(3)],
)
attributes = {
    "POSITION": position,
    "NORMAL": add(f32(normals), 5126, "VEC3", N * N),
    "TEXCOORD_0": add(f32(uvs), 5126, "VEC2", N * N),
    "JOINTS_0": add(bytes(joints), 5121, "VEC4", N * N),
    "WEIGHTS_0": add(f32(weights), 5126, "VEC4", N * N),
}
index = add(struct.pack(f"<{len(indices)}H", *indices), 5123, "SCALAR", len(indices))
binds = add(f32(inverse_binds), 5126, "MAT4", 2)
time = add(f32(times), 5126, "SCALAR", 3, min=[0.0], max=[1.0])
rotation = add(f32(rotations), 5126, "VEC4", 3)
translation = add(f32(translations), 5126, "VEC3", 3)
bin_chunk += b"\0" * (-len(bin_chunk) % 4)
views.append({"buffer": 0, "byteOffset": len(bin_chunk), "byteLength": len(texture)})
bin_chunk += texture

document = {
    "asset": {"version": "2.0", "generator": "grid.py"},
    "scene": 0,
    "scenes": [{"nodes": [0, 1]}],
    "nodes": [
        {"name": "grid", "mesh": 0, "skin": 0},
        {"name": "root", "children": [2]},
        {"name": "tip", "translation": [0, 1, 0]},
    ],
    "meshes": [
        {"primitives": [{"attributes": attributes, "indices": index, "material": 0}]}
    ],
    "skins": [{"joints": [1, 2], "inverseBindMatrices": binds}],
    "animations": [
        {
            "name": "bend",
            "samplers": [{"input": time, "output": rotation}, {"input": time, "output": translation}],
            "channels": [
                {"sampler": 0, "target": {"node": 2, "path": "rotation"}},
                {"sampler": 1, "target": {"node": 1, "path": "translation"}},
            ],
        }
    ],
    "materials": [{"name": "grid", "pbrMetallicRoughness": {"baseColorTexture": {"index": 0}}}],
    "textures": [{"source": 0}],
    "images": [{"bufferView": len(views) - 1, "mimeType": "image/png"}],
    "accessors": accessors,
    "bufferViews": views,
    "buffers": [{"byteLength": len(bin_chunk)}],
}

json_chunk = json.dumps(document, separators=(",", ":")).encode()
json_chunk += b" " * (-len(json_chunk) % 4)
bin_chunk += b"\0" * (-len(bin_chunk) % 4)
with open("grid.glb", "wb") as out:
    out.write(b"glTF" + struct.pack("<II", 2, 12 + 8 + len(json_chunk) + 8 + len(bin_chunk)))
    out.write(struct.pack("<I", len(json_chunk)) + b"JSON" + json_chunk)
    out.write(struct.pack("<I", len(bin_chunk)) + b"BIN\0" + bin_chunk)
