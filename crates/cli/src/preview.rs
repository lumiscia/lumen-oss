//! `lumen preview`: frames sampled evenly across a composition's timeline, tiled into one image.

use std::{
    fs::File,
    io::{BufWriter, Write},
    ops::Range,
    path::{Path, PathBuf},
};

use anyhow::{Context, Result, bail};
use clap::Args;
use image::{
    ExtendedColorType, ImageEncoder, ImageFormat, Rgb, RgbImage, RgbaImage,
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
    imageops::{self, FilterType},
};
use lumen_engine::{
    composition::Composition,
    diagnostic::Diagnostic,
    gpu::{DEFAULT_MAX_TEXT_GLYPHS, MAX_TEXT_GLYPHS_CEILING},
};
use lumen_server::render::{
    CancelToken, DEFAULT_JPEG_QUALITY, FrameRange, ImageRender, OutputFormat, RenderOutput,
    RenderProgress,
};

use crate::{
    SourceArgs, load_composition, parse_frames, print_progress, print_warnings, render_to_partial,
};

/// Space between cells and around the edge of the sheet, in pixels.
const GUTTER: u32 = 8;
/// What shows through transparent pixels and between cells.
const BACKGROUND: Rgb<u8> = Rgb([40, 40, 40]);
/// The widest `--width` takes.
const MAX_WIDTH: u32 = 16_384;
/// The tallest sheet, which is the most a JPEG holds.
const MAX_HEIGHT: u32 = 65_535;

#[derive(Debug, Args)]
pub(crate) struct PreviewArgs {
    /// Output image: .png, .jpg or .jpeg
    #[arg(short, long, value_name = "PATH")]
    output: PathBuf,

    /// Frames to sample from as START..END, END not included [default: all]
    ///
    /// Either side can be left out, as in 30.. or ..60.
    #[arg(long, value_name = "START..END", value_parser = parse_frames)]
    frames: Option<FrameRange>,

    /// Frames to sample, spread evenly over the range
    #[arg(
        long,
        value_name = "N",
        default_value_t = 12,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    count: u32,

    /// Frames per row
    #[arg(
        long,
        value_name = "N",
        default_value_t = 4,
        value_parser = clap::value_parser!(u32).range(1..)
    )]
    columns: u32,

    /// Width of the whole sheet in pixels
    #[arg(
        long,
        value_name = "PX",
        default_value_t = 1920,
        value_parser = clap::value_parser!(u32).range(1..=i64::from(MAX_WIDTH))
    )]
    width: u32,

    #[command(flatten)]
    source: SourceArgs,

    /// Most glyph instances one text node may draw
    ///
    /// Counts glyph, stroke and decoration instances. A text over it fails the preview.
    #[arg(
        long,
        env = "LUMEN_MAX_TEXT_GLYPHS",
        value_name = "N",
        default_value_t = DEFAULT_MAX_TEXT_GLYPHS as u32,
        value_parser = clap::value_parser!(u32).range(1..=MAX_TEXT_GLYPHS_CEILING as i64)
    )]
    max_text_glyphs: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SheetFormat {
    Png,
    Jpeg,
}

impl SheetFormat {
    fn of(output: &Path) -> Result<Self> {
        let extension = output
            .extension()
            .and_then(|value| value.to_str())
            .map(|value| value.to_ascii_lowercase());
        match extension.as_deref() {
            Some("png") => Ok(Self::Png),
            Some("jpg" | "jpeg") => Ok(Self::Jpeg),
            _ => bail!(
                "a preview is one image; use .png, .jpg or .jpeg (got `{}`)",
                output.display()
            ),
        }
    }
}

/// Where each frame goes on the sheet: `columns` cells a row, row by row, each `cell_width` by
/// `cell_height` with [`GUTTER`] between them and around the edge. Pixels the cells leave over
/// from `width` are split between the left and right edges.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Layout {
    columns: u32,
    rows: u32,
    cell_width: u32,
    cell_height: u32,
    width: u32,
    height: u32,
    left: u32,
}

