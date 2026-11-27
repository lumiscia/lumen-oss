//! Shapes the text of `text_2d` nodes into glyph outlines, and keeps them, and the paths
//! `render_2d` flattens them to, while the text, font and layout stay the same.

use std::sync::Arc;

use foldhash::HashMap;
use lumen_text::{GlyphKey, GlyphOutline, OutlineCommand, TextLayoutRequest};
use lyon_path::iterator::PathIterator;
use lyon_path::math::point;
use lyon_path::{Path, PathEvent};
use lyon_tessellation::{
    FillGeometryBuilder, FillOptions, FillRule, FillTessellator, FillVertex, GeometryBuilder,
    GeometryBuilderError, VertexId,
};

use super::path::{MAX_PATH_POINTS, PathPoint, contour_points, point_bounds};
use super::{Text2d, Text2dParams};
use crate::error::{LumenError, RenderError, WarningKind};
use crate::gpu::frame_graph::FrameCx;
use crate::media::MediaStore;
use crate::node::NodeId;
use crate::node::source::text::{TextAlignmentHorizontal, TextAlignmentVertical, TextLayoutParams};
use crate::node::source::text_content::TextContent;
use crate::node::source::text_fonts::{load_font_families, text_system, used_font_families};

/// Greatest size, line width, letter spacing or line height in pixels.
const MAX_LENGTH: f64 = 1e6;
/// Font sizes below this draw nothing.
const MIN_FONT_SIZE: f64 = 1e-6;
/// Most commands in one glyph's outline.
const MAX_GLYPH_COMMANDS: usize = 4096;
/// Most points a text's outlines flatten to, coarsening its curves until they fit.
const MAX_TEXT_POINTS: usize = 1 << 20;
/// Most points of the glyphs drawn as one path, leaving room for the points where overlapping
/// contours cross within [`MAX_PATH_POINTS`].
const MAX_CHUNK_POINTS: usize = MAX_PATH_POINTS / 2;
/// Doublings of the flattening tolerance tried before an outline is too complex.
const FLATTENING_ATTEMPTS: usize = 12;

fn text_error(node: NodeId, details: impl Into<String>) -> LumenError {
    RenderError::evaluating::<Text2d>(node, details).into()
}

/// Fails unless `value`, the length param `param`, is a finite number of at most [`MAX_LENGTH`]
/// in size that is not negative unless `signed`.
pub(super) fn check_length(param: &str, value: f64, signed: bool) -> Result<(), String> {
    if !value.is_finite() {
        Err(format!("`{param}` must be a finite number, got {value}"))
    } else if !signed && value < 0.0 {
        Err(format!("`{param}` must be at least 0, got {value}"))
    } else if value.abs() > MAX_LENGTH {
        Err(format!(
            "`{param}` must be at most {MAX_LENGTH:e} pixels, got {value:e}"
        ))
    } else {
        Ok(())
    }
}

pub(super) fn check_weight(weight: i64) -> Result<(), String> {
    if (1..=1000).contains(&weight) {
        Ok(())
    } else {
        Err(format!(
            "`font_weight` must be from 1 to 1000, got {weight}"
        ))
    }
}

