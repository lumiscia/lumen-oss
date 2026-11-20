//! The size an image is decoded at: where every use of it bounds how large it is shown, no
//! larger than twice that, and no larger than the device or host allows.
//!
//! A `media_in` raster covers the device pixels of its image at the render scale. Most nodes read
//! a raster's texels as its pixels, so an image they read is decoded at that size. `resize`,
//! `transform` and `image_plane_3d` read their source at its density instead, so an image whose
//! every reader is one of them may be decoded at fewer texels per axis while covering the same
//! pixels (see [`Raster::reduced`](super::frame_graph::Raster::reduced)). Such an image is decoded
//! at twice the size it is shown at, where a literal `resize` or `transform` in the root
//! composition bounds that size, and at most at the device's or host's limit.

use std::borrow::Cow;
use std::sync::Arc;

use foldhash::{HashMap, HashMapExt, HashSet, HashSetExt};
use lumen_gpu::Size;

use crate::composition::Composition;
use half::f16;

use crate::media::{CpuMediaFrame, LinearMediaFrame};
use crate::node::processing::resize::ResizeMode;
use crate::node::source::media_in::MediaInSourceKindDelegate;
use crate::node::{Deferred, Node, NodeId, NodeKind};

use super::color::{linear_to_srgb, srgb_to_linear};
use super::frame_graph::RecordNode;

/// An image is reduced only when it is shown at less than this fraction of its size.
const REDUCE_BELOW: f64 = 0.5;
/// How much larger than it is shown a reduced image is kept, so that filtering and mild
/// supersampling stay sharp.
const HEADROOM: f64 = 2.0;

/// How a node that reads a source at its density shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum ReducedRead {
    /// At a size that isn't known before rendering.
    Unknown,
    /// Drawn into a `width` by `height` rectangle of canvas pixels, fitted as `mode` says.
    Fitted {
        width: f64,
        height: f64,
        mode: ResizeMode,
    },
    /// Scaled by at most this much.
    Scaled(f64),
}

/// A known use of an image: how its reader shows it, and the most anything after the reader
/// magnifies what it draws.
#[derive(Debug, Clone, Copy)]
struct Shown {
    read: ReducedRead,
    magnified: f64,
}

impl Shown {
    /// The most the use shows an image of `native` size at, as a fraction of that size.
    fn scale(self, native: Size) -> f64 {
        let own = match self.read {
            ReducedRead::Unknown => f64::INFINITY,
            ReducedRead::Scaled(scale) => scale,
            ReducedRead::Fitted {
                width,
                height,
                mode,
            } => {
                let x = width / f64::from(native.width.max(1));
                let y = height / f64::from(native.height.max(1));
                match mode {
                    ResizeMode::Fit => x.min(y),
                    ResizeMode::Stretch | ResizeMode::Fill => x.max(y),
                }
            }
        };
        own * self.magnified
    }
}

/// What the readers of the `media_in` nodes that show an image allow.
#[derive(Debug, Clone)]
struct Uses {
    /// The uses, or `None` once one shows the image at a size that isn't known before rendering.
    shown: Option<Vec<Shown>>,
    /// Whether every reader reads the image at its density.
    reduced: bool,
}

impl Default for Uses {
    fn default() -> Self {
        Self {
            shown: Some(Vec::new()),
            reduced: true,
        }
    }
}

impl Uses {
    fn add(&mut self, shown: Option<Vec<Shown>>, reduced: bool) {
        self.reduced &= reduced;
        match (self.shown.as_mut(), shown) {
            (Some(uses), Some(shown)) => uses.extend(shown),
            _ => self.shown = None,
        }
    }
}

/// How large a composition can show each image it names, and whether it can read each at fewer
/// texels than its pixels.
#[derive(Debug, Default)]
pub(crate) struct ShownSizes {
    sources: HashMap<String, Uses>,
    /// The uses of `media_in` nodes that pick their source with an expression, which can name any
    /// image.
    any_source: Option<Uses>,
}

