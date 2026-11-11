//! The size an image is decoded at: where every use of it bounds how large it is shown, no
//! larger than twice that, and no larger than the device or host allows.
//!
//! A `media_in` raster is the size its image is decoded at, and nodes read a raster's texels as
//! its pixels, so a reduced image would change what most of them draw. Only a `resize` with a
//! literal size and mode maps its whole source into a rectangle of its own, and draws the same
//! picture from an image reduced to its exact aspect ratio. So an image is reduced only when every
//! use of it is such a resize in the root composition, and only to a size with its aspect ratio.
//! Any other use keeps the image at its native size, which past the device's texture limit can't
//! be shown.

use std::borrow::Cow;
use std::sync::Arc;

use foldhash::{HashMap, HashMapExt, HashSet, HashSetExt};
use lumen_gpu::Size;

use crate::composition::Composition;
use crate::media::CpuMediaFrame;
use crate::node::processing::resize::ResizeMode;
use crate::node::source::media_in::MediaInSourceKindDelegate;
use crate::node::{Deferred, Node, NodeId, NodeKind};

use super::color::{linear_to_srgb, srgb_to_linear};

/// An image is reduced only when it is shown at less than this fraction of its size.
const REDUCE_BELOW: f64 = 0.5;
/// How much larger than it is shown a reduced image is kept, so that filtering and mild
/// supersampling stay sharp.
const HEADROOM: f64 = 2.0;

/// A `resize` an image is drawn into, and the most anything after it magnifies what it draws.
#[derive(Debug, Clone, Copy)]
struct Fitted {
    width: f64,
    height: f64,
    mode: ResizeMode,
    magnified: f64,
}

impl Fitted {
    /// The most the resize shows an image of `native` size at, as a fraction of that size.
    fn scale(self, native: Size) -> f64 {
        let x = self.width / f64::from(native.width.max(1));
        let y = self.height / f64::from(native.height.max(1));
        let fitted = match self.mode {
            ResizeMode::Fit => x.min(y),
            ResizeMode::Stretch | ResizeMode::Fill => x.max(y),
        };
        fitted * self.magnified
    }
}

/// How large a composition can show each image it names.
#[derive(Debug, Default)]
pub(crate) struct ShownSizes {
    /// By source, the resizes its uses draw it into, or `None` once one use shows it at a size
    /// that isn't known before rendering.
    sources: HashMap<String, Option<Vec<Fitted>>>,
    /// Whether a `media_in` picks its source with an expression, which can name any image.
    any_source: bool,
}

impl ShownSizes {
    pub(crate) fn of(composition: &Composition) -> Arc<Self> {
        let mut shown = Self::default();
        let root = composition.graph();
        let mut magnified = Magnification::new(&root.nodes, root.output());
        for node in root.nodes.values() {
            shown.add(node, Some(&mut magnified));
        }
        for template in composition.compositions().values() {
            for node in template.scene.graph.nodes.values() {
                shown.add(node, None);
            }
        }
        Arc::new(shown)
    }

    /// Adds where `node`, if it is a `media_in` that can show an image, draws it. Without
    /// `magnified`, the node is in a template, whose output may be shown at any size.
    fn add(&mut self, node: &NodeKind, magnified: Option<&mut Magnification<'_>>) {
        let NodeKind::MediaIn(media) = node else {
            return;
        };
        let source = match &media.params.source {
            Deferred::Value(source) => source,
            Deferred::Expr(_) => {
                self.any_source = true;
                return;
            }
        };
        if matches!(media.params.kind, MediaInSourceKindDelegate::Video) {
            return;
        }
        let fitted = matches!(media.params.kind, MediaInSourceKindDelegate::Image)
            .then_some(magnified)
            .flatten()
            .and_then(|magnified| magnified.fitted(media.id));
        let entry = self
            .sources
            .entry(source.clone())
            .or_insert_with(|| Some(Vec::new()));
        match (entry.as_mut(), fitted) {
            (Some(uses), Some(fitted)) => uses.extend(fitted),
            _ => *entry = None,
        }
    }