impl Text2dParams {
    /// The lengths among the params, by name, and whether each may be negative.
    pub(super) fn lengths(&self) -> [(&'static str, f64, bool); 4] {
        [
            ("font_size", self.font_size, false),
            ("max_width", self.max_width, false),
            ("letter_spacing", self.letter_spacing, true),
            ("line_height", self.line_height, false),
        ]
    }

    fn check(&self) -> Result<(), String> {
        check_weight(self.font_weight)?;
        for (param, value, signed) in self.lengths() {
            check_length(param, value, signed)?;
        }
        Ok(())
    }

    /// The layout a `text` node with these params shapes with.
    fn layout(&self) -> TextLayoutParams {
        TextLayoutParams {
            content: TextContent::plain(self.content.clone()),
            font_family: self.font_family.clone(),
            font_size: self.font_size,
            font_weight: self.font_weight,
            font_style: self.font_style,
            max_width: self.max_width,
            letter_spacing: self.letter_spacing,
            font_features: self.font_features.clone(),
            line_height: self.line_height,
            alignment_horizontal: self.alignment_horizontal,
            text_transform: self.text_transform,
            ..TextLayoutParams::default()
        }
    }

    /// The point of the text's box at `position`, as a fraction of its width.
    fn anchor_x(&self) -> f32 {
        match self.alignment_horizontal {
            TextAlignmentHorizontal::Left | TextAlignmentHorizontal::Justify => 0.0,
            TextAlignmentHorizontal::Center => 0.5,
            TextAlignmentHorizontal::Right => 1.0,
        }
    }
}

/// One glyph with an outline, placed in the text.
struct PlacedGlyph {
    /// Pen position in pixels from the text's `position`, Y down.
    origin: [f32; 2],
    /// The glyph's character number in the text, counting from 1.
    character: u32,
    /// In pixels from the pen position, Y up.
    outline: Arc<Path>,
}

/// A `text_2d`'s text, shaped, with the outline of every glyph it draws.
#[derive(Default)]
pub(crate) struct TextGlyphs {
    /// Tells texts shaped apart, for what is built from them; 0 for text with nothing to draw.
    pub(crate) id: u64,
    glyphs: Vec<PlacedGlyph>,
    /// Glyphs left out for having no outline.
    skipped: usize,
}

/// A text's outlines as the paths `render_2d` draws, in pixels from its `position`.
pub(super) struct TextPaths {
    /// The points of each path, consecutive glyphs together.
    pub(super) chunks: Vec<Vec<PathPoint>>,
    /// The box around every path's points.
    pub(super) bounds: ([f32; 2], [f32; 2]),
}

/// Everything a text's glyphs are shaped and placed from.
#[derive(PartialEq)]
struct TextKey {
    request: TextLayoutRequest,
    font_generation: u64,
    anchor_x: f32,
    alignment_vertical: TextAlignmentVertical,
}

struct KeptText {
    key: TextKey,
    glyphs: Arc<TextGlyphs>,
    last_used: u64,
}

struct KeptPaths {
    paths: Arc<TextPaths>,
    last_used: u64,
}

/// The texts recent frames shaped, and the paths their outlines flattened to, by text and
/// flattening tolerance.
#[derive(Default)]
pub(crate) struct TextCache {
    texts: Vec<KeptText>,
    paths: HashMap<(u64, u32), KeptPaths>,
    next_id: u64,
    /// Texts shaped so far.
    #[cfg(feature = "test-hooks")]
    pub(crate) shaped: usize,
}

impl TextCache {
    /// Drops what neither this frame nor the last one used.
    pub(super) fn begin_frame(&mut self, tick: u64) {
        let recent = |last_used: u64| last_used.saturating_add(1) >= tick;
        self.texts.retain(|kept| recent(kept.last_used));
        self.paths.retain(|_, kept| recent(kept.last_used));
    }

    pub(super) fn clear(&mut self) {
        self.texts.clear();
        self.paths.clear();
    }

    /// The glyphs of the text `key` names, shaped by `shape` unless a recent frame shaped them.
    fn glyphs(
        &mut self,
        key: TextKey,
        tick: u64,
        shape: impl FnOnce(&TextKey) -> crate::Result<TextGlyphs>,
    ) -> crate::Result<Arc<TextGlyphs>> {
        if let Some(kept) = self.texts.iter_mut().find(|kept| kept.key == key) {
            kept.last_used = tick;
            return Ok(Arc::clone(&kept.glyphs));
        }
        let mut glyphs = shape(&key)?;
        self.next_id += 1;
        glyphs.id = self.next_id;
        #[cfg(feature = "test-hooks")]
        {
            self.shaped += 1;
        }
        let glyphs = Arc::new(glyphs);
        self.texts.push(KeptText {
            key,
            glyphs: Arc::clone(&glyphs),
            last_used: tick,
        });
        Ok(glyphs)
    }

