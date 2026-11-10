use std::f32::consts::PI;

use glam::{Vec2, Vec3};

use super::*;

/// A Radiance file of `texels`, its rows run-length encoded or flat.
fn hdr_file(width: u32, height: u32, texels: &[Vec3], encoded: bool) -> Vec<u8> {
    let mut bytes =
        format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes();
    let rgbe = |texel: Vec3| {
        let largest = texel.max_element();
        if largest < 1e-32 {
            return [0; 4];
        }
        let exponent = largest.log2().floor() as i32 + 1;
        let scale = (8 - exponent) as f32;
        let [r, g, b] = texel.to_array().map(|c| (c * scale.exp2()) as u8);
        [r, g, b, (exponent + 128) as u8]
    };
    for row in texels.chunks(width as usize) {
        let pixels: Vec<[u8; 4]> = row.iter().map(|&texel| rgbe(texel)).collect();
        if encoded {
            bytes.extend([2, 2, (width >> 8) as u8, width as u8]);
            for channel in 0..4 {
                for span in pixels.chunks(128) {
                    bytes.push(span.len() as u8);
                    bytes.extend(span.iter().map(|pixel| pixel[channel]));
                }
            }
        } else {
            bytes.extend(pixels.into_iter().flatten());
        }
    }
    bytes
}

fn close(a: Vec3, b: Vec3, tolerance: f32) -> bool {
    (a - b).abs().max_element() <= tolerance
}

#[test]
fn a_map_centres_minus_z_with_plus_x_to_its_right_and_plus_y_at_its_top() {
    assert!(close(
        direction(Vec2::new(0.5, 0.5)).normalize(),
        Vec3::NEG_Z,
        1e-6
    ));
    assert!(close(direction(Vec2::new(0.75, 0.5)), Vec3::X, 1e-6));
    assert!(close(direction(Vec2::new(0.3, 0.0)), Vec3::Y, 1e-6));
    assert!(close(direction(Vec2::new(0.3, 1.0)), Vec3::NEG_Y, 1e-6));
    assert!((uv(Vec3::NEG_Z) - Vec2::new(0.5, 0.5)).length() < 1e-6);
    assert!((uv(Vec3::X) - Vec2::new(0.75, 0.5)).length() < 1e-6);
    assert!((uv(Vec3::NEG_X) - Vec2::new(0.25, 0.5)).length() < 1e-6);
    for x in 1..16 {
        for y in 1..8 {
            let at = Vec2::new(x as f32 / 16.0, y as f32 / 8.0);
            assert!((uv(direction(at)) - at).length() < 1e-5, "{at}");
        }
    }
}

#[test]
fn a_uniform_environment_gives_uniform_irradiance_of_its_radiance() {
    let lit = prefilter(&Equirect::from_fn(64, 32, |_| Vec3::splat(2.0)));
    for normal in [Vec3::X, Vec3::NEG_Y, Vec3::new(1.0, 2.0, -3.0).normalize()] {
        assert!(
            close(lit.irradiance_at(normal), Vec3::splat(2.0), 0.01),
            "{normal}"
        );
    }
}

#[test]
fn a_sky_over_black_ground_lights_normals_by_how_far_they_face_up() {
    let sky = Equirect::from_fn(128, 64, |d| Vec3::splat(f32::from(u8::from(d.y > 0.0))));
    let lit = prefilter(&sky);
    for normal in [Vec3::Y, Vec3::NEG_Y, Vec3::X, Vec3::new(0.0, 0.6, 0.8)] {
        let expected = (1.0 + normal.y) / 2.0;
        assert!(
            close(lit.irradiance_at(normal), Vec3::splat(expected), 0.01),
            "{normal}: {}",
            lit.irradiance_at(normal)
        );
    }
}

/// A dim map with a bright patch around `centre`.
fn patch(width: u32, height: u32, centre: Vec3) -> Equirect {
    Equirect::from_fn(width, height, |d| {
        if d.dot(centre) > 0.98 {
            Vec3::new(40.0, 20.0, 10.0)
        } else {
            Vec3::splat(0.1)
        }
    })
}

#[test]
fn the_sharpest_level_is_the_map_and_rougher_ones_spread_its_light_and_keep_it() {
    let map = patch(256, 128, Vec3::new(0.3, 0.5, -0.8).normalize());
    let lit = prefilter(&map);
    assert_eq!(lit.levels.len(), SPECULAR_LEVELS as usize);
    assert_eq!(lit.levels[0], map);
    let mut peak = map.texels().iter().map(|texel| texel.x).fold(0.0, f32::max);
    for (index, level) in lit.levels.iter().enumerate().skip(1) {
        assert_eq!(level.width(), 256 >> index);
        let power = level.power();
        assert!(
            close(power / map.power(), Vec3::ONE, 0.02),
            "level {index} holds {power}, not {}",
            map.power()
        );
        let brightest = level
            .texels()
            .iter()
            .map(|texel| texel.x)
            .fold(0.0, f32::max);
        assert!(
            brightest < peak,
            "level {index} is blurrier than the one before"
        );
        peak = brightest;
    }
}