    /// The most `source`, an image of `native` size, is shown at, as a fraction of that size.
    fn scale(&self, source: &str, native: Size) -> Option<f64> {
        if self.any_source {
            return None;
        }
        let uses = self.sources.get(source)?.as_ref()?;
        uses.iter()
            .map(|fitted| fitted.scale(native))
            .reduce(f64::max)
    }
}

/// The size an image is decoded at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decoded {
    Native,
    /// Reduced to `size`, which has the native aspect ratio exactly, and to fit the size limit
    /// when `limited`.
    Reduced {
        size: Size,
        limited: bool,
    },
    /// Past the device's limit, and either used where it can't be reduced or without a smaller
    /// size of its aspect ratio within the limit.
    TooLarge,
}

/// The size image `source`, of `native` size, is decoded at. Where the composition lets it be
/// reduced, that is twice the size it is shown at when that is under half its own, and within
/// `host_max` and `device_max` on a side. Elsewhere it keeps its size, if `device_max` allows.
pub(crate) fn decoded_size(
    shown: &ShownSizes,
    source: &str,
    native: Size,
    host_max: u32,
    device_max: u32,
) -> Decoded {
    let longest = native.width.max(native.height);
    let fallback = if longest > device_max {
        Decoded::TooLarge
    } else {
        Decoded::Native
    };
    let Some(scale) = shown.scale(source, native) else {
        return fallback;
    };
    // Sizes with the native aspect ratio are multiples of the ratio in lowest terms.
    let multiples = gcd(native.width, native.height);
    let unit = Size::new(native.width / multiples, native.height / multiples);
    let shown_multiple = if scale < REDUCE_BELOW {
        // The epsilon keeps an exact multiple from rounding up past it.
        ((f64::from(multiples) * HEADROOM * scale - 1e-9).ceil() as u32).max(1)
    } else {
        multiples
    };
    let limit_multiple = host_max.min(device_max) / unit.width.max(unit.height);
    let multiple = shown_multiple.min(limit_multiple);
    if multiple == 0 {
        return fallback;
    }
    if multiple >= multiples {
        return Decoded::Native;
    }
    Decoded::Reduced {
        size: Size::new(unit.width * multiple, unit.height * multiple),
        limited: limit_multiple < shown_multiple,
    }
}

fn gcd(mut a: u32, mut b: u32) -> u32 {
    while b != 0 {
        (a, b) = (b, a % b);
    }
    a.max(1)
}

/// The most each node's output is magnified on its way to a graph's output, `f64::INFINITY` when
/// that isn't known before rendering.
struct Magnification<'a> {
    nodes: &'a std::collections::HashMap<NodeId, NodeKind>,
    output: NodeId,
    consumers: HashMap<NodeId, Vec<NodeId>>,
    found: HashMap<NodeId, f64>,
}

impl<'a> Magnification<'a> {
    fn new(nodes: &'a std::collections::HashMap<NodeId, NodeKind>, output: NodeId) -> Self {
        let mut consumers: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
        for (&id, node) in nodes {
            for input in node.input_ports().into_iter().flatten() {
                consumers.entry(input).or_default().push(id);
            }
        }
        Self {
            nodes,
            output,
            consumers,
            found: HashMap::new(),
        }
    }

    /// The resizes `media`'s output is drawn into, when nothing else reads it and what each draws
    /// is magnified by a known amount.
    fn fitted(&mut self, media: NodeId) -> Option<Vec<Fitted>> {
        let readers = self.consumers.get(&media).cloned().unwrap_or_default();
        if readers.is_empty() {
            return None;
        }
        readers
            .into_iter()
            .map(|reader| {
                let NodeKind::Resize(resize) = self.nodes.get(&reader)? else {
                    return None;
                };
                let (Deferred::Value(width), Deferred::Value(height)) =
                    (&resize.params.width, &resize.params.height)
                else {
                    return None;
                };
                let mode = match resize.params.mode {
                    crate::node::processing::resize::ResizeModeDelegate::Stretch => {
                        ResizeMode::Stretch
                    }
                    crate::node::processing::resize::ResizeModeDelegate::Fit => ResizeMode::Fit,
                    crate::node::processing::resize::ResizeModeDelegate::Fill => ResizeMode::Fill,
                };
                let magnified = self.of(reader);
                magnified.is_finite().then_some(Fitted {
                    width: (*width).max(1) as f64,
                    height: (*height).max(1) as f64,
                    mode,
                    magnified,
                })
            })
            .collect()
    }