    /// The paths of `glyphs` with curves flattened to within `tolerance` pixels, unless a recent
    /// frame flattened them as finely. The tolerance is rounded down to a power of two, so a
    /// text whose scale animates is flattened again only as the scale passes one.
    pub(super) fn paths(
        &mut self,
        glyphs: &TextGlyphs,
        tolerance: f32,
        tick: u64,
    ) -> Result<Arc<TextPaths>, String> {
        let tolerance = 2.0_f32.powi(tolerance.log2().floor().clamp(-60.0, 60.0) as i32);
        let key = (glyphs.id, tolerance.to_bits());
        if let Some(kept) = self.paths.get_mut(&key) {
            kept.last_used = tick;
            return Ok(Arc::clone(&kept.paths));
        }
        let paths = Arc::new(glyphs.paths(tolerance)?);
        self.paths.insert(
            key,
            KeptPaths {
                paths: Arc::clone(&paths),
                last_used: tick,
            },
        );
        Ok(paths)
    }
}

/// The glyphs of `node`'s text with `params` at this frame, shaped when no recent frame shaped
/// the same text, font and layout.
pub(crate) fn shape_text(
    cx: &mut FrameCx<'_>,
    node: NodeId,
    params: &Text2dParams,
) -> crate::Result<Arc<TextGlyphs>> {
    params
        .check()
        .map_err(|details| text_error(node, details))?;
    if params.font_size < MIN_FONT_SIZE {
        return Ok(Arc::default());
    }
    let media = cx.media();
    let layout = params.layout();
    let (request, font_generation, missing) = {
        let mut system = text_system(media)?;
        load_font_families(&mut system, media, &layout)?;
        let missing: Vec<String> = used_font_families(&layout)
            .filter(|family| !family.is_empty() && !system.has_family(family))
            .map(str::to_string)
            .collect();
        (layout.request(&[]), system.font_generation(), missing)
    };
    cx.note_font(&params.font_family);
    for family in missing {
        cx.warn(node, WarningKind::FontFallback { family });
    }
    let key = TextKey {
        request,
        font_generation,
        anchor_x: params.anchor_x(),
        alignment_vertical: params.alignment_vertical,
    };
    let max_glyphs = cx.max_text_glyphs();
    let (_, tick, cache, _) = cx.drawing_cache();
    let glyphs = cache
        .texts
        .glyphs(key, tick, |key| shape(media, key, max_glyphs, node))?;
    if glyphs.skipped > 0 {
        cx.warn(
            node,
            WarningKind::NoOutlines {
                glyphs: glyphs.skipped,
            },
        );
    }
    Ok(glyphs)
}

/// Shapes the text of `key` and takes the outline of each glyph it draws, failing past
/// `max_glyphs` glyphs or [`MAX_GLYPH_COMMANDS`] commands in a glyph.
fn shape(
    media: Option<&dyn MediaStore>,
    key: &TextKey,
    max_glyphs: usize,
    node: NodeId,
) -> crate::Result<TextGlyphs> {
    let mut system = text_system(media)?;
    let shaped = system.shape_changed(&key.request);
    let layout = shaped.place([0.0, 0.0]);
    let width = key.request.max_width.unwrap_or(shaped.measurement.width);
    let origin = [key.anchor_x * width, key.alignment_vertical.offset(&shaped)];
    // `None` for a glyph with no outline.
    let mut outlines: HashMap<GlyphKey, Option<Arc<Path>>> = HashMap::default();
    let (mut glyphs, mut skipped) = (Vec::new(), 0);
    for placed in &layout.glyphs {
        let glyph = placed.key.outline_key();
        let character = placed.item.glyph + 1;
        let outline = match outlines.get(&glyph) {
            Some(outline) => outline.clone(),
            None => {
                let outline = match system.glyph_outline(glyph) {
                    GlyphOutline::Commands(commands) if commands.len() > MAX_GLYPH_COMMANDS => {
                        return Err(text_error(
                            node,
                            format!(
                                "the glyph of character {character} has an outline of {} \
                                 commands, more than the limit of {MAX_GLYPH_COMMANDS} for one \
                                 glyph; use another font",
                                commands.len()
                            ),
                        ));
                    }
                    GlyphOutline::Commands(commands) => Some(Arc::new(outline_path(&commands))),
                    GlyphOutline::Unavailable => None,
                };
                outlines.insert(glyph, outline.clone());
                outline
            }
        };
        match outline {
            None => skipped += 1,
            // Whitespace.
            Some(outline) if outline.iter().next().is_none() => {}
            Some(outline) => glyphs.push(PlacedGlyph {
                origin: [
                    placed.position[0] - origin[0],
                    placed.position[1] - origin[1],
                ],
                character,
                outline,
            }),
        }
    }
    if glyphs.len() > max_glyphs {
        return Err(text_error(
            node,
            format!(
                "text outlines {} glyphs, more than the limit of {max_glyphs} a text node can \
                 draw; split it across nodes, shorten it, or raise the host's text glyph limit \
                 (`--max-text-glyphs` of the lumen CLI and lumen-server, `set_max_text_glyphs` \
                 of the renderer)",
                glyphs.len()
            ),
        ));
    }
    Ok(TextGlyphs {
        id: 0,
        glyphs,
        skipped,
    })
}

/// The path of a glyph's outline commands, every contour closed.
fn outline_path(commands: &[OutlineCommand]) -> Path {
    let mut builder = Path::builder();
    let mut open = false;
    for command in commands {
        match *command {
            OutlineCommand::MoveTo([x, y]) => {
                if open {
                    builder.end(true);
                }
                builder.begin(point(x, y));
                open = true;
            }
            OutlineCommand::LineTo([x, y]) if open => {
                builder.line_to(point(x, y));
            }
            OutlineCommand::QuadTo([cx, cy], [x, y]) if open => {
                builder.quadratic_bezier_to(point(cx, cy), point(x, y));
            }
            OutlineCommand::CurveTo([ax, ay], [bx, by], [x, y]) if open => {
                builder.cubic_bezier_to(point(ax, ay), point(bx, by), point(x, y));
            }
            OutlineCommand::Close if open => {
                builder.end(true);
                open = false;
            }
            _ => {}
        }
    }
    if open {
        builder.end(true);
    }
    builder.build()
}

/// `contour` without points that repeat the one before or close it, or `None` when fewer than
/// three are left.
fn closed(mut contour: Vec<[f32; 2]>) -> Option<Vec<[f32; 2]>> {
    contour.dedup();
    while contour.len() > 1 && contour.first() == contour.last() {
        contour.pop();
    }
    (contour.len() >= 3).then_some(contour)
}

fn point_count(contours: &[Vec<[f32; 2]>]) -> usize {
    contours.iter().map(Vec::len).sum()
}

/// The contours of `glyph` in pixels from the text's position, with curves flattened to within
/// `tolerance` pixels, coarsened until they have at most `max_points` points.
fn flatten_glyph(
    glyph: &PlacedGlyph,
    mut tolerance: f32,
    max_points: usize,
) -> Option<Vec<Vec<[f32; 2]>>> {
    let [x, y] = glyph.origin;
    let at = |p: lyon_path::math::Point| [x + p.x, y - p.y];
    'attempts: for _ in 0..FLATTENING_ATTEMPTS {
        let (mut contours, mut current, mut points) = (Vec::new(), Vec::new(), 0);
        for event in glyph.outline.iter().flattened(tolerance) {
            match event {
                PathEvent::Begin { at: p } => current.push(at(p)),
                PathEvent::Line { to, .. } => current.push(at(to)),
                PathEvent::End { .. } => {
                    if let Some(contour) = closed(std::mem::take(&mut current)) {
                        points += contour.len();
                        contours.push(contour);
                    }
                }
                PathEvent::Quadratic { .. } | PathEvent::Cubic { .. } => {}
            }
            if points + current.len() > max_points {
                tolerance *= 2.0;
                continue 'attempts;
            }
        }
        return Some(contours);
    }
    None
}