impl ShownSizes {
    pub(crate) fn of(composition: &Composition) -> Arc<Self> {
        let mut shown = Self::default();
        let root = composition.graph();
        let mut magnified = Magnification::new(&root.nodes, root.output());
        for node in root.nodes.values() {
            shown.add(node, &mut magnified, true);
        }
        for template in composition.compositions().values() {
            let graph = &template.scene.graph;
            let mut magnified = Magnification::new(&graph.nodes, graph.output());
            for node in graph.nodes.values() {
                shown.add(node, &mut magnified, false);
            }
        }
        Arc::new(shown)
    }

    /// Adds how `node`, if it is a `media_in` that can show an image, is read. Outside the root
    /// composition, its output may be shown at any size.
    fn add(&mut self, node: &NodeKind, magnified: &mut Magnification<'_>, root: bool) {
        let NodeKind::MediaIn(media) = node else {
            return;
        };
        if matches!(media.params.kind, MediaInSourceKindDelegate::Video) {
            return;
        }
        let (shown, reduced) = magnified.readers(media.id);
        let sized = root && matches!(media.params.kind, MediaInSourceKindDelegate::Image);
        let shown = shown.filter(|_| sized);
        let uses = match &media.params.source {
            Deferred::Value(source) => self.sources.entry(source.clone()).or_default(),
            Deferred::Expr(_) => self.any_source.get_or_insert_with(Uses::default),
        };
        uses.add(shown, reduced);
    }

    /// The uses of `source`, and of every `media_in` that could name it.
    fn uses(&self, source: &str) -> Uses {
        let mut uses = self.sources.get(source).cloned().unwrap_or(Uses {
            shown: None,
            reduced: self.any_source.is_some(),
        });
        if let Some(any) = &self.any_source {
            uses.add(None, any.reduced);
        }
        uses
    }
}

/// The size an image is decoded at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Decoded {
    /// Decoded at `texels` to cover `shown` device pixels, its native size at the render scale
    /// with each side rounded up. `texels` are fewer only where every reader reads the image at
    /// its density, and `limited` when the size limit made them fewer than its uses ask for.
    At {
        texels: Size,
        shown: Size,
        limited: bool,
    },
    /// Past the device's limit and read where it can't be reduced.
    TooLarge,
}

/// The size image `source`, of `native` size, is decoded at for a render at `render_scale`. It
/// covers its native size at the render scale. Where every reader reads it at its density, its
/// texels are twice the size it is shown at when that is under half its own, and fit `host_max`
/// and `device_max` on a side. Elsewhere it has a texel per pixel, if `device_max` allows.
pub(crate) fn decoded_size(
    shown: &ShownSizes,
    source: &str,
    native: Size,
    host_max: u32,
    device_max: u32,
    render_scale: f64,
) -> Decoded {
    let pixels = super::render_scale::device_size(native.width, native.height, render_scale);
    let uses = shown.uses(source);
    if !uses.reduced {
        if pixels.width.max(pixels.height) > device_max {
            return Decoded::TooLarge;
        }
        return Decoded::At {
            texels: pixels,
            shown: pixels,
            limited: false,
        };
    }
    let scale = uses
        .shown
        .and_then(|uses| uses.iter().map(|use_| use_.scale(native)).reduce(f64::max))
        .map(|scale| scale * render_scale)
        .filter(|scale| *scale < REDUCE_BELOW);
    // The epsilon keeps an exact size from rounding up past it.
    let extent = |side: u32, scale: f64| ((f64::from(side) * scale - 1e-9).ceil() as u32).max(1);
    let wanted = match scale {
        Some(scale) => Size::new(
            extent(native.width, HEADROOM * scale).min(pixels.width),
            extent(native.height, HEADROOM * scale).min(pixels.height),
        ),
        None => pixels,
    };
    let limit = host_max.min(device_max).max(1);
    let side = wanted.width.max(wanted.height);
    if side <= limit {
        return Decoded::At {
            texels: wanted,
            shown: pixels,
            limited: false,
        };
    }
    let fitted = |extent: u32| {
        (u64::from(extent) * u64::from(limit) / u64::from(side)).clamp(1, u64::from(limit)) as u32
    };
    Decoded::At {
        texels: Size::new(fitted(wanted.width), fitted(wanted.height)),
        shown: pixels,
        limited: true,
    }
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

    /// How `media`'s readers show it, `None` when one shows it at a size that isn't known, and
    /// whether every one reads it at its density. A graph's output is read by what shows the
    /// graph, which reads texels as pixels.
    fn readers(&mut self, media: NodeId) -> (Option<Vec<Shown>>, bool) {
        let readers = self.consumers.get(&media).cloned().unwrap_or_default();
        if readers.is_empty() || media == self.output {
            return (None, false);
        }
        let mut shown = Some(Vec::new());
        for reader in readers {
            let Some(read) = self.nodes.get(&reader).and_then(RecordNode::reads_reduced) else {
                return (None, false);
            };
            let magnified = match read {
                ReducedRead::Unknown => f64::INFINITY,
                _ => self.of(reader),
            };
            if let Some(uses) = shown.as_mut() {
                if magnified.is_finite() {
                    uses.push(Shown { read, magnified });
                } else {
                    shown = None;
                }
            }
        }
        (shown, true)
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
        self.nodes
            .get(&reader)
            .map_or(f64::INFINITY, RecordNode::magnification)
    }
}