impl Layout {
    /// The grid for `frames` frames of a `canvas` (width, height) composition on a sheet `width`
    /// wide. There are never more columns than frames.
    fn new(frames: u32, columns: u32, width: u32, canvas: (u32, u32)) -> Result<Self> {
        let columns = columns.min(frames).max(1);
        let rows = frames.div_ceil(columns);
        let gutters = GUTTER * (columns + 1);
        let cell_width = width.saturating_sub(gutters) / columns;
        if cell_width == 0 {
            bail!(
                "--width {width} leaves no room for {columns} columns; they need at least {}",
                gutters + columns
            );
        }
        let (canvas_width, canvas_height) =
            (u64::from(canvas.0.max(1)), u64::from(canvas.1.max(1)));
        let cell_height =
            ((u64::from(cell_width) * canvas_height + canvas_width / 2) / canvas_width).max(1);
        let height = u64::from(rows) * (cell_height + u64::from(GUTTER)) + u64::from(GUTTER);
        if height > u64::from(MAX_HEIGHT) {
            bail!(
                "a sheet of {rows} rows would be {height} pixels tall, more than {MAX_HEIGHT}; \
                 use a smaller --count, more --columns or a smaller --width"
            );
        }
        Ok(Self {
            columns,
            rows,
            cell_width,
            cell_height: cell_height as u32,
            width,
            height: height as u32,
            left: GUTTER + (width - gutters - cell_width * columns) / 2,
        })
    }

    /// The top-left corner of the cell of the `index`th frame.
    fn cell_origin(&self, index: u32) -> (u32, u32) {
        let (column, row) = (index % self.columns, index / self.columns);
        (
            self.left + column * (self.cell_width + GUTTER),
            GUTTER + row * (self.cell_height + GUTTER),
        )
    }
}

/// `count` frames of `range`, the first and last among them, as evenly spaced as whole frames
/// allow; every frame when the range has no more than `count`.
fn sample_frames(range: Range<u32>, count: u32) -> Vec<u32> {
    let length = range.end - range.start;
    if length <= count {
        return range.collect();
    }
    if count == 1 {
        return vec![range.start];
    }
    let span = u64::from(length - 1);
    let steps = u64::from(count - 1);
    (0..count)
        .map(|index| range.start + (u64::from(index) * span / steps) as u32)
        .collect()
}

pub(crate) fn preview(args: &PreviewArgs) -> Result<()> {
    let format = SheetFormat::of(&args.output)?;
    let composition = load_composition(&args.source)?;
    let range = RenderOutput {
        format: OutputFormat::PngSequence,
        frames: args.frames.unwrap_or_default(),
        ..RenderOutput::default()
    }
    .resolve(composition.timeline().duration_frames)?;
    let frames = sample_frames(range, args.count);
    let settings = composition.render_settings();
    let layout = Layout::new(
        frames.len() as u32,
        args.columns,
        args.width,
        (settings.width, settings.height),
    )?;

    let mut diagnostics: Vec<Diagnostic> = Vec::new();
    render_to_partial(&args.output, |partial| {
        let mut sheet = RgbImage::from_pixel(layout.width, layout.height, BACKGROUND);
        for (index, &frame) in frames.iter().enumerate() {
            let (image, frame_diagnostics) = render_frame(&composition, args, frame)?;
            let cell = fit_cell(&image, &layout);
            let (x, y) = layout.cell_origin(index as u32);
            imageops::replace(&mut sheet, &cell, i64::from(x), i64::from(y));
            for diagnostic in frame_diagnostics {
                if !diagnostics.contains(&diagnostic) {
                    diagnostics.push(diagnostic);
                }
            }
            let completed = index as u32 + 1;
            print_progress(RenderProgress {
                stage: "rendering",
                frame: completed,
                total_frames: frames.len() as u32,
                ratio: completed as f32 / frames.len() as f32,
            });
        }
        write_sheet(&sheet, format, partial)
    })?;
    print_warnings(&diagnostics);
    println!(
        "preview complete output={} frames={} grid={}x{} size={}x{}",
        args.output.display(),
        frames.len(),
        layout.columns,
        layout.rows,
        layout.width,
        layout.height
    );
    Ok(())
}

