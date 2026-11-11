//! Embedded ICC profiles, read with `moxcms`: how an image's samples become linear light in the
//! working space's BT.709 primaries. Matrix/TRC RGB profiles and grey profiles are read; any
//! other profile is read as sRGB, and why is reported for a warning.

use half::{f16, vec::HalfBitsVecExt};
use moxcms::{
    ColorProfile, DataColorSpace, ParsingOptions, ProfileClass, ToneCurveEvaluator, ToneReprCurve,
};

use super::yuv::icc_colorants_to_bt709;
use crate::gpu::color::{linear_to_srgb, srgb_to_linear};

/// The largest profile read. Matrix/TRC profiles take a few KiB; ones this large carry lookup
/// tables, which aren't read.
pub(crate) const MAX_PROFILE_BYTES: usize = 1 << 20;

/// How far each element of a profile's matrix to BT.709 may be from the identity for its
/// primaries to be read as BT.709's. sRGB profiles in circulation, such as HP's IEC 61966-2.1,
/// are about 3e-4 from it, from rounding and their own adaptation to D50.
const BT709_TOLERANCE: f64 = 1e-3;

/// The largest magnitude an element of a profile's matrix to BT.709 may have. ACES AP0, the
/// widest primaries in common use, needs 2.52 and ProPhoto 2.03; a profile past this doesn't
/// describe a real RGB space, and colour within `0..=1` stays within 24 after the matrix.
const MAX_GAMUT_ELEMENT: f64 = 8.0;

/// The largest finite half float, which linear light is clipped to.
const HALF_MAX: f32 = 65504.0;

type Curve = Box<dyn ToneCurveEvaluator + Send + Sync>;

/// How an image's samples become the working space's linear BT.709.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ImageColor {
    /// The red, green and blue curves from samples to linear light, or `None` for the sRGB curve.
    pub(crate) curves: Option<[ToneReprCurve; 3]>,
    /// Linear RGB in the image's primaries to linear BT.709, by rows, or `None` for BT.709.
    pub(crate) gamut: Option<[[f64; 3]; 3]>,
}

impl ImageColor {
    pub(crate) const SRGB: Self = Self {
        curves: None,
        gamut: None,
    };

    /// The colour `profile` describes for an image whose samples are grey when `grey`, or why it
    /// is read as sRGB instead. A profile that matches sRGB is `SRGB`.
    pub(crate) fn of_profile(profile: &[u8], grey: bool) -> Result<Self, String> {
        if profile.len() > MAX_PROFILE_BYTES {
            return Err(format!(
                "its ICC profile is {} KiB, past the {} KiB Lumen reads, so it is read as sRGB",
                profile.len() >> 10,
                MAX_PROFILE_BYTES >> 10
            ));
        }
        // The profile is untrusted: a parser that panics on it only leaves the image sRGB.
        std::panic::catch_unwind(|| read(profile, grey)).unwrap_or_else(|_| {
            Err("its ICC profile can't be read, so it is read as sRGB".to_string())
        })
    }

    /// `rgba`, 16-bit samples in this colour, straight unless `premultiplied`, as straight linear
    /// BT.709 half floats in the same buffer. Colour outside BT.709 is clipped at zero and may be
    /// above 1.
    pub(crate) fn to_linear(&self, mut rgba: Vec<u16>, premultiplied: bool) -> Vec<f16> {
        let tables = self.tables();
        let gamut = self.gamut_f32();
        in_bands(&mut rgba, |_, texels| {
            for texel in texels.as_chunks_mut::<4>().0 {
                convert_texel(texel, &tables, gamut.as_ref(), premultiplied);
            }
        });
        rgba.reinterpret_into()
    }

