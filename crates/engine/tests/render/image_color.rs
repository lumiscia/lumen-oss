//! Image files' colour: embedded ICC profiles bring their colour into the working space, and
//! 16-bit samples keep their depth.

use image::{ExtendedColorType, ImageEncoder, codecs::jpeg::JpegEncoder, codecs::png::PngEncoder};
use lumen_engine::media::MediaFrame;
use moxcms::ColorProfile;
use serde_json::json;

use crate::support::{TestMedia, gpu, render, render_working_space, scene};

/// Display P3 and Adobe RGB (1998) to sRGB in linear light, as published, independent of the
/// engine's maths. Both have a D65 white.
const P3_TO_BT709: [[f64; 3]; 3] = [
    [1.224_940, -0.224_940, 0.0],
    [-0.042_057, 1.042_057, 0.0],
    [-0.019_638, -0.078_636, 1.098_274],
];
const ADOBE_TO_BT709: [[f64; 3]; 3] = [
    [1.398_283, -0.398_283, 0.0],
    [0.0, 1.0, 0.0],
    [0.0, -0.042_929, 1.042_929],
];
const ADOBE_GAMMA: f64 = 2.199_218_75;

/// One flat patch per colour, `PATCH` pixels square, in a row.
const PATCH: u32 = 16;
const PATCHES: [[u8; 3]; 8] = [
    [255, 0, 0],
    [0, 255, 0],
    [0, 0, 255],
    [255, 255, 255],
    [128, 128, 128],
    [200, 120, 40],
    [30, 160, 220],
    [10, 5, 3],
];

fn patch_pixels() -> Vec<u8> {
    let width = PATCH * PATCHES.len() as u32;
    (0..width * PATCH)
        .flat_map(|index| PATCHES[((index % width) / PATCH) as usize])
        .collect()
}

fn profile(profile: ColorProfile) -> Vec<u8> {
    profile.encode().expect("encode profile")
}

fn png(rgb: &[u8], width: u32, height: u32, icc: Option<Vec<u8>>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = PngEncoder::new(&mut bytes);
    if let Some(icc) = icc {
        encoder.set_icc_profile(icc).expect("PNG takes a profile");
    }
    encoder
        .write_image(rgb, width, height, ExtendedColorType::Rgb8)
        .expect("encode PNG");
    bytes
}

fn jpeg(rgb: &[u8], width: u32, height: u32, icc: Vec<u8>) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = JpegEncoder::new_with_quality(&mut bytes, 100);
    encoder.set_icc_profile(icc).expect("JPEG takes a profile");
    encoder
        .write_image(rgb, width, height, ExtendedColorType::Rgb8)
        .expect("encode JPEG");
    bytes
}

/// An RGB profile that converts colour only through an A2B0 lookup table.
fn lookup_table_profile() -> Vec<u8> {
    use moxcms::{LutDataType, LutStore, LutType, LutWarehouse, Matrix3d};

    let lut = LutDataType {
        num_input_channels: 3,
        num_output_channels: 3,
        num_clut_grid_points: 2,
        matrix: Matrix3d {
            v: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        },
        num_input_table_entries: 2,
        num_output_table_entries: 2,
        input_table: LutStore::Store16([0, u16::MAX].repeat(3)),
        clut_table: LutStore::Store16(
            (0..8_u16)
                .flat_map(|corner| {
                    [4, 2, 1].map(|bit| if corner & bit != 0 { u16::MAX } else { 0 })
                })
                .collect(),
        ),
        output_table: LutStore::Store16([0, u16::MAX].repeat(3)),
        lut_type: LutType::Lut16,
    };
    let mut lookup = ColorProfile::new_srgb();
    lookup.red_trc = None;
    lookup.green_trc = None;
    lookup.blue_trc = None;
    lookup.lut_a_to_b_perceptual = Some(LutWarehouse::Lut(lut));
    profile(lookup)
}

fn srgb_decode(encoded: f64) -> f64 {
    if encoded <= 0.04045 {
        encoded / 12.92
    } else {
        ((encoded + 0.055) / 1.055).powf(2.4)
    }
}

/// `image` shown at its own size, as the working space holds it.
fn working_space(file: &[u8], width: u32, height: u32) -> Vec<[f32; 4]> {
    let media = TestMedia::default().image_file("photo", file);
    let composition = scene(
        (width, height),
        json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "photo" } }]),
    )
    .into();
    render_working_space(&mut gpu().renderer(), &composition, 0.0, &media)
}