#[test]
fn shrinking_a_map_keeps_the_light_it_holds_even_near_its_poles() {
    let map = patch(64, 32, Vec3::new(0.0, 0.99, 0.14).normalize());
    let half = map.half();
    assert_eq!((half.width(), half.height()), (32, 16));
    assert!(close(half.power() / map.power(), Vec3::ONE, 1e-4));
}

#[test]
fn odd_sized_maps_keep_their_light_however_they_are_shrunk() {
    // All of the light in the last column, which a half has no column of its own for.
    let mut texels = vec![Vec3::ZERO; 513 * 2];
    texels[512] = Vec3::splat(100.0);
    texels[513 + 512] = Vec3::splat(100.0);
    let map = Equirect::new(513, 2, texels);
    for (smaller, size) in [(map.half(), (256, 1)), (map.fit(1), (1, 1))] {
        assert_eq!((smaller.width(), smaller.height()), size);
        assert!(close(smaller.power() / map.power(), Vec3::ONE, 1e-4));
    }
    let lit = prefilter(&map);
    assert!(lit.irradiance[0].x > 0.0);
    assert!(close(lit.levels[1].power() / map.power(), Vec3::ONE, 0.02));
    // One texel wide and high, and far from 2:1.
    for (width, height) in [(1, 7), (9, 1), (5, 13)] {
        let map = Equirect::from_fn(width, height, |d| Vec3::splat(1.0 + d.y + d.x));
        assert!(close(map.half().power() / map.power(), Vec3::ONE, 1e-4));
    }
    // Shrunk while read, with its last row and column only partly filling a texel.
    let (width, height) = (MAX_MAP_SIDE * 2 - 1, 5);
    let texels = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .map(|(x, y)| {
            Vec3::splat(if x == width - 1 || y == height - 1 {
                50.0
            } else {
                1.0
            })
        })
        .collect();
    let source = Equirect::new(width, height, texels);
    let read = decode(&hdr_file(width, height, source.texels(), true)).expect("reads");
    assert_eq!((read.width(), read.height()), (MAX_MAP_SIDE - 1, 2));
    assert!(close(read.power() / source.power(), Vec3::ONE, 1e-3));
}

#[test]
fn a_sun_in_one_texel_lights_by_its_power_and_extreme_texels_stay_finite() {
    let mut texels = vec![Vec3::ZERO; 512 * 256];
    // Just above the horizon in front.
    texels[120 * 512 + 256] = Vec3::splat(1e6);
    texels[0] = Vec3::new(f32::INFINITY, f32::NAN, -5.0);
    texels[1] = Vec3::splat(f32::MAX);
    let map = Equirect::new(512, 256, texels);
    assert_eq!(map.texel(0, 0), Vec3::ZERO);
    assert_eq!(map.texel(1, 0), Vec3::splat(MAX_RADIANCE));

    let lit = prefilter(&map);
    let halves = lit.levels.iter().flat_map(Equirect::rgba16f);
    assert!(
        halves
            .map(half::f16::from_bits)
            .all(|value| value.is_finite())
    );
    assert!(lit.irradiance.iter().all(|c| c.is_finite()));

    // Without the extreme texels, a surface facing the sun is lit by its power over π, with
    // the overshoot of harmonics up to band two at a single direction's peak.
    let mut texels = vec![Vec3::ZERO; 512 * 256];
    texels[120 * 512 + 256] = Vec3::splat(1e6);
    let map = Equirect::new(512, 256, texels);
    let sun = texel_direction(512, 256, 256, 120);
    let expected = map.power().x / PI * 1.0625;
    let lit = prefilter(&map).irradiance_at(sun).x;
    assert!((lit / expected - 1.0).abs() < 0.02, "{lit} vs {expected}");
}

#[test]
fn the_side_facing_away_from_a_small_sun_gets_little_of_its_light() {
    let sun = Vec3::new(0.3, 0.8, -0.5).normalize();
    let map = Equirect::from_fn(256, 128, |d| {
        Vec3::splat(if d.dot(sun) > 0.999 { 1e4 } else { 0.0 })
    });
    let lit = prefilter(&map);
    let (facing, away) = (lit.irradiance_at(sun).x, lit.irradiance_at(-sun).x);
    // Order-2 harmonics of a point light give 1/17 of the facing side to the opposite one.
    assert!(away < 0.06 * facing, "{away} vs {facing}");
}