    /// `rgba`, float samples in this colour, straight unless `premultiplied`, as straight linear
    /// BT.709 half floats. A sample that isn't finite is 0, a negative one is clipped at 0, and
    /// one above 1 keeps its light, up to the largest half float.
    pub(crate) fn float_to_linear(&self, rgba: &[f32], premultiplied: bool) -> Vec<f16> {
        let curves = self.curves.as_ref().map(|curves| {
            curves
                .each_ref()
                .map(|curve| curve.make_linear_evaluator().ok())
        });
        let gamut = self.gamut_f32();
        let mut linear = vec![f16::ZERO; rgba.len()];
        in_bands(&mut linear, |start, texels| {
            let source = &rgba[start..start + texels.len()];
            for (texel, source) in texels
                .as_chunks_mut::<4>()
                .0
                .iter_mut()
                .zip(source.as_chunks::<4>().0)
            {
                *texel = float_texel(*source, curves.as_ref(), gamut.as_ref(), premultiplied);
            }
        });
        linear
    }

    fn gamut_f32(&self) -> Option<[[f32; 3]; 3]> {
        self.gamut
            .map(|rows| rows.map(|row| row.map(|value| value as f32)))
    }

    /// Each channel's linear light for every 16-bit sample.
    fn tables(&self) -> [std::sync::Arc<[f32]>; 3] {
        let table = |curve: Option<&ToneReprCurve>| -> std::sync::Arc<[f32]> {
            let evaluator = curve.and_then(|curve| curve.make_linear_evaluator().ok());
            (0..=u16::MAX)
                .map(|sample| {
                    let encoded = f32::from(sample) / 65535.0;
                    match &evaluator {
                        Some(evaluator) => linear_light(evaluator.evaluate_value(encoded)),
                        None => srgb_to_linear(encoded),
                    }
                })
                .collect()
        };
        let Some([red, green, blue]) = &self.curves else {
            let srgb = table(None);
            return [srgb.clone(), srgb.clone(), srgb];
        };
        let red_table = table(Some(red));
        let green_table = if green == red {
            red_table.clone()
        } else {
            table(Some(green))
        };
        let blue_table = if blue == red {
            red_table.clone()
        } else if blue == green {
            green_table.clone()
        } else {
            table(Some(blue))
        };
        [red_table, green_table, blue_table]
    }
}

/// One texel of 16-bit samples as straight linear BT.709 half-float bits, in place.
fn convert_texel(
    texel: &mut [u16; 4],
    tables: &[std::sync::Arc<[f32]>; 3],
    gamut: Option<&[[f32; 3]; 3]>,
    premultiplied: bool,
) {
    let alpha = texel[3];
    let divided = premultiplied && alpha != u16::MAX;
    let mut light = [0.0_f32; 3];
    for (channel, light) in light.iter_mut().enumerate() {
        let sample = if !divided {
            usize::from(texel[channel])
        } else if alpha == 0 {
            0
        } else {
            let straight = (f32::from(texel[channel]) / f32::from(alpha)).min(1.0);
            (straight * 65535.0 + 0.5) as usize
        };
        *light = tables[channel][sample];
    }
    if let Some(gamut) = gamut {
        light =
            gamut.map(|row| (row[0] * light[0] + row[1] * light[1] + row[2] * light[2]).max(0.0));
    }
    for (sample, light) in texel.iter_mut().zip(light) {
        *sample = f16::from_f32(light).to_bits();
    }
    texel[3] = f16::from_f32(f32::from(alpha) / 65535.0).to_bits();
}

/// One texel of float samples as straight linear BT.709 half floats. `curves` are each channel's,
/// `None` for the sRGB curve, and a channel whose curve can't be evaluated takes the sRGB one.
fn float_texel(
    source: [f32; 4],
    curves: Option<&[Option<Curve>; 3]>,
    gamut: Option<&[[f32; 3]; 3]>,
    premultiplied: bool,
) -> [f16; 4] {
    let alpha = finite_or_zero(source[3]).clamp(0.0, 1.0);
    let mut light = [0.0_f32; 3];
    for (channel, light) in light.iter_mut().enumerate() {
        let mut sample = finite_or_zero(source[channel]).max(0.0);
        if premultiplied {
            sample = if alpha > 0.0 { sample / alpha } else { 0.0 };
        }
        let curve = curves.and_then(|curves| curves[channel].as_ref());
        let value = match curve {
            Some(curve) => curve.evaluate_value(sample),
            None => srgb_to_linear(sample),
        };
        // A finite sample whose light overflows is as bright as a half float holds.
        *light = if value.is_nan() {
            0.0
        } else {
            value.clamp(0.0, HALF_MAX)
        };
    }
    if let Some(gamut) = gamut {
        light =
            gamut.map(|row| (row[0] * light[0] + row[1] * light[1] + row[2] * light[2]).max(0.0));
    }
    [light[0], light[1], light[2], alpha].map(|value| f16::from_f32(value.min(HALF_MAX)))
}