    /// The most `node`'s output is magnified on its way to the output.
    fn of(&mut self, node: NodeId) -> f64 {
        let mut stack = vec![node];
        let mut expanding = HashSet::new();
        while let Some(&top) = stack.last() {
            if self.found.contains_key(&top) {
                stack.pop();
                continue;
            }
            if top == self.output {
                self.found.insert(top, 1.0);
                stack.pop();
                continue;
            }
            let readers = self.consumers.get(&top).map_or(&[][..], Vec::as_slice);
            let pending: Vec<NodeId> = readers
                .iter()
                .copied()
                .filter(|reader| !self.found.contains_key(reader))
                .collect();
            if pending.is_empty() || !expanding.insert(top) {
                // A node read by none never reaches the output, and one reached again while its
                // readers are being found is in a cycle; neither is bounded.
                let magnified = if pending.is_empty() && !readers.is_empty() {
                    readers
                        .iter()
                        .map(|&reader| {
                            let (after, factor) = (self.found[&reader], self.factor(reader));
                            if after.is_finite() && factor.is_finite() {
                                after * factor
                            } else {
                                f64::INFINITY
                            }
                        })
                        .fold(0.0, f64::max)
                } else {
                    f64::INFINITY
                };
                self.found.insert(top, magnified);
                stack.pop();
                continue;
            }
            stack.extend(pending);
        }
        self.found[&node]
    }

    /// How much `reader` magnifies what it reads.
    fn factor(&self, reader: NodeId) -> f64 {
        let Some(node) = self.nodes.get(&reader) else {
            return f64::INFINITY;
        };
        match node {
            NodeKind::MediaOutput(_)
            | NodeKind::Merge(_)
            | NodeKind::RasterMultiMerge(_)
            | NodeKind::Switch(_)
            | NodeKind::Opacity(_)
            | NodeKind::Blur(_)
            | NodeKind::ColorGrade(_)
            | NodeKind::Exposure(_)
            | NodeKind::HueSaturation(_)
            | NodeKind::Levels(_)
            | NodeKind::Crop(_)
            | NodeKind::Shadow(_)
            | NodeKind::Glow(_)
            | NodeKind::MotionBlur(_)
            | NodeKind::DirectionalBlur(_) => 1.0,
            NodeKind::Transform(transform) => {
                match (&transform.params.scale_x, &transform.params.scale_y) {
                    (Deferred::Value(x), Deferred::Value(y)) if x.is_finite() && y.is_finite() => {
                        x.abs().max(y.abs())
                    }
                    _ => f64::INFINITY,
                }
            }
            _ => f64::INFINITY,
        }
    }
}

/// `frame`'s RGBA8 at `size`: as it is at its own size, premultiplied as `premultiplied` says,
/// or else averaged over each pixel's footprint, an area filter in linear light on premultiplied
/// colour, and stored straight.
pub(crate) fn rgba8_at(frame: &CpuMediaFrame, premultiplied: bool, size: Size) -> Cow<'_, [u8]> {
    if (frame.width, frame.height) == (size.width, size.height) {
        return super::renderer::fit_frame_to_rgba8(frame, size.width, size.height);
    }
    Cow::Owned(area_filtered(frame, premultiplied, size))
}

/// For each output pixel along one axis, the source pixels its footprint covers and how much of
/// each, as fractions of the footprint.
fn footprints(source: u32, output: u32) -> Vec<Vec<(usize, f32)>> {
    let ratio = f64::from(source) / f64::from(output);
    (0..output)
        .map(|index| {
            let start = f64::from(index) * ratio;
            let end = (start + ratio).min(f64::from(source));
            let first = start.floor() as u32;
            let last = (end.ceil() as u32).clamp(first + 1, source);
            (first..last)
                .map(|texel| {
                    let covered = end.min(f64::from(texel + 1)) - start.max(f64::from(texel));
                    (texel as usize, (covered / ratio) as f32)
                })
                .filter(|(_, weight)| *weight > 0.0)
                .collect()
        })
        .collect()
}