#[test]
fn the_brdf_table_matches_the_lights_brdf_over_a_uniform_environment() {
    let table = brdf_table();
    let n = BRDF_TABLE_SIZE;
    // A mirror reflects all of the light, split between scale and bias by Fresnel.
    for (i, [scale, bias]) in table.iter().take(n).enumerate() {
        assert!(
            (scale + bias - 1.0).abs() < 0.01,
            "a mirror at cell {i}: {scale} {bias}"
        );
    }
    for (i, j) in [(31, 8), (16, 8), (4, 20), (24, 31), (1, 31)] {
        let n_dot_v = (i as f32 + 0.5) / n as f32;
        let roughness = j as f32 / (n - 1) as f32;
        let alpha = roughness * roughness;
        // The light's BRDF times n·l, summed over a grid of the hemisphere.
        let v = Vec3::new((1.0 - n_dot_v * n_dot_v).sqrt(), 0.0, n_dot_v);
        let steps = 400;
        let mut expected = [0.0_f32; 2];
        for a in 0..steps {
            let cos_l = (a as f32 + 0.5) / steps as f32;
            let sin_l = (1.0 - cos_l * cos_l).sqrt();
            for b in 0..steps {
                let phi = 2.0 * PI * (b as f32 + 0.5) / steps as f32;
                let l = Vec3::new(sin_l * phi.cos(), sin_l * phi.sin(), cos_l);
                let h = (l + v).normalize();
                let a2 = alpha * alpha;
                let d = h.z * h.z * (a2 - 1.0) + 1.0;
                let brdf = a2 / (PI * d * d) * prefilter_visibility(n_dot_v, cos_l, alpha) * cos_l;
                let fresnel = (1.0 - v.dot(h)).powi(5);
                expected[0] += (1.0 - fresnel) * brdf;
                expected[1] += fresnel * brdf;
            }
        }
        let expected = expected.map(|sum| sum * 2.0 * PI / (steps * steps) as f32);
        let [scale, bias] = table[j * n + i];
        for f0 in [0.04, 1.0] {
            let error = (f0 * scale + bias) - (f0 * expected[0] + expected[1]);
            assert!(
                error.abs() < 0.01,
                "n·v {n_dot_v}, roughness {roughness}, f0 {f0}: {error}"
            );
        }
    }
}

fn prefilter_visibility(n_dot_v: f32, n_dot_l: f32, alpha: f32) -> f32 {
    let a2 = alpha * alpha;
    let gv = n_dot_l * (n_dot_v * n_dot_v * (1.0 - a2) + a2).sqrt();
    let gl = n_dot_v * (n_dot_l * n_dot_l * (1.0 - a2) + a2).sqrt();
    0.5 / (gv + gl)
}

#[test]
fn radiance_files_read_flat_or_encoded_with_their_first_row_at_the_top() {
    let map = Equirect::from_fn(16, 8, |d| Vec3::new(d.y + 1.0, 0.5, 4.0 * (d.x + 1.0)));
    for encoded in [false, true] {
        let read = decode(&hdr_file(16, 8, map.texels(), encoded)).expect("reads");
        assert_eq!((read.width(), read.height()), (16, 8));
        for (read, written) in read.texels().iter().zip(map.texels()) {
            assert!(
                close(*read, *written, written.max_element() / 64.0),
                "{read} {written}"
            );
        }
    }
}

#[test]
fn a_file_larger_than_a_map_may_be_is_shrunk_with_its_light_kept() {
    let width = MAX_MAP_SIDE * 2;
    let source = Equirect::from_fn(width, 4, |d| Vec3::splat(1.0 + d.x.max(0.0)));
    let read = decode(&hdr_file(width, 4, source.texels(), true)).expect("reads");
    assert_eq!((read.width(), read.height()), (MAX_MAP_SIDE, 2));
    assert!(close(read.power() / source.power(), Vec3::ONE, 0.01));
}