/// Renders one frame at the composition's size, with the warnings its render gave.
fn render_frame(
    composition: &Composition,
    args: &PreviewArgs,
    frame: u32,
) -> Result<(RgbaImage, Vec<Diagnostic>)> {
    let media_store = args.source.media_store()?.explaining_missing_sources();
    let output = RenderOutput {
        format: OutputFormat::Png,
        frames: FrameRange {
            start: Some(frame),
            end: None,
        },
        ..RenderOutput::default()
    };
    let mut render = ImageRender::open(composition, media_store, &output, CancelToken::new())?;
    render.set_max_text_glyphs(args.max_text_glyphs as usize);
    let mut decoded = None;
    let report = render.run(
        &mut |encoded| {
            let image = image::load_from_memory_with_format(&encoded.bytes, ImageFormat::Png)
                .with_context(|| format!("failed to decode frame {frame}"))?;
            decoded = Some(image.to_rgba8());
            Ok(())
        },
        &mut |_| {},
    )?;
    let image = decoded.with_context(|| format!("frame {frame} rendered no image"))?;
    Ok((image, report.diagnostics))
}

/// The frame over [`BACKGROUND`], scaled to the layout's cell. It is made opaque first so that
/// scaling doesn't blend in the colour of transparent pixels.
fn fit_cell(frame: &RgbaImage, layout: &Layout) -> RgbImage {
    let [red, green, blue] = BACKGROUND.0.map(u32::from);
    let opaque = RgbImage::from_fn(frame.width(), frame.height(), |x, y| {
        let [r, g, b, a] = frame.get_pixel(x, y).0.map(u32::from);
        let over = |colour: u32, background: u32| {
            ((colour * a + background * (255 - a) + 127) / 255) as u8
        };
        Rgb([over(r, red), over(g, green), over(b, blue)])
    });
    if opaque.dimensions() == (layout.cell_width, layout.cell_height) {
        return opaque;
    }
    imageops::resize(
        &opaque,
        layout.cell_width,
        layout.cell_height,
        FilterType::Triangle,
    )
}

fn write_sheet(sheet: &RgbImage, format: SheetFormat, path: &Path) -> Result<()> {
    let file = File::create(path).with_context(|| format!("failed to write {}", path.display()))?;
    let mut writer = BufWriter::new(file);
    let (width, height) = sheet.dimensions();
    match format {
        SheetFormat::Png => PngEncoder::new(&mut writer).write_image(
            sheet.as_raw(),
            width,
            height,
            ExtendedColorType::Rgb8,
        ),
        SheetFormat::Jpeg => JpegEncoder::new_with_quality(&mut writer, DEFAULT_JPEG_QUALITY)
            .write_image(sheet.as_raw(), width, height, ExtendedColorType::Rgb8),
    }
    .context("failed to encode the sheet")?;
    writer
        .flush()
        .with_context(|| format!("failed to write {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::{GUTTER, Layout, sample_frames};

    #[test]
    fn samples_spread_from_the_first_frame_to_the_last() {
        assert_eq!(
            sample_frames(0..90, 12),
            [0, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 89]
        );
        assert_eq!(sample_frames(30..300, 4), [30, 119, 209, 299]);
        assert_eq!(sample_frames(10..20, 1), [10]);
        assert_eq!(sample_frames(5..9, 12), [5, 6, 7, 8]);
        assert_eq!(sample_frames(0..12, 12), (0..12).collect::<Vec<_>>());
    }

    #[test]
    fn layouts_fill_the_width_and_keep_the_aspect_ratio() {
        let layout = Layout::new(12, 4, 1920, (1920, 1080)).unwrap();
        assert_eq!((layout.columns, layout.rows), (4, 3));
        assert_eq!((layout.cell_width, layout.cell_height), (470, 264));
        assert_eq!(layout.height, 3 * 264 + 4 * GUTTER);
        assert_eq!(layout.cell_origin(0), (GUTTER, GUTTER));
        assert_eq!(layout.cell_origin(5), (GUTTER + 478, GUTTER + 272));

        let uneven = Layout::new(5, 4, 1010, (100, 100)).unwrap();
        assert_eq!(
            (uneven.columns, uneven.rows, uneven.cell_width),
            (4, 2, 242)
        );
        assert_eq!(uneven.cell_origin(0), (GUTTER + 1, GUTTER));
        assert_eq!(uneven.cell_origin(4), (GUTTER + 1, 2 * GUTTER + 242));
        assert_eq!(Layout::new(3, 4, 1000, (100, 100)).unwrap().columns, 3);

        assert!(Layout::new(4, 4, 43, (1, 1)).is_err());
        assert!(Layout::new(4, 4, 44, (1, 1)).is_ok());
        assert!(Layout::new(100_000, 1, 1920, (16, 9)).is_err());
    }
}