fn area_filtered(frame: &CpuMediaFrame, premultiplied: bool, size: Size) -> Vec<u8> {
    let columns = footprints(frame.width, size.width);
    let rows = footprints(frame.height, size.height);
    let row_bytes = size.width as usize * 4;
    let mut out = vec![0_u8; row_bytes * size.height as usize];
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map_or(1, usize::from);
        let band = rows.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (rows, out) in rows.chunks(band).zip(out.chunks_mut(band * row_bytes)) {
                let columns = &columns;
                scope.spawn(move || filter_rows(frame, premultiplied, columns, rows, out));
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    filter_rows(frame, premultiplied, &columns, &rows, &mut out);
    out
}

/// Writes to `out` the output rows whose footprints are `rows`.
fn filter_rows(
    frame: &CpuMediaFrame,
    premultiplied: bool,
    columns: &[Vec<(usize, f32)>],
    rows: &[Vec<(usize, f32)>],
    out: &mut [u8],
) {
    let decode: [f32; 256] = std::array::from_fn(|code| srgb_to_linear(code as f32 / 255.0));
    let width = columns.len();
    // One source row in linear premultiplied light, and that row averaged across each column's
    // footprint.
    let mut linear = vec![0.0_f32; frame.width as usize * 4];
    let mut reduced = vec![0.0_f32; width * 4];
    let mut reduced_row = None;
    let mut sum = vec![0.0_f32; width * 4];
    for (footprint, row) in rows.iter().zip(out.chunks_exact_mut(width * 4)) {
        sum.fill(0.0);
        for &(source_y, weight) in footprint {
            if reduced_row != Some(source_y) {
                let start = source_y * frame.row_bytes;
                let texels = &frame.rgba[start..start + frame.width as usize * 4];
                for (pixel, texel) in linear
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(texels.as_chunks::<4>().0)
                {
                    let alpha = f32::from(texel[3]) / 255.0;
                    // Only partly transparent premultiplied colour is divided out of its code.
                    let divided = premultiplied && !matches!(texel[3], 0 | 255);
                    for channel in 0..3 {
                        let light = if divided {
                            srgb_to_linear(((f32::from(texel[channel]) / 255.0) / alpha).min(1.0))
                        } else {
                            decode[texel[channel] as usize]
                        };
                        pixel[channel] = light * alpha;
                    }
                    pixel[3] = alpha;
                }
                for (pixel, footprint) in reduced.as_chunks_mut::<4>().0.iter_mut().zip(columns) {
                    let mut total = [0.0_f32; 4];
                    for &(x, weight) in footprint {
                        for (sum, value) in total.iter_mut().zip(&linear[x * 4..x * 4 + 4]) {
                            *sum += value * weight;
                        }
                    }
                    *pixel = total;
                }
                reduced_row = Some(source_y);
            }
            for (sum, value) in sum.iter_mut().zip(&reduced) {
                *sum += value * weight;
            }
        }
        for (texel, pixel) in row
            .as_chunks_mut::<4>()
            .0
            .iter_mut()
            .zip(sum.as_chunks::<4>().0)
        {
            let alpha = pixel[3].clamp(0.0, 1.0);
            if alpha > 0.0 {
                for channel in 0..3 {
                    let encoded = linear_to_srgb((pixel[channel] / alpha).clamp(0.0, 1.0));
                    texel[channel] = (encoded * 255.0 + 0.5) as u8;
                }
            }
            texel[3] = (alpha * 255.0 + 0.5) as u8;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, rgba: Vec<u8>) -> CpuMediaFrame {
        CpuMediaFrame {
            rgba: Arc::new(rgba),
            width,
            height,
            row_bytes: width as usize * 4,
        }
    }

    #[test]
    fn a_checkerboard_halved_is_mid_grey_in_linear_light() {
        let pixels: Vec<u8> = (0..8 * 8)
            .flat_map(|index| {
                let white = (index % 8 + index / 8) % 2 == 0;
                if white { [255; 4] } else { [0, 0, 0, 255] }
            })
            .collect();
        let reduced = rgba8_at(&frame(8, 8, pixels), false, Size::new(4, 4)).into_owned();
        for texel in reduced.as_chunks::<4>().0 {
            assert_eq!(*texel, [188, 188, 188, 255]);
        }
    }

    #[test]
    fn colour_is_averaged_by_how_much_light_it_covers() {
        // A red pixel at full alpha beside a transparent one is red at half alpha, not darkened.
        let reduced = rgba8_at(
            &frame(2, 1, vec![255, 0, 0, 255, 0, 255, 0, 0]),
            false,
            Size::new(1, 1),
        )
        .into_owned();
        assert_eq!(reduced, [255, 0, 0, 128]);
        // Premultiplied input gives the same result.
        let premultiplied = rgba8_at(
            &frame(2, 1, vec![255, 0, 0, 255, 0, 0, 0, 0]),
            true,
            Size::new(1, 1),
        )
        .into_owned();
        assert_eq!(premultiplied, [255, 0, 0, 128]);
    }

    #[test]
    fn a_footprint_covers_fractions_of_the_pixels_at_its_edges() {
        let footprints = footprints(3, 2);
        assert_eq!(footprints[0].len(), 2);
        let [(0, a), (1, b)] = footprints[0][..] else {
            panic!("{footprints:?}");
        };
        assert!((a - 2.0 / 3.0).abs() < 1e-6 && (b - 1.0 / 3.0).abs() < 1e-6);
    }

    fn fitted(width: f64, height: f64, mode: ResizeMode) -> ShownSizes {
        ShownSizes {
            sources: [(
                "photo".to_string(),
                Some(vec![Fitted {
                    width,
                    height,
                    mode,
                    magnified: 1.0,
                }]),
            )]
            .into_iter()
            .collect(),
            any_source: false,
        }
    }

    #[test]
    fn an_image_is_reduced_to_twice_its_shown_size_only_under_half_of_it() {
        let native = Size::new(4000, 3000);
        let decoded = |shown: &ShownSizes| decoded_size(shown, "photo", native, 16384, 16384);
        let reduced = |width, height| Decoded::Reduced {
            size: Size::new(width, height),
            limited: false,
        };
        assert_eq!(
            decoded(&fitted(400.0, 300.0, ResizeMode::Stretch)),
            reduced(800, 600)
        );
        // Fill covers the rectangle, so its longer scale counts.
        assert_eq!(
            decoded(&fitted(400.0, 100.0, ResizeMode::Fill)),
            reduced(800, 600)
        );
        // 2 × 4000 / 30 is 266.7, rounded up to a multiple of 4:3.
        assert_eq!(
            decoded(&fitted(400.0, 100.0, ResizeMode::Fit)),
            reduced(268, 201)
        );
        assert_eq!(
            decoded(&fitted(2100.0, 300.0, ResizeMode::Stretch)),
            Decoded::Native
        );
        assert_eq!(decoded(&ShownSizes::default()), Decoded::Native);
    }

    #[test]
    fn a_reduced_image_keeps_its_aspect_ratio_exactly_or_its_size() {
        // 1000 × 9 has no smaller size of its aspect ratio.
        assert_eq!(
            decoded_size(
                &fitted(100.0, 100.0, ResizeMode::Fit),
                "photo",
                Size::new(1000, 9),
                16384,
                16384
            ),
            Decoded::Native
        );
    }

    #[test]
    fn an_image_past_the_limit_is_reduced_only_where_its_uses_allow() {
        let wide = Size::new(32768, 2);
        assert_eq!(
            decoded_size(
                &fitted(100.0, 1.0, ResizeMode::Fill),
                "photo",
                wide,
                16384,
                16384
            ),
            Decoded::Reduced {
                size: Size::new(16384, 1),
                limited: true,
            }
        );
        assert_eq!(
            decoded_size(&ShownSizes::default(), "photo", wide, 16384, 16384),
            Decoded::TooLarge
        );
        // A host's lower limit keeps an image it can't reduce at its own size.
        assert_eq!(
            decoded_size(
                &ShownSizes::default(),
                "photo",
                Size::new(3200, 1600),
                1600,
                16384
            ),
            Decoded::Native
        );
        // Past the device's limit with no smaller size of its aspect ratio within it.
        assert_eq!(
            decoded_size(
                &fitted(100.0, 1.0, ResizeMode::Fill),
                "photo",
                Size::new(16385, 3),
                16384,
                16384
            ),
            Decoded::TooLarge
        );
    }
}