/// `frame`'s RGBA8 at `size`: as it is at its own size, premultiplied as `premultiplied` says,
/// or else averaged over each pixel's footprint, an area filter in linear light on premultiplied
/// colour, and stored straight.
pub(crate) fn rgba8_at(frame: &CpuMediaFrame, premultiplied: bool, size: Size) -> Cow<'_, [u8]> {
    if (frame.width, frame.height) == (size.width, size.height) {
        return super::renderer::fit_frame_to_rgba8(frame, size.width, size.height);
    }
    let texels = Srgb8 {
        frame,
        premultiplied,
        decode: std::array::from_fn(|code| srgb_to_linear(code as f32 / 255.0)),
    };
    Cow::Owned(area_filtered(&texels, size))
}

/// `frame`'s straight linear half floats at `size`, filtered as `rgba8_at` filters.
pub(crate) fn linear_at(frame: &LinearMediaFrame, size: Size) -> Cow<'_, [f16]> {
    if (frame.width, frame.height) == (size.width, size.height) {
        return Cow::Borrowed(&frame.rgba);
    }
    Cow::Owned(area_filtered(&Linear(frame), size))
}

/// An image's texels as the area filter reads and writes them.
trait Texels: Sync {
    type Sample: Copy + Default + Send;

    fn width(&self) -> u32;

    fn height(&self) -> u32;

    /// The samples of source row `y`, four per texel.
    fn row(&self, y: usize) -> &[Self::Sample];

    /// One texel as linear light, premultiplied.
    fn read(&self, texel: &[Self::Sample; 4]) -> [f32; 4];

    /// Linear premultiplied light as a straight texel.
    fn write(pixel: [f32; 4]) -> [Self::Sample; 4];
}

/// 8-bit sRGB-encoded texels, premultiplied as `premultiplied` says.
struct Srgb8<'a> {
    frame: &'a CpuMediaFrame,
    premultiplied: bool,
    /// Each code's linear light.
    decode: [f32; 256],
}

impl Texels for Srgb8<'_> {
    type Sample = u8;

    fn width(&self) -> u32 {
        self.frame.width
    }

    fn height(&self) -> u32 {
        self.frame.height
    }

    fn row(&self, y: usize) -> &[u8] {
        let start = y * self.frame.row_bytes;
        &self.frame.rgba[start..start + self.frame.width as usize * 4]
    }

    fn read(&self, texel: &[u8; 4]) -> [f32; 4] {
        let alpha = f32::from(texel[3]) / 255.0;
        // Only partly transparent premultiplied colour is divided out of its code.
        let divided = self.premultiplied && !matches!(texel[3], 0 | 255);
        let mut pixel = [0.0; 4];
        for channel in 0..3 {
            let light = if divided {
                srgb_to_linear(((f32::from(texel[channel]) / 255.0) / alpha).min(1.0))
            } else {
                self.decode[texel[channel] as usize]
            };
            pixel[channel] = light * alpha;
        }
        pixel[3] = alpha;
        pixel
    }

    fn write(pixel: [f32; 4]) -> [u8; 4] {
        let alpha = pixel[3].clamp(0.0, 1.0);
        let mut texel = [0; 4];
        if alpha > 0.0 {
            for channel in 0..3 {
                let encoded = linear_to_srgb((pixel[channel] / alpha).clamp(0.0, 1.0));
                texel[channel] = (encoded * 255.0 + 0.5) as u8;
            }
        }
        texel[3] = (alpha * 255.0 + 0.5) as u8;
        texel
    }
}