impl TextGlyphs {
    /// Each glyph's contours in pixels from the text's position, with curves flattened to within
    /// `tolerance` pixels, coarsened until every glyph has at most `max_glyph_points` points and
    /// all of them at most `max_points`.
    fn flattened(
        &self,
        mut tolerance: f32,
        max_points: usize,
        max_glyph_points: usize,
    ) -> Result<Vec<Vec<Vec<[f32; 2]>>>, String> {
        'attempts: for _ in 0..FLATTENING_ATTEMPTS {
            let (mut glyphs, mut points) = (Vec::with_capacity(self.glyphs.len()), 0);
            for glyph in &self.glyphs {
                let contours =
                    flatten_glyph(glyph, tolerance, max_glyph_points).ok_or_else(|| {
                        format!(
                            "the glyph of character {} flattens to more than {max_glyph_points} \
                         points; use another font",
                            glyph.character
                        )
                    })?;
                points += point_count(&contours);
                if points > max_points {
                    tolerance *= 2.0;
                    continue 'attempts;
                }
                glyphs.push(contours);
            }
            return Ok(glyphs);
        }
        Err(format!(
            "the text flattens to more than {max_points} points; split it across nodes or \
             shorten it"
        ))
    }

    /// Every glyph's contours in pixels from the text's position, with curves flattened to within
    /// `tolerance` pixels, coarsened until they have at most `max_points` points.
    #[cfg(feature = "3d")]
    pub(crate) fn contours(
        &self,
        tolerance: f32,
        max_points: usize,
    ) -> Result<Vec<Vec<[f32; 2]>>, String> {
        Ok(self
            .flattened(tolerance, max_points, max_points)?
            .into_iter()
            .flatten()
            .collect())
    }

    /// The text's outlines as paths of consecutive glyphs, each of at most [`MAX_CHUNK_POINTS`]
    /// points before overlapping contours are joined, with curves flattened to within
    /// `tolerance` pixels.
    fn paths(&self, tolerance: f32) -> Result<TextPaths, String> {
        let glyphs = self.flattened(tolerance, MAX_TEXT_POINTS, MAX_CHUNK_POINTS)?;
        let mut chunks = Vec::new();
        let (mut chunk, mut points) = (Vec::new(), 0);
        for contours in glyphs {
            let count = point_count(&contours);
            if points + count > MAX_CHUNK_POINTS && !chunk.is_empty() {
                chunks.push(chunk_points(std::mem::take(&mut chunk))?);
                points = 0;
            }
            points += count;
            chunk.extend(contours);
        }
        if !chunk.is_empty() {
            chunks.push(chunk_points(chunk)?);
        }
        let bounds = chunks
            .iter()
            .map(|points| point_bounds(points))
            .reduce(|(min, max), (low, high)| {
                (
                    [min[0].min(low[0]), min[1].min(low[1])],
                    [max[0].max(high[0]), max[1].max(high[1])],
                )
            })
            .unwrap_or_default();
        Ok(TextPaths { chunks, bounds })
    }
}

