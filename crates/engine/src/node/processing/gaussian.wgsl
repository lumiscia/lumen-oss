// Shared separable Gaussian blur. Each pass blurs along one axis with
// linear-sampled tap pairs: two adjacent Gaussian weights collapse into one
// bilinear fetch, halving the texture reads.
//
// Positions are in texel units where texel `i` has its center at `i + 0.5`.

const GAUSSIAN_MIN_SIGMA: f32 = 0.001;
const GAUSSIAN_EXTENT_SIGMAS: f32 = 3.0;
const GAUSSIAN_MIN_PAIR_WEIGHT: f32 = 1e-30;

fn gaussian_edge_inside(index: f32, extent: f32) -> f32 {
    return select(0.0, 1.0, index >= 0.0 && index < extent);
}

// Fraction of a bilinear footprint at `pos` that lies inside the texture.
fn gaussian_coverage(pos: vec2<f32>, dims: vec2<f32>) -> f32 {
    let p = pos - vec2<f32>(0.5);
    let base = floor(p);
    let f = p - base;
    let x = mix(gaussian_edge_inside(base.x, dims.x), gaussian_edge_inside(base.x + 1.0, dims.x), f.x);
    let y = mix(gaussian_edge_inside(base.y, dims.y), gaussian_edge_inside(base.y + 1.0, dims.y), f.y);
    return x * y;
}

fn gaussian_load_premultiplied(tex: texture_2d<f32>, coord: vec2<f32>, dims: vec2<f32>) -> vec4<f32> {
    let clamped = clamp(coord, vec2<f32>(0.0), dims - vec2<f32>(1.0));
    return premultiply(textureLoad(tex, vec2<i32>(clamped), 0));
}

// Bilinear fetch. Straight-alpha sources are premultiplied before
// interpolating, so transparent texels do not drag their color into the edge.
// Texels outside the texture clamp to the edge, or read as transparent when
// `transparent_edge` is set.
fn gaussian_tap(
    tex: texture_2d<f32>,
    samp: sampler,
    pos: vec2<f32>,
    straight: bool,
    transparent_edge: bool,
) -> vec4<f32> {
    let dims = vec2<f32>(textureDimensions(tex));
    var color: vec4<f32>;
    if (straight) {
        let p = pos - vec2<f32>(0.5);
        let base = floor(p);
        let f = p - base;
        let top = mix(
            gaussian_load_premultiplied(tex, base, dims),
            gaussian_load_premultiplied(tex, base + vec2<f32>(1.0, 0.0), dims),
            f.x,
        );
        let bottom = mix(
            gaussian_load_premultiplied(tex, base + vec2<f32>(0.0, 1.0), dims),
            gaussian_load_premultiplied(tex, base + vec2<f32>(1.0, 1.0), dims),
            f.x,
        );
        color = mix(top, bottom, f.y);
    } else {
        color = textureSampleLevel(tex, samp, pos / dims, 0.0);
    }
    if (transparent_edge) {
        color = color * gaussian_coverage(pos, dims);
    }
    return color;
}

// One normalized Gaussian pass of standard deviation `sigma` along `axis`,
// truncated at three sigma. The result is premultiplied when the input is.
fn gaussian_blur_1d(
    tex: texture_2d<f32>,
    samp: sampler,
    center: vec2<f32>,
    axis: vec2<f32>,
    sigma: f32,
    straight: bool,
    transparent_edge: bool,
) -> vec4<f32> {
    var acc = gaussian_tap(tex, samp, center, straight, transparent_edge);
    if (sigma < GAUSSIAN_MIN_SIGMA) {
        return acc;
    }

    let falloff = -0.5 / (sigma * sigma);
    let extent = ceil(sigma * GAUSSIAN_EXTENT_SIGMAS);
    var total = 1.0;
    for (var i = 1.0; i <= extent; i = i + 2.0) {
        let w0 = exp(falloff * i * i);
        let w1 = exp(falloff * (i + 1.0) * (i + 1.0));
        let weight = w0 + w1;
        if (weight < GAUSSIAN_MIN_PAIR_WEIGHT) {
            break;
        }
        let offset = axis * ((i * w0 + (i + 1.0) * w1) / weight);
        acc = acc + weight * (
            gaussian_tap(tex, samp, center + offset, straight, transparent_edge)
            + gaussian_tap(tex, samp, center - offset, straight, transparent_edge)
        );
        total = total + 2.0 * weight;
    }
    return acc / total;
}