#[test]
fn profiled_patches_reach_the_working_space_values_their_profiles_describe() {
    let pixels = patch_pixels();
    let (width, height) = (PATCH * PATCHES.len() as u32, PATCH);
    let p3 = profile(ColorProfile::new_display_p3());
    let adobe = profile(ColorProfile::new_adobe_rgb());
    let cases = [
        (
            "P3 PNG",
            png(&pixels, width, height, Some(p3.clone())),
            None,
            P3_TO_BT709,
        ),
        (
            "P3 JPEG",
            jpeg(&pixels, width, height, p3),
            None,
            P3_TO_BT709,
        ),
        (
            "Adobe RGB PNG",
            png(&pixels, width, height, Some(adobe.clone())),
            Some(ADOBE_GAMMA),
            ADOBE_TO_BT709,
        ),
        (
            "Adobe RGB JPEG",
            jpeg(&pixels, width, height, adobe),
            Some(ADOBE_GAMMA),
            ADOBE_TO_BT709,
        ),
    ];
    // Each file's curve is the sRGB one, or a pure power.
    for (name, file, gamma, gamut) in cases {
        // JPEG changes the codes a little, so each patch's centre is checked against the codes
        // the file decodes to.
        let decoded = image::load_from_memory(&file).expect("decode").to_rgb8();
        let shown = working_space(&file, width, height);
        for patch in 0..PATCHES.len() as u32 {
            let (x, y) = (patch * PATCH + PATCH / 2, PATCH / 2);
            let code = decoded.get_pixel(x, y).0;
            let light = code.map(|code| {
                let encoded = f64::from(code) / 255.0;
                gamma.map_or_else(|| srgb_decode(encoded), |gamma| encoded.powf(gamma))
            });
            let expected = gamut
                .map(|row| (row[0] * light[0] + row[1] * light[1] + row[2] * light[2]).max(0.0));
            let texel = shown[(y * width + x) as usize];
            for channel in 0..3 {
                let got = f64::from(texel[channel]);
                // Half floats round to 1/2048 of a value; the profile's colorants to 1/65536.
                let tolerance = 1.5e-3 + expected[channel] * 2e-3;
                assert!(
                    (got - expected[channel]).abs() <= tolerance,
                    "{name}, patch {code:?}, channel {channel}: {got} vs {}",
                    expected[channel]
                );
            }
            assert_eq!(texel[3], 1.0, "{name}");
        }
    }
}

#[test]
fn untagged_and_srgb_tagged_images_pass_through_exactly() {
    let codes: Vec<u8> = (0..=255)
        .flat_map(|code| [code, 255 - code, code / 3])
        .collect();
    let srgb = profile(ColorProfile::new_srgb());
    for icc in [None, Some(srgb)] {
        let tagged = icc.is_some();
        let file = png(&codes, 256, 1, icc);
        let media = TestMedia::default().image_file("photo", &file);
        let composition = scene(
            (256, 1),
            json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "photo" } }]),
        )
        .into();
        let mut renderer = gpu().renderer();
        let frame = render(&mut renderer, &composition, 0.0, &media);
        let rgb: Vec<u8> = frame
            .rgba
            .chunks(4)
            .flat_map(|texel| texel[..3].to_vec())
            .collect();
        assert_eq!(rgb, codes, "tagged {tagged}");
        assert!(renderer.take_diagnostics().is_empty(), "tagged {tagged}");
        assert_eq!(renderer.cache_stats().image_textures, 256 * 4);
    }
}