/// The path points of `contours`, joined where they overlap so that the even-odd rule the paths
/// are filled by covers what the nonzero rule does.
fn chunk_points(contours: Vec<Vec<[f32; 2]>>) -> Result<Vec<PathPoint>, String> {
    let contours = nonzero_boundary(&contours, MAX_PATH_POINTS)?.unwrap_or(contours);
    Ok(contour_points(&contours))
}

/// Collects a tessellation's points and triangles, refusing the vertex past `max_vertices`.
struct Triangles {
    points: Vec<[f32; 2]>,
    triangles: Vec<u32>,
    scale: f32,
    max_vertices: usize,
}

impl GeometryBuilder for Triangles {
    fn add_triangle(&mut self, a: VertexId, b: VertexId, c: VertexId) {
        self.triangles.extend([a.0, b.0, c.0]);
    }
}

impl FillGeometryBuilder for Triangles {
    fn add_fill_vertex(&mut self, vertex: FillVertex) -> Result<VertexId, GeometryBuilderError> {
        if self.points.len() >= self.max_vertices {
            return Err(GeometryBuilderError::TooManyVertices);
        }
        let p = vertex.position();
        self.points.push([p.x / self.scale, p.y / self.scale]);
        Ok(VertexId(self.points.len() as u32 - 1))
    }
}