#[test]
fn malformed_and_oversized_files_are_errors_before_their_pixels_are_read() {
    let good = hdr_file(16, 2, &[Vec3::ONE; 32], true);
    let too_large = |error| matches!(error, Err(EnvironmentError::TooLarge(_)));
    let invalid = |error| matches!(error, Err(EnvironmentError::Invalid(_)));
    let unsupported = |error| matches!(error, Err(EnvironmentError::Unsupported(_)));
    assert!(invalid(decode(&good[..good.len() - 3])), "truncated");
    assert!(too_large(decode(b"#?RADIANCE\n\n-Y 10 +X 9000\n")));
    assert!(too_large(decode(b"#?RADIANCE\n\n-Y 8000 +X 8000\n")));
    assert!(invalid(decode(b"#?RADIANCE\n\n-Y 0 +X 16\n")));
    assert!(unsupported(decode(b"#?RADIANCE\n\n+Y 2 +X 16\n")));
    assert!(unsupported(decode(
        b"#?RADIANCE\nFORMAT=32-bit_rle_xyze\n\n-Y 2 +X 16\n"
    )));
    assert!(unsupported(decode(b"\x89PNG\r\n\x1a\n")));
    let mut overrun = good.clone();
    let header = overrun.len() - (good.len() - good.iter().rposition(|&b| b == b'\n').unwrap() - 1);
    overrun[header + 4] = 200;
    assert!(invalid(decode(&overrun)), "a run past the row's end");

    // Long lines are refused without reading them through.
    let mut long_size = b"#?RADIANCE\n\n-Y 2 +X 16".to_vec();
    long_size.extend(b" 1".repeat(4 << 20));
    long_size.push(b'\n');
    assert!(invalid(decode(&long_size)), "a long size line");
    let mut long_comment = b"#?RADIANCE\n#".to_vec();
    long_comment.extend(b"x".repeat(8 << 20));
    long_comment.extend(b"\n\n-Y 2 +X 16\n");
    assert!(invalid(decode(&long_comment)), "a long comment line");
    let many_lines = [
        &b"#?RADIANCE\n"[..],
        &b"#\n".repeat(1000),
        b"\n-Y 2 +X 16\n",
    ]
    .concat();
    assert!(invalid(decode(&many_lines)), "many header lines");
    assert!(
        unsupported(decode(b"#?RADIANCE\n\n-Y 2 +X 16 +Z 1\n")),
        "a fifth field"
    );
}

#[cfg(feature = "exr")]
mod exr_files {
    use ::exr::prelude::{
        Encoding, Image, ImageAttributes, IntegerBounds, Layer, LayerAttributes, SpecificChannels,
        WritableImage,
    };
    use glam::Vec3;

    use super::super::{EnvironmentError, decode};

    type At = ::exr::math::Vec2<usize>;

    macro_rules! exr_file {
        ($image:expr) => {{
            let mut bytes = std::io::Cursor::new(Vec::new());
            $image.write().to_buffered(&mut bytes).expect("writes");
            bytes.into_inner()
        }};
    }

    fn rgb(width: usize, height: usize, pixel: fn(usize, usize) -> (f32, f32, f32)) -> Vec<u8> {
        exr_file!(Image::from_channels(
            (width, height),
            SpecificChannels::rgb(move |at: At| pixel(at.x(), at.y())),
        ))
    }

    #[test]
    fn openexr_files_read_with_unusable_values_made_dark() {
        let bytes = rgb(4, 2, |x, y| match (x, y) {
            (0, 0) => (f32::NAN, f32::INFINITY, -1.0),
            _ => (x as f32, y as f32, 0.5),
        });
        let map = decode(&bytes).expect("reads");
        assert_eq!((map.width(), map.height()), (4, 2));
        assert_eq!(map.texel(0, 0), Vec3::ZERO);
        assert_eq!(map.texel(3, 1), Vec3::new(3.0, 1.0, 0.5));
    }

    #[test]
    fn openexr_features_beyond_one_flat_rgb_image_are_refused() {
        let unsupported = |error| matches!(error, Err(EnvironmentError::Unsupported(_)));
        let layer = |name: &str| {
            Layer::new(
                (4, 4),
                LayerAttributes::named(name),
                Encoding::FAST_LOSSLESS,
                SpecificChannels::rgb(|_: At| (1.0_f32, 1.0_f32, 1.0_f32)),
            )
        };
        let parts = Image::empty(ImageAttributes::new(IntegerBounds::from_dimensions((4, 4))))
            .with_layer(layer("a"))
            .with_layer(layer("b"));
        assert!(unsupported(decode(&exr_file!(parts))), "several parts");
        let luminance = Image::from_channels(
            (4, 4),
            SpecificChannels::build()
                .with_channel("Y")
                .with_pixel_fn(|_: At| (1.0_f32,)),
        );
        assert!(unsupported(decode(&exr_file!(luminance))), "no R, G and B");

        let too_wide = rgb(9000, 1, |_, _| (1.0, 1.0, 1.0));
        assert!(matches!(
            decode(&too_wide),
            Err(EnvironmentError::TooLarge(_))
        ));
        let truncated = rgb(64, 64, |x, _| (x as f32, 1.0, 1.0));
        assert!(matches!(
            decode(&truncated[..truncated.len() / 2]),
            Err(EnvironmentError::Invalid(_))
        ));
    }
}