/// Straight linear half floats, whose colour may be above 1.
struct Linear<'a>(&'a LinearMediaFrame);

impl Texels for Linear<'_> {
    type Sample = f16;

    fn width(&self) -> u32 {
        self.0.width
    }

    fn height(&self) -> u32 {
        self.0.height
    }

    fn row(&self, y: usize) -> &[f16] {
        let row = self.0.width as usize * 4;
        &self.0.rgba[y * row..(y + 1) * row]
    }

    fn read(&self, texel: &[f16; 4]) -> [f32; 4] {
        let alpha = texel[3].to_f32();
        [0, 1, 2, 3].map(|channel| {
            if channel == 3 {
                alpha
            } else {
                texel[channel].to_f32() * alpha
            }
        })
    }

    fn write(pixel: [f32; 4]) -> [f16; 4] {
        let alpha = pixel[3].clamp(0.0, 1.0);
        [0, 1, 2, 3].map(|channel| match channel {
            3 => f16::from_f32(alpha),
            _ if alpha > 0.0 => f16::from_f32((pixel[channel] / alpha).max(0.0)),
            _ => f16::ZERO,
        })
    }
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

fn area_filtered<T: Texels>(texels: &T, size: Size) -> Vec<T::Sample> {
    let columns = footprints(texels.width(), size.width);
    let rows = footprints(texels.height(), size.height);
    let row_samples = size.width as usize * 4;
    let mut out = vec![T::Sample::default(); row_samples * size.height as usize];
    #[cfg(not(target_arch = "wasm32"))]
    {
        let threads = std::thread::available_parallelism().map_or(1, usize::from);
        let band = rows.len().div_ceil(threads).max(1);
        std::thread::scope(|scope| {
            for (rows, out) in rows.chunks(band).zip(out.chunks_mut(band * row_samples)) {
                let columns = &columns;
                scope.spawn(move || filter_rows(texels, columns, rows, out));
            }
        });
    }
    #[cfg(target_arch = "wasm32")]
    filter_rows(texels, &columns, &rows, &mut out);
    out
}