fn finite_or_zero(value: f32) -> f32 {
    if value.is_finite() { value } else { 0.0 }
}

/// Runs `convert` on `samples` in bands of whole texels, with each band's offset, a band to a
/// thread where there are threads.
fn in_bands<T: Send>(samples: &mut [T], convert: impl Fn(usize, &mut [T]) + Sync) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map_or(1, usize::from);
        let band = (samples.len() / 4).div_ceil(threads).max(1) * 4;
        let convert = &convert;
        std::thread::scope(|scope| {
            for (index, texels) in samples.chunks_mut(band).enumerate() {
                scope.spawn(move || convert(index * band, texels));
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    convert(0, samples);
}

/// A curve's output as linear light: within `0..=1`, and 0 where it isn't a number.
fn linear_light(value: f32) -> f32 {
    if value.is_nan() {
        0.0
    } else {
        value.clamp(0.0, 1.0)
    }
}

fn read(bytes: &[u8], grey: bool) -> Result<ImageColor, String> {
    let options = ParsingOptions {
        max_profile_size: MAX_PROFILE_BYTES + 1,
        ..ParsingOptions::default()
    };
    let profile = ColorProfile::new_from_slice_with_options(bytes, options).map_err(|error| {
        format!("its ICC profile can't be read ({error}), so it is read as sRGB")
    })?;
    if matches!(
        profile.profile_class,
        ProfileClass::DeviceLink | ProfileClass::Abstract | ProfileClass::Named
    ) {
        return Err(format!(
            "its ICC profile is a {:?} profile, not one of an image's colour, so it is read as \
             sRGB",
            profile.profile_class
        ));
    }
    // A CMM uses a profile's A2B tags in preference to its matrix and curves, which are then at
    // most an approximation of them.
    if profile.lut_a_to_b_perceptual.is_some()
        || profile.lut_a_to_b_colorimetric.is_some()
        || profile.lut_a_to_b_saturation.is_some()
    {
        return Err(unsupported_lookup());
    }
    let (curves, gamut) = match (profile.color_space, grey) {
        (DataColorSpace::Rgb, false) => {
            let (Some(red), Some(green), Some(blue)) =
                (&profile.red_trc, &profile.green_trc, &profile.blue_trc)
            else {
                return Err(unsupported_lookup());
            };
            if !profile.is_matrix_shaper() || profile.pcs != DataColorSpace::Xyz {
                return Err(unsupported_lookup());
            }
            let gamut = icc_colorants_to_bt709(profile.colorant_matrix().v);
            if !gamut
                .iter()
                .flatten()
                .all(|value| value.is_finite() && value.abs() <= MAX_GAMUT_ELEMENT)
            {
                return Err(
                    "its ICC profile's primaries aren't those of a real RGB space, so it is read \
                     as sRGB"
                        .to_string(),
                );
            }
            ([red.clone(), green.clone(), blue.clone()], gamut)
        }
        (DataColorSpace::Gray, true) => {
            let Some(grey) = &profile.gray_trc else {
                return Err(
                    "its grey ICC profile has no tone curve, so it is read as sRGB".to_string(),
                );
            };
            let identity = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
            ([grey.clone(), grey.clone(), grey.clone()], identity)
        }
        (space, grey) => {
            return Err(format!(
                "its ICC profile is for {} colour, but the image is {}, so it is read as sRGB",
                color_space_name(space),
                if grey { "grey" } else { "RGB" }
            ));
        }
    };
    let mut srgb_curves = true;
    for curve in &curves {
        let evaluator = curve.make_linear_evaluator().map_err(|_| {
            "its ICC profile's tone curves can't be evaluated, so it is read as sRGB".to_string()
        })?;
        srgb_curves &= (0..=255_u8).all(|code| {
            let encoded = f32::from(code) / 255.0;
            let light = linear_light(evaluator.evaluate_value(encoded));
            (linear_to_srgb(light) - encoded).abs() * 255.0 < 0.5
        });
    }
    let bt709 = gamut.iter().enumerate().all(|(row, values)| {
        values.iter().enumerate().all(|(column, value)| {
            let identity = if row == column { 1.0 } else { 0.0 };
            (value - identity).abs() < BT709_TOLERANCE
        })
    });
    Ok(ImageColor {
        curves: (!srgb_curves).then_some(curves),
        gamut: (!bt709).then_some(gamut),
    })
}

fn unsupported_lookup() -> String {
    "its ICC profile converts colour with lookup tables, which Lumen doesn't read, so it is read \
     as sRGB"
        .to_string()
}

fn color_space_name(space: DataColorSpace) -> String {
    match space {
        DataColorSpace::Rgb => "RGB".to_string(),
        DataColorSpace::Gray => "grey".to_string(),
        DataColorSpace::Cmyk => "CMYK".to_string(),
        other => format!("{other:?}"),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use moxcms::{LutDataType, LutStore, LutType, LutWarehouse, Matrix3d, ToneReprCurve};

    use super::*;
    use crate::media::ColorPrimaries;

    /// Adobe RGB (1998) to sRGB in linear light, as published, independent of this module's maths.
    pub(crate) const ADOBE_TO_BT709: [[f64; 3]; 3] = [
        [1.398_283, -0.398_283, 0.0],
        [0.0, 1.0, 0.0],
        [0.0, -0.042_928_6, 1.042_929],
    ];
    /// Adobe RGB's tone curve, a pure power.
    pub(crate) const ADOBE_GAMMA: f32 = 2.199_218_8;

    pub(crate) fn encoded(profile: ColorProfile) -> Vec<u8> {
        profile.encode().expect("encode profile")
    }

    pub(crate) fn display_p3() -> Vec<u8> {
        encoded(ColorProfile::new_display_p3())
    }

    pub(crate) fn adobe_rgb() -> Vec<u8> {
        encoded(ColorProfile::new_adobe_rgb())
    }

    /// An RGB profile that converts colour only through an A2B0 lookup table.
    pub(crate) fn lookup_table_profile() -> Vec<u8> {
        let mut profile = with_lookup_table(ColorProfile::new_srgb());
        profile.red_trc = None;
        profile.green_trc = None;
        profile.blue_trc = None;
        encoded(profile)
    }

    /// `profile` with an A2B0 lookup table that keeps each colour, beside its matrix and curves.
    pub(crate) fn with_lookup_table(mut profile: ColorProfile) -> ColorProfile {
        let identity = Matrix3d {
            v: [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
        };
        let lut = LutDataType {
            num_input_channels: 3,
            num_output_channels: 3,
            num_clut_grid_points: 2,
            matrix: identity,
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
        profile.lut_a_to_b_perceptual = Some(LutWarehouse::Lut(lut));
        profile
    }

    fn max_difference(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> f64 {
        a.iter()
            .flatten()
            .zip(b.iter().flatten())
            .map(|(a, b)| (a - b).abs())
            .fold(0.0, f64::max)
    }

    #[test]
    fn srgb_profiles_read_as_srgb() {
        // The parametric curve, and the 1024-entry table older sRGB profiles carry.
        let mut table = ColorProfile::new_srgb();
        let curve = ToneReprCurve::Lut(
            (0..1024)
                .map(|index| (srgb_to_linear(index as f32 / 1023.0) * 65535.0 + 0.5) as u16)
                .collect(),
        );
        table.red_trc = Some(curve.clone());
        table.green_trc = Some(curve.clone());
        table.blue_trc = Some(curve);
        for profile in [encoded(ColorProfile::new_srgb()), encoded(table)] {
            assert_eq!(
                ImageColor::of_profile(&profile, false),
                Ok(ImageColor::SRGB)
            );
        }
    }

    #[test]
    fn display_p3_keeps_the_srgb_curve_and_converts_its_primaries() {
        let color = ImageColor::of_profile(&display_p3(), false).expect("read P3");
        assert!(color.curves.is_none());
        let gamut = color.gamut.expect("P3 is wider than BT.709");
        // The profile's colorants are quantised to 1/65536 and were adapted to D50 by its
        // writer, whose rounding differs slightly from this one's: under 0.1% of a channel.
        let error = max_difference(gamut, ColorPrimaries::DisplayP3.to_bt709());
        assert!(error < 1e-3, "{gamut:?}: {error}");
    }

    #[test]
    fn adobe_rgb_converts_its_curve_and_primaries_on_the_cpu() {
        let color = ImageColor::of_profile(&adobe_rgb(), false).expect("read Adobe RGB");
        assert!(color.curves.is_some());
        let error = max_difference(color.gamut.expect("wider"), ADOBE_TO_BT709);
        assert!(error < 1e-3, "{error}");
        let samples = [0_u16, 8192, 32768, 65535];
        let rgba: Vec<u16> = samples
            .iter()
            .flat_map(|&value| [value, 65535 - value, value / 2, 65535])
            .collect();
        let linear = color.to_linear(rgba.clone(), false);
        for (texel, source) in linear
            .as_chunks::<4>()
            .0
            .iter()
            .zip(rgba.as_chunks::<4>().0)
        {
            let light = [0, 1, 2].map(|c| (f32::from(source[c]) / 65535.0).powf(ADOBE_GAMMA));
            for (channel, row) in ADOBE_TO_BT709.iter().enumerate() {
                let expected = (row[0] as f32 * light[0]
                    + row[1] as f32 * light[1]
                    + row[2] as f32 * light[2])
                    .max(0.0);
                let got = texel[channel].to_f32();
                assert!(
                    (got - expected).abs() <= 2e-3 * expected.max(0.05),
                    "{source:?} channel {channel}: {got} vs {expected}"
                );
            }
            assert_eq!(texel[3].to_f32(), 1.0);
        }
    }

    #[test]
    fn premultiplied_samples_are_divided_before_their_curve() {
        let half_red = [32768_u16, 0, 0, 32768];
        let linear = ImageColor::SRGB.to_linear(half_red.to_vec(), true);
        assert_eq!(linear[0].to_f32(), 1.0);
        assert!((linear[3].to_f32() - 0.5).abs() < 1e-3);
    }

    #[test]
    fn profiles_that_cant_be_honoured_read_as_srgb_and_say_why() {
        let p3 = display_p3();
        let mut cmyk = p3.clone();
        cmyk[16..20].copy_from_slice(b"CMYK");
        let mut grey = encoded(ColorProfile::new_gray_with_gamma(2.2));
        let mut too_many_tags = p3.clone();
        too_many_tags[128..132].copy_from_slice(&u32::MAX.to_be_bytes());
        let mut nan_curve = ColorProfile::new_srgb();
        nan_curve.red_trc = Some(ToneReprCurve::Parametric(vec![f32::NAN]));
        // The largest colorant the profile's fixed point holds, which takes red past half floats.
        let mut huge_red = ColorProfile::new_display_p3();
        huge_red.red_colorant.x = 30000.0;
        let cases: [(&str, Vec<u8>, bool); 12] = [
            ("empty", Vec::new(), false),
            ("truncated header", p3[..64].to_vec(), false),
            ("truncated tags", p3[..p3.len() / 2].to_vec(), false),
            (
                "garbage",
                (0..4096).map(|i| (i * 7 + 3) as u8).collect(),
                false,
            ),
            ("oversized", vec![0; MAX_PROFILE_BYTES + 1], false),
            ("too many tags", too_many_tags, false),
            ("CMYK", cmyk, false),
            ("lookup tables", lookup_table_profile(), false),
            (
                "lookup tables beside a matrix",
                encoded(with_lookup_table(ColorProfile::new_display_p3())),
                false,
            ),
            ("huge colorants", encoded(huge_red), false),
            ("grey profile on RGB", std::mem::take(&mut grey), false),
            ("RGB profile on grey", p3.clone(), true),
        ];
        for (name, profile, grey) in cases {
            let why = ImageColor::of_profile(&profile, grey).expect_err(name);
            assert!(why.contains("read as sRGB"), "{name}: {why}");
        }
        // A curve that isn't a number reads as black rather than poisoning the image.
        if let Ok(color) = ImageColor::of_profile(&encoded(nan_curve), false) {
            let linear = color.to_linear(vec![u16::MAX; 4], false);
            assert!(linear.iter().all(|value| value.is_finite()), "{linear:?}");
        }
    }

    #[test]
    fn the_widest_real_rgb_spaces_are_read() {
        for (name, profile) in [
            ("BT.2020", ColorProfile::new_bt2020()),
            ("ProPhoto", ColorProfile::new_pro_photo_rgb()),
            ("ACES 2065-1", ColorProfile::new_aces_aces_2065_1_linear()),
        ] {
            let color = ImageColor::of_profile(&encoded(profile), false).expect(name);
            assert!(color.gamut.is_some(), "{name}");
        }
    }

    #[test]
    fn float_samples_keep_light_above_one_and_drop_what_isnt_a_number() {
        let rgba = [
            [2.0, 0.5, -0.25, 1.0],
            [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 1.0],
            [1e30, 0.0, 0.0, f32::NAN],
            [0.5, 0.25, 0.0, 0.5],
        ];
        let srgb = ImageColor::SRGB.float_to_linear(rgba.as_flattened(), false);
        let texels = srgb.as_chunks::<4>().0;
        let close = |got: f16, expected: f32| (got.to_f32() - expected).abs() <= expected * 1e-3;
        assert!(close(texels[0][0], srgb_to_linear(2.0)), "{:?}", texels[0]);
        assert!(texels[0][0].to_f32() > 4.0);
        assert!(close(texels[0][1], srgb_to_linear(0.5)));
        assert_eq!(texels[0][2].to_f32(), 0.0);
        assert_eq!(texels[1].map(f16::to_f32), [0.0, 0.0, 0.0, 1.0]);
        assert_eq!(texels[2].map(f16::to_f32), [HALF_MAX, 0.0, 0.0, 0.0]);
        // Premultiplied samples are divided by their alpha first.
        let premultiplied = ImageColor::SRGB.float_to_linear(&rgba[3], true);
        assert!(close(premultiplied[0], 1.0) && close(premultiplied[1], srgb_to_linear(0.5)));
        // A profile's curve is evaluated past 1 too.
        let adobe = ImageColor::of_profile(&adobe_rgb(), false).expect("read Adobe RGB");
        let linear = adobe.float_to_linear(&[2.0, 2.0, 2.0, 1.0], false);
        let white = 2.0_f32.powf(ADOBE_GAMMA);
        assert!(
            linear[..3].iter().all(|&value| close(value, white)),
            "{linear:?}"
        );
    }

    #[test]
    fn mutated_profiles_never_panic_or_poison_pixels() {
        let profiles = [display_p3(), adobe_rgb(), lookup_table_profile()];
        let mut state = 0x2545_f491_u32;
        let mut next = move || {
            state ^= state << 13;
            state ^= state >> 17;
            state ^= state << 5;
            state
        };
        for round in 0..3000 {
            let mut profile = profiles[round % profiles.len()].clone();
            for _ in 0..1 + next() % 8 {
                let at = next() as usize % profile.len();
                profile[at] = next() as u8;
            }
            profile.truncate(next() as usize % (profile.len() + 1) + 128.min(profile.len()));
            if let Ok(color) = ImageColor::of_profile(&profile, false) {
                let mut linear = color.to_linear(vec![0, 30000, u16::MAX, u16::MAX], false);
                linear.extend(color.float_to_linear(&[0.0, 0.5, 2.0, 1.0], false));
                assert!(linear.iter().all(|value| value.is_finite()), "{linear:?}");
            }
        }
    }
}