#[test]
fn profiles_that_cant_be_honoured_warn_once_and_render_as_srgb() {
    let pixels = patch_pixels();
    let (width, height) = (PATCH * PATCHES.len() as u32, PATCH);
    let mut corrupt = profile(ColorProfile::new_display_p3());
    corrupt.truncate(corrupt.len() / 2);
    let untagged = working_space(&png(&pixels, width, height, None), width, height);
    for (name, icc, why) in [
        ("corrupt", corrupt, "can't be read"),
        ("lookup tables", lookup_table_profile(), "lookup tables"),
    ] {
        let file = png(&pixels, width, height, Some(icc));
        let media = TestMedia::default().image_file("photo", &file);
        let composition = scene(
            (width, height),
            json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "photo" } }]),
        )
        .frames(2)
        .into();
        let mut renderer = gpu().renderer();
        let shown = render_working_space(&mut renderer, &composition, 0.0, &media);
        assert_eq!(shown, untagged, "{name}");
        renderer
            .render_frame_at(&composition, 1.0, &media)
            .expect("frame 1 renders");
        let warnings = renderer.take_diagnostics();
        assert_eq!(warnings.len(), 1, "{name}: {warnings:?}");
        assert_eq!(warnings[0].code, "media.color_unsupported");
        assert!(
            warnings[0]
                .message
                .starts_with("media_in node 1: image `photo`: its ICC profile")
                && warnings[0].message.contains(why)
                && warnings[0].message.ends_with("so it is read as sRGB"),
            "{name}: {}",
            warnings[0].message
        );
    }
}

#[test]
fn a_16_bit_gradient_keeps_more_than_256_levels() {
    const WIDTH: u32 = 1024;
    let samples: Vec<u16> = (0..WIDTH)
        .flat_map(|x| {
            let value = (x * 64) as u16;
            [value, value, value]
        })
        .collect();
    let mut file = Vec::new();
    PngEncoder::new(&mut file)
        .write_image(
            bytemuck::cast_slice(&samples),
            WIDTH,
            1,
            ExtendedColorType::Rgb16,
        )
        .expect("encode 16-bit PNG");
    let media = TestMedia::default().image_file("ramp", &file);
    let composition = scene(
        (WIDTH, 1),
        json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "ramp" } }]),
    )
    .into();
    let mut renderer = gpu().renderer();
    let shown = render_working_space(&mut renderer, &composition, 0.0, &media);
    let mut levels: Vec<u32> = shown.iter().map(|texel| texel[0].to_bits()).collect();
    levels.dedup();
    assert!(levels.len() > 900, "{} levels", levels.len());
    for (x, texel) in shown.iter().enumerate() {
        let expected = srgb_decode(f64::from(x as u32 * 64) / 65535.0);
        assert!(
            (f64::from(texel[0]) - expected).abs() <= expected * 1e-3 + 1e-6,
            "{x}: {} vs {expected}",
            texel[0]
        );
    }
    // Half floats take 8 bytes a pixel.
    assert_eq!(renderer.cache_stats().image_textures, u64::from(WIDTH) * 8);
}

#[test]
fn eight_bit_images_keep_their_frame_and_texture_format() {
    let pixels = patch_pixels();
    let (width, height) = (PATCH * PATCHES.len() as u32, PATCH);
    let p3 = png(
        &pixels,
        width,
        height,
        Some(profile(ColorProfile::new_display_p3())),
    );
    let adobe = png(
        &pixels,
        width,
        height,
        Some(profile(ColorProfile::new_adobe_rgb())),
    );
    for (name, file, bytes_per_pixel) in [
        ("untagged", png(&pixels, width, height, None), 4),
        ("Display P3", p3, 4),
        ("Adobe RGB", adobe, 8),
    ] {
        let media = TestMedia::default().image_file("photo", &file);
        let composition = scene(
            (width, height),
            json!([{ "id": 1, "type": "media_in", "params": { "kind": "image", "source": "photo" } }]),
        )
        .into();
        let mut renderer = gpu().renderer();
        render_working_space(&mut renderer, &composition, 0.0, &media);
        assert_eq!(
            renderer.cache_stats().image_textures,
            u64::from(width * height) * bytes_per_pixel,
            "{name}"
        );
    }
    let resolver = |file: &[u8]| {
        let file_path = tempfile::NamedTempFile::new().expect("temp file");
        std::fs::write(file_path.path(), file).expect("write");
        lumen_engine::media::ImageFileResolver::open(file_path.path().to_string_lossy())
            .expect("open")
    };
    use lumen_engine::media::ImageResolver;
    let untagged = resolver(&png(&pixels, width, height, None));
    let Ok(MediaFrame::CpuRgba(frame)) = untagged.frame() else {
        panic!("an opaque untagged PNG is RGBA8");
    };
    let rgba: Vec<u8> = pixels
        .chunks(3)
        .flat_map(|rgb| [rgb[0], rgb[1], rgb[2], 255])
        .collect();
    assert_eq!(*frame.rgba, rgba);
}