/// The boundary of the area closed `contours` fill by the nonzero rule, as closed contours that
/// fill it by the even-odd rule too: where contours overlap, they are joined, turning where
/// they cross. `None` when `contours` are that boundary already. An error when finding it fails or
/// takes more than `max_points` points, since the contours as they are would fill wrongly.
fn nonzero_boundary(
    contours: &[Vec<[f32; 2]>],
    max_points: usize,
) -> Result<Option<Vec<Vec<[f32; 2]>>>, String> {
    let too_complex = || {
        format!(
            "overlapping glyphs make an outline of more than {max_points} points; \
             overlap the glyphs less or split the text into several nodes"
        )
    };
    // The tessellator's own thresholds suit outlines some hundreds of units across, so outlines
    // are filled at that size, scaled by a power of two that leaves every point as exact.
    let extent = contours
        .iter()
        .flatten()
        .fold(0.0_f32, |extent, p| extent.max(p[0].abs()).max(p[1].abs()));
    let scale = if extent > 0.0 {
        2.0_f32.powi(10 - extent.log2().ceil() as i32)
    } else {
        1.0
    };
    let events = contours.iter().flat_map(|contour| {
        let at = |p: [f32; 2]| point(p[0] * scale, p[1] * scale);
        let first = at(contour[0]);
        std::iter::once(PathEvent::Begin { at: first })
            .chain(contour.windows(2).map(move |pair| PathEvent::Line {
                from: at(pair[0]),
                to: at(pair[1]),
            }))
            .chain(std::iter::once(PathEvent::End {
                last: at(contour[contour.len() - 1]),
                first,
                close: true,
            }))
    });
    let input = point_count(contours);
    let mut built = Triangles {
        points: Vec::new(),
        triangles: Vec::new(),
        scale,
        max_vertices: max_points,
    };
    FillTessellator::new()
        .tessellate(
            events,
            &FillOptions::default().with_fill_rule(FillRule::NonZero),
            &mut built,
        )
        .map_err(|_| too_complex())?;
    let Triangles {
        points,
        mut triangles,
        ..
    } = built;
    // The tessellator winds every triangle one way, so the total area tells which.
    let area: f32 = triangles
        .as_chunks::<3>()
        .0
        .iter()
        .map(|t| {
            let [a, b, c] = t.map(|i| points[i as usize]);
            (b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])
        })
        .sum();
    if area < 0.0 {
        for triangle in triangles.as_chunks_mut::<3>().0 {
            triangle.swap(1, 2);
        }
    }
    // The edges of the triangles that no neighbour shares are the boundary.
    let mut edges: HashMap<(u32, u32), u32> = HashMap::default();
    for triangle in triangles.as_chunks::<3>().0 {
        for i in 0..3 {
            let (a, b) = (triangle[i], triangle[(i + 1) % 3]);
            if a != b {
                *edges.entry((a, b)).or_default() += 1;
            }
        }
    }
    let mut boundary: Vec<(u32, u32)> = edges
        .keys()
        .filter(|&&(a, b)| !edges.contains_key(&(b, a)))
        .copied()
        .collect();
    boundary.sort_unstable();

    // Contours that already are the boundary keep their own starts and directions.
    let key = |p: [f32; 2]| [(p[0] + 0.0).to_bits(), (p[1] + 0.0).to_bits()];
    let segment = |a: [f32; 2], b: [f32; 2]| {
        let (a, b) = (key(a), key(b));
        (a.min(b), a.max(b))
    };
    let mut segments: HashMap<([u32; 2], [u32; 2]), u32> = HashMap::default();
    for contour in contours {
        for (i, &a) in contour.iter().enumerate() {
            let b = contour[(i + 1) % contour.len()];
            *segments.entry(segment(a, b)).or_default() += 1;
        }
    }
    let unchanged = boundary.len() == input
        && boundary.iter().all(|&(a, b)| {
            segments
                .get(&segment(points[a as usize], points[b as usize]))
                .is_some_and(|&count| count == 1)
        });
    if unchanged {
        return Ok(None);
    }

    let mut next: HashMap<u32, Vec<u32>> = HashMap::default();
    for &(a, b) in &boundary {
        next.entry(a).or_default().push(b);
    }
    let mut outlines = Vec::new();
    for (start, _) in boundary {
        let mut contour = Vec::new();
        let mut at = start;
        while let Some(to) = next.get_mut(&at).and_then(Vec::pop) {
            contour.push(points[at as usize]);
            at = to;
            if at == start {
                break;
            }
        }
        if let Some(contour) = closed(contour) {
            outlines.push(contour);
        }
    }
    if point_count(&outlines) > max_points {
        return Err(too_complex());
    }
    Ok(Some(outlines))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn square(min: [f32; 2], size: f32) -> Vec<[f32; 2]> {
        let [x, y] = min;
        vec![[x, y], [x + size, y], [x + size, y + size], [x, y + size]]
    }

    fn area(contours: &[Vec<[f32; 2]>]) -> f32 {
        contours
            .iter()
            .map(|contour| {
                let n = contour.len();
                (0..n)
                    .map(|i| {
                        let (a, b) = (contour[i], contour[(i + 1) % n]);
                        a[0] * b[1] - b[0] * a[1]
                    })
                    .sum::<f32>()
                    * 0.5
            })
            .sum()
    }

    #[test]
    fn overlapping_contours_join_into_one_boundary() {
        // Two squares wound the same way overlap in a 5 by 5 square, which even-odd would leave
        // empty.
        let joined = nonzero_boundary(&[square([0.0, 0.0], 10.0), square([5.0, 5.0], 10.0)], 1024)
            .unwrap()
            .expect("the squares overlap");
        assert_eq!(joined.len(), 1, "{joined:?}");
        assert_eq!(area(&joined).abs(), 175.0);
        // A square inside another wound the same way is covered by it.
        let nested = nonzero_boundary(&[square([0.0, 0.0], 10.0), square([2.0, 2.0], 4.0)], 1024)
            .unwrap()
            .expect("the inner square is no edge");
        assert_eq!(nested.len(), 1, "{nested:?}");
        assert_eq!(area(&nested).abs(), 100.0);
    }

    #[test]
    fn a_boundary_past_the_point_limit_is_an_error() {
        let squares: Vec<_> = (0..8).map(|i| square([i as f32, i as f32], 10.0)).collect();
        assert!(nonzero_boundary(&squares, 16).is_err());
    }

    #[test]
    fn contours_that_are_their_boundary_stay_as_they_are() {
        let mut hole = square([2.0, 2.0], 4.0);
        hole.reverse();
        assert!(
            nonzero_boundary(&[square([0.0, 0.0], 10.0), hole], 1024)
                .unwrap()
                .is_none()
        );
        assert!(
            nonzero_boundary(&[square([0.0, 0.0], 4.0), square([10.0, 0.0], 4.0)], 1024)
                .unwrap()
                .is_none()
        );
    }
}