/// Writes to `out` the output rows whose footprints are `rows`.
fn filter_rows<T: Texels>(
    texels: &T,
    columns: &[Vec<(usize, f32)>],
    rows: &[Vec<(usize, f32)>],
    out: &mut [T::Sample],
) {
    let width = columns.len();
    // One source row in linear premultiplied light, and that row averaged across each column's
    // footprint.
    let mut linear = vec![0.0_f32; texels.width() as usize * 4];
    let mut reduced = vec![0.0_f32; width * 4];
    let mut reduced_row = None;
    let mut sum = vec![0.0_f32; width * 4];
    for (footprint, row) in rows.iter().zip(out.chunks_exact_mut(width * 4)) {
        sum.fill(0.0);
        for &(source_y, weight) in footprint {
            if reduced_row != Some(source_y) {
                for (pixel, texel) in linear
                    .as_chunks_mut::<4>()
                    .0
                    .iter_mut()
                    .zip(texels.row(source_y).as_chunks::<4>().0)
                {
                    *pixel = texels.read(texel);
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
            *texel = T::write(*pixel);
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
    fn linear_images_are_averaged_at_half_float_precision() {
        // A ramp of 2048 levels halved keeps a level per output pixel, each the mean of the two
        // it covers, and translucent colour is weighted by alpha as 8-bit colour is.
        let ramp: Vec<f16> = (0..2048)
            .flat_map(|x| {
                let value = f16::from_f32(x as f32 / 2047.0);
                [value, value, value, f16::ONE]
            })
            .collect();
        let frame = LinearMediaFrame {
            rgba: ramp.clone(),
            width: 2048,
            height: 1,
        };
        let reduced = linear_at(&frame, Size::new(1024, 1));
        let mut levels: Vec<u16> = reduced.iter().step_by(4).map(|v| v.to_bits()).collect();
        levels.dedup();
        assert_eq!(levels.len(), 1024);
        for (x, texel) in reduced.as_chunks::<4>().0.iter().enumerate() {
            let mean = (ramp[x * 8].to_f32() + ramp[x * 8 + 4].to_f32()) / 2.0;
            assert!((texel[0].to_f32() - mean).abs() <= mean * 1e-3, "{x}");
        }
        let edge = LinearMediaFrame {
            rgba: [1.5, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0]
                .map(f16::from_f32)
                .to_vec(),
            width: 2,
            height: 1,
        };
        let texel: Vec<f32> = linear_at(&edge, Size::new(1, 1))
            .iter()
            .map(|value| value.to_f32())
            .collect();
        assert_eq!(texel, [1.5, 0.0, 0.0, 0.5]);
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
        let read = ReducedRead::Fitted {
            width,
            height,
            mode,
        };
        ShownSizes {
            sources: [(
                "photo".to_string(),
                Uses {
                    shown: Some(vec![Shown {
                        read,
                        magnified: 1.0,
                    }]),
                    reduced: true,
                },
            )]
            .into_iter()
            .collect(),
            any_source: None,
        }
    }

    fn at(texels: (u32, u32), shown: (u32, u32), limited: bool) -> Decoded {
        Decoded::At {
            texels: Size::new(texels.0, texels.1),
            shown: Size::new(shown.0, shown.1),
            limited,
        }
    }

    #[test]
    fn an_image_is_reduced_to_twice_its_shown_size_only_under_half_of_it() {
        let native = Size::new(4000, 3000);
        let decoded = |shown: &ShownSizes| decoded_size(shown, "photo", native, 16384, 16384, 1.0);
        let reduced = |width, height| at((width, height), (4000, 3000), false);
        assert_eq!(
            decoded(&fitted(400.0, 300.0, ResizeMode::Stretch)),
            reduced(800, 600)
        );
        // Fill covers the rectangle, so its longer scale counts.
        assert_eq!(
            decoded(&fitted(400.0, 100.0, ResizeMode::Fill)),
            reduced(800, 600)
        );
        // 2 × 4000 / 30 is 266.7, rounded up on each side.
        assert_eq!(
            decoded(&fitted(400.0, 100.0, ResizeMode::Fit)),
            reduced(267, 200)
        );
        assert_eq!(
            decoded(&fitted(2100.0, 300.0, ResizeMode::Stretch)),
            reduced(4000, 3000)
        );
        assert_eq!(decoded(&ShownSizes::default()), reduced(4000, 3000));

        // A render scale makes every image cover fewer pixels, on each side rounded up, and
        // composes with a resize's reduction.
        let at_scale = |shown: &ShownSizes, native, scale| {
            decoded_size(shown, "photo", native, 16384, 16384, scale)
        };
        let unused = ShownSizes::default();
        let half = (2000, 1500);
        assert_eq!(at_scale(&unused, native, 0.5), at(half, half, false));
        assert_eq!(
            at_scale(&unused, Size::new(101, 51), 0.5),
            at((51, 26), (51, 26), false)
        );
        let resized = fitted(400.0, 300.0, ResizeMode::Stretch);
        assert_eq!(at_scale(&resized, native, 0.5), at((400, 300), half, false));
        let enlarged = fitted(2100.0, 300.0, ResizeMode::Stretch);
        assert_eq!(at_scale(&enlarged, native, 0.5), at(half, half, false));
    }

    #[test]
    fn an_image_is_reduced_on_each_axis_whatever_its_aspect_ratio() {
        // 1000 × 9 has no smaller size of its aspect ratio, and covers its own pixels at 200 × 2.
        assert_eq!(
            decoded_size(
                &fitted(100.0, 100.0, ResizeMode::Fit),
                "photo",
                Size::new(1000, 9),
                16384,
                16384,
                1.0
            ),
            at((200, 2), (1000, 9), false)
        );
    }

    #[test]
    fn an_image_past_the_limit_is_reduced_only_where_its_readers_allow() {
        let wide = Size::new(32768, 2);
        assert_eq!(
            decoded_size(
                &fitted(100.0, 1.0, ResizeMode::Fill),
                "photo",
                wide,
                16384,
                16384,
                1.0
            ),
            at((16384, 1), (32768, 2), true)
        );
        assert_eq!(
            decoded_size(&ShownSizes::default(), "photo", wide, 16384, 16384, 1.0),
            Decoded::TooLarge
        );
        // A host's lower limit keeps an image it can't reduce at its own size.
        assert_eq!(
            decoded_size(
                &ShownSizes::default(),
                "photo",
                Size::new(3200, 1600),
                1600,
                16384,
                1.0
            ),
            at((3200, 1600), (3200, 1600), false)
        );
    }

    /// The decoded size of a 400 × 200 `photo` that `media_in` 1 shows to `readers`, under a
    /// `limit` on a side. The last reader is the graph's output.
    fn read_by(readers: serde_json::Value, limit: u32) -> Decoded {
        let mut nodes = vec![serde_json::json!({
            "id": 1, "type": "media_in", "params": { "kind": "image", "source": "photo" }
        })];
        let mut connections = Vec::new();
        for mut reader in readers.as_array().unwrap().clone() {
            let id = reader["id"].clone();
            let inputs = reader.as_object_mut().unwrap().remove("in").unwrap();
            for (port, from) in inputs.as_object().unwrap() {
                connections
                    .push(serde_json::json!({ "from_node": from, "to_node": id, "to_port": port }));
            }
            nodes.push(reader);
        }
        let last = nodes.last().unwrap()["id"].clone();
        nodes.push(serde_json::json!({ "id": 99, "type": "media_output" }));
        connections
            .push(serde_json::json!({ "from_node": last, "to_node": 99, "to_port": "source" }));
        let composition: Composition = crate::json::parse_value(&serde_json::json!({
            "timeline": { "fps": 30, "duration_frames": 1 },
            "render_settings": { "width": 400, "height": 200 },
            "nodes": nodes,
            "connections": connections,
        }))
        .unwrap();
        let shown = ShownSizes::of(&composition);
        decoded_size(&shown, "photo", Size::new(400, 200), limit, limit, 1.0)
    }

    fn transform(id: u32, from: u32, scale: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "id": id, "type": "transform", "in": { "source": from },
                            "params": { "scale_x": scale, "scale_y": scale, "rotate": 30 } })
    }

    #[test]
    fn a_literal_transform_scale_bounds_the_size_its_source_is_shown_at() {
        let native = (400, 200);
        assert_eq!(
            read_by(serde_json::json!([transform(2, 1, 0.25.into())]), 16384),
            at((200, 100), native, false)
        );
        // What follows magnifies it.
        assert_eq!(
            read_by(
                serde_json::json!([transform(2, 1, 0.25.into()), transform(3, 2, 1.5.into())]),
                16384
            ),
            at((300, 150), native, false)
        );
        // A blur reads the image's texels as its pixels.
        let mixed = serde_json::json!([
            transform(2, 1, 0.25.into()),
            { "id": 3, "type": "blur", "in": { "source": 1 } },
            { "id": 4, "type": "merge", "in": { "base": 2, "overlay": 3 } },
        ]);
        assert_eq!(read_by(mixed.clone(), 16384), at(native, native, false));
        assert_eq!(read_by(mixed, 100), Decoded::TooLarge);
    }

    #[test]
    fn an_animated_transform_reads_its_source_reduced_only_to_fit_the_limit() {
        let animated = || serde_json::json!([transform(2, 1, "=0.25".into())]);
        assert_eq!(
            read_by(animated(), 16384),
            at((400, 200), (400, 200), false)
        );
        assert_eq!(read_by(animated(), 100), at((100, 50), (400, 200), true));
    }
}
