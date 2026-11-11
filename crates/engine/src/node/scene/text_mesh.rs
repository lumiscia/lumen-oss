//! Builds the meshes of `text_3d` nodes and keeps them while their text stays the same.

use std::collections::HashSet;
use std::sync::Arc;

use foldhash::HashMap;
use lumen_3d::glam::{Vec2, Vec3};
use lumen_3d::model::{MAX_INDICES, MAX_VERTICES};
use lumen_3d::{Bounds, Extrusion, Mesh};
use lumen_gpu::Recorder;
use lumen_text::{GlyphKey, GlyphOutline, OutlineCommand, TextLayoutRequest};
use lyon_path::iterator::PathIterator;
use lyon_path::math::point;
use lyon_path::{Path, PathEvent};

use super::collect::GpuMesh;
use super::text_3d::Text3dParams;
use crate::error::{LumenError, RenderError};
use crate::gpu::frame_graph::FrameCx;
use crate::media::MediaStore;
use crate::node::NodeId;
use crate::node::source::text::{
    TextAlignmentHorizontal, TextAlignmentVertical, TextLayoutParams, load_font_families,
    text_system, used_font_families,
};
use crate::node::source::text_content::TextContent;

/// Pixels to the em that text is shaped at, before its outlines are scaled to world units.
const SHAPE_EM: f64 = 128.0;
/// Greatest distance between a glyph's curves and the straight edges they are built from, in
/// ems.
const CURVE_TOLERANCE: f64 = 0.001;
const MAX_BEVEL_SEGMENTS: u32 = 16;
/// Bytes of text meshes kept on the GPU beyond those the latest frame drew or a kept output drew.
const TEXT_BUDGET_BYTES: u64 = 64 << 20;
/// Most text meshes kept, counting those of text with nothing to draw.
const MAX_TEXTS: usize = 256;
/// Glyph meshes kept for building other text from, which are dropped together once there are
/// more.
const MAX_GLYPH_MESHES: usize = 4096;

/// A text's mesh on the GPU, with its index count and what it covers before it is placed.
#[derive(Debug, Clone, Copy)]
pub(super) struct TextMesh {
    pub(super) mesh: GpuMesh,
    pub(super) index_count: u32,
    pub(super) bounds: Bounds,
}

/// Everything a text's mesh is built from.
#[derive(Debug, Clone, PartialEq)]
struct TextKey {
    request: TextLayoutRequest,
    font_generation: u64,
    /// World units per pixel of the layout.
    scale: f32,
    extrusion: Extrusion,
    /// The point of the text's box at its origin, as fractions of its width and height.
    anchor: [f32; 2],
}

impl TextKey {
    /// The key of `params`' text, shaped by `request` with fonts of `font_generation`.
    fn new(params: &Text3dParams, request: TextLayoutRequest, font_generation: u64) -> Self {
        Self {
            request,
            font_generation,
            scale: (params.font_size / SHAPE_EM) as f32,
            extrusion: Extrusion {
                depth: params.depth.max(0.0) as f32,
                bevel: params.bevel_size.max(0.0) as f32,
                bevel_segments: params.bevel_segments.min(MAX_BEVEL_SEGMENTS),
            },
            anchor: [
                match params.alignment_horizontal {
                    TextAlignmentHorizontal::Left | TextAlignmentHorizontal::Justify => 0.0,
                    TextAlignmentHorizontal::Center => 0.5,
                    TextAlignmentHorizontal::Right => 1.0,
                },
                match params.alignment_vertical {
                    TextAlignmentVertical::Top => 0.0,
                    TextAlignmentVertical::Middle => 0.5,
                    TextAlignmentVertical::Bottom => 1.0,
                },
            ],
        }
    }

    /// What a glyph's mesh is built from besides the glyph.
    fn glyph_bits(&self) -> [u32; 4] {
        let extrusion = &self.extrusion;
        [
            self.scale.to_bits(),
            extrusion.depth.to_bits(),
            extrusion.bevel.to_bits(),
            extrusion.bevel_segments,
        ]
    }
}

struct CachedText {
    key: TextKey,
    /// Names the mesh among those a kept output holds.
    id: String,
    mesh: Option<TextMesh>,
    /// Glyphs left out for having no outline.
    skipped: usize,
    bytes: u64,
    last_used: u64,
}

/// What the latest frames' `text_3d` nodes drew, and the glyph meshes they were built from.
#[derive(Default)]
pub(crate) struct TextMeshes {
    texts: Vec<CachedText>,
    bytes: u64,
    next_id: u64,
    glyphs: GlyphMeshes,
}

/// Each glyph's mesh at a size and extrusion, or `None` for a glyph with no outline, for the
/// fonts of one generation.
#[derive(Default)]
struct GlyphMeshes {
    font_generation: u64,
    meshes: HashMap<(GlyphKey, [u32; 4]), Option<Arc<Mesh>>>,
}

impl TextMeshes {
    pub(super) fn release(&mut self, rec: &mut Recorder) {
        for text in self.texts.drain(..) {
            if let Some(mesh) = text.mesh {
                mesh.mesh.forget(rec);
            }
        }
        self.bytes = 0;
        self.glyphs = GlyphMeshes::default();
    }

    /// Frees text meshes, least recently drawn first, while more than [`MAX_TEXTS`] or
    /// [`TEXT_BUDGET_BYTES`] are kept, sparing those frame `tick` drew and those `kept` names.
    pub(super) fn trim(&mut self, rec: &mut Recorder, tick: u64, kept: &HashSet<&String>) {
        while self.texts.len() > MAX_TEXTS || self.bytes > TEXT_BUDGET_BYTES {
            let oldest = self
                .texts
                .iter()
                .enumerate()
                .filter(|(_, text)| text.last_used != tick && !kept.contains(&text.id))
                .min_by_key(|(_, text)| text.last_used)
                .map(|(index, _)| index);
            let Some(index) = oldest else {
                break;
            };
            let text = self.texts.swap_remove(index);
            if let Some(mesh) = text.mesh {
                mesh.mesh.forget(rec);
            }
            self.bytes -= text.bytes;
        }
    }

    /// Adds the text `built` from `key`, drawn in frame `tick`, and returns where it is.
    fn insert(&mut self, rec: &mut Recorder, key: TextKey, built: Built, tick: u64) -> usize {
        let mesh = built.mesh.filter(|mesh| !mesh.indices.is_empty());
        let bytes = mesh.as_ref().map_or(0, |mesh| {
            (std::mem::size_of_val(mesh.vertices.as_slice())
                + std::mem::size_of_val(mesh.indices.as_slice())) as u64
        });
        let mesh = mesh.map(|mesh| TextMesh {
            mesh: GpuMesh::upload(rec, &mesh),
            index_count: mesh.indices.len() as u32,
            bounds: Bounds::of(
                mesh.vertices
                    .iter()
                    .map(|vertex| Vec3::from(vertex.position)),
            ),
        });
        self.next_id += 1;
        self.bytes += bytes;
        self.texts.push(CachedText {
            key,
            id: format!("text_3d mesh {}", self.next_id),
            mesh,
            skipped: built.skipped,
            bytes,
            last_used: tick,
        });
        self.texts.len() - 1
    }
}

fn text_error(node_id: NodeId, details: String) -> LumenError {
    RenderError::NodeEvaluation {
        node_id,
        node_kind: "Text3d",
        details,
    }
    .into()
}

/// The layout a 2D `text` node would shape `params`' text with at `em` pixels to the em, so that
/// it lays out the same way.
fn layout(params: &Text3dParams, em: f64) -> TextLayoutParams {
    let pixels = em / params.font_size;
    TextLayoutParams {
        content: TextContent::plain(params.content.clone()),
        font_family: params.font_family.clone(),
        font_size: em,
        font_weight: params.font_weight,
        font_style: params.font_style,
        max_width: params.max_width.max(0.0) * pixels,
        letter_spacing: params.letter_spacing * pixels,
        font_features: params.font_features.clone(),
        line_height: params.line_height.max(0.0) * pixels,
        alignment_horizontal: params.alignment_horizontal,
        text_transform: params.text_transform,
        ..TextLayoutParams::default()
    }
}

/// The mesh of `node_id`'s text with `params` at this frame, built when nothing drawn before
/// has the same text, font, layout and extrusion. `None` when it draws nothing.
pub(super) fn text_mesh(
    cx: &mut FrameCx<'_>,
    node_id: NodeId,
    params: &Text3dParams,
) -> crate::Result<Option<TextMesh>> {
    if !(params.font_size.is_finite() && params.font_size > 0.0) {
        return Ok(None);
    }
    let media = cx.media();
    let layout = layout(params, SHAPE_EM);
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
        cx.note_font_fallback(node_id, family);
    }
    let key = TextKey::new(params, request, font_generation);
    let tick = cx.tick();
    let max_glyphs = cx.max_text_glyphs();
    let (rec, cache) = cx.scene_cache();
    let texts = &mut cache.texts;
    let index = match texts.texts.iter().position(|text| text.key == key) {
        Some(index) => index,
        None => {
            let built =
                build(media, &key, &mut texts.glyphs, max_glyphs).map_err(
                    |failure| match failure {
                        BuildError::Lumen(error) => error,
                        BuildError::Limit(details) => text_error(node_id, details),
                    },
                )?;
            texts.insert(rec, key, built, tick)
        }
    };
    let cached = &mut texts.texts[index];
    cached.last_used = tick;
    let (id, mesh, skipped) = (cached.id.clone(), cached.mesh, cached.skipped);
    cx.drew_model(&id);
    if skipped > 0 {
        cx.note_missing_outlines(node_id, skipped);
    }
    Ok(mesh)
}

struct Built {
    mesh: Option<Mesh>,
    skipped: usize,
}

enum BuildError {
    Lumen(LumenError),
    /// Why the text is more than a node may extrude.
    Limit(String),
}

/// Shapes the text of `key` and puts its glyphs' meshes together, building those that `glyphs`
/// lacks.
fn build(
    media: Option<&dyn MediaStore>,
    key: &TextKey,
    glyphs: &mut GlyphMeshes,
    max_glyphs: usize,
) -> Result<Built, BuildError> {
    if glyphs.font_generation != key.font_generation || glyphs.meshes.len() > MAX_GLYPH_MESHES {
        glyphs.meshes.clear();
        glyphs.font_generation = key.font_generation;
    }
    let bits = key.glyph_bits();
    let (layout, measurement, outlines) = {
        let mut system = text_system(media).map_err(BuildError::Lumen)?;
        let shaped = system.shape_changed(&key.request);
        let layout = shaped.place([0.0, 0.0]);
        let mut outlines: HashMap<GlyphKey, GlyphOutline> = HashMap::default();
        for glyph in &layout.glyphs {
            let outline_key = glyph.key.outline_key();
            if !glyphs.meshes.contains_key(&(outline_key, bits)) {
                outlines
                    .entry(outline_key)
                    .or_insert_with(|| system.glyph_outline(outline_key));
            }
        }
        (layout, shaped.measurement, outlines)
    };

    let draws = |glyph: GlyphKey| match glyphs.meshes.get(&(glyph, bits)) {
        Some(mesh) => mesh.as_ref().is_some_and(|mesh| !mesh.indices.is_empty()),
        None => matches!(outlines.get(&glyph), Some(GlyphOutline::Commands(commands))
            if !commands.is_empty()),
    };
    let drawn = layout
        .glyphs
        .iter()
        .filter(|glyph| draws(glyph.key.outline_key()))
        .count();
    if drawn > max_glyphs {
        return Err(BuildError::Limit(format!(
            "text extrudes {drawn} glyphs, more than the limit of {max_glyphs} a text node can \
             draw; split it across nodes, shorten it, or raise the host's text glyph limit \
             (`--max-text-glyphs` of lumen-local and lumen-server, `set_max_text_glyphs` of the \
             renderer)"
        )));
    }
    let tolerance = (SHAPE_EM * CURVE_TOLERANCE) as f32;
    for (glyph, outline) in outlines {
        let mesh = match outline {
            GlyphOutline::Commands(commands) => Some(Arc::new(lumen_3d::extrude(
                &flatten(&commands, key.scale, tolerance),
                &key.extrusion,
            ))),
            GlyphOutline::Unavailable => None,
        };
        glyphs.meshes.insert((glyph, bits), mesh);
    }

    let width = key.request.max_width.unwrap_or(measurement.width);
    let origin = [key.anchor[0] * width, key.anchor[1] * measurement.height];
    let (mut vertex_count, mut index_count, mut skipped) = (0, 0, 0);
    let mut parts = Vec::new();
    for glyph in &layout.glyphs {
        match glyphs.meshes.get(&(glyph.key.outline_key(), bits)) {
            Some(Some(mesh)) if !mesh.indices.is_empty() => {
                vertex_count += mesh.vertices.len();
                index_count += mesh.indices.len();
                if vertex_count > MAX_VERTICES || index_count > MAX_INDICES {
                    return Err(BuildError::Limit(format!(
                        "the extruded text has more than {MAX_VERTICES} vertices or \
                         {MAX_INDICES} indices; split it across nodes, shorten it, or use fewer \
                         `bevel_segments`"
                    )));
                }
                let offset = Vec3::new(
                    (glyph.position[0] - origin[0]) * key.scale,
                    (origin[1] - glyph.position[1]) * key.scale,
                    0.0,
                );
                parts.push((offset, Arc::clone(mesh)));
            }
            Some(None) => skipped += 1,
            _ => {}
        }
    }
    let mut mesh = Mesh {
        vertices: Vec::with_capacity(vertex_count),
        indices: Vec::with_capacity(index_count),
    };
    for (offset, part) in parts {
        let base = mesh.vertices.len() as u32;
        mesh.vertices.extend(part.vertices.iter().map(|vertex| {
            let mut vertex = *vertex;
            vertex.position = (Vec3::from(vertex.position) + offset).into();
            vertex
        }));
        mesh.indices
            .extend(part.indices.iter().map(|index| base + index));
    }
    Ok(Built {
        mesh: Some(mesh),
        skipped,
    })
}

/// The closed polygons of an outline in pixels, flattened to within `tolerance` pixels and
/// scaled by `scale`.
fn flatten(commands: &[OutlineCommand], scale: f32, tolerance: f32) -> Vec<Vec<Vec2>> {
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
    let mut contours = Vec::new();
    let mut current = Vec::new();
    let at = |p: lyon_path::math::Point| Vec2::new(p.x, p.y) * scale;
    for event in builder.build().iter().flattened(tolerance) {
        match event {
            PathEvent::Begin { at: p } => current.push(at(p)),
            PathEvent::Line { to, .. } => current.push(at(to)),
            PathEvent::End { .. } => contours.push(std::mem::take(&mut current)),
            _ => {}
        }
    }
    contours
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Inter's variable font, whose contours overlap where strokes join.
    const INTER: &[u8] = include_bytes!("../../../../text/assets/inter/InterVariable-Latin.ttf");

    /// The outline of `glyph` in Roboto, one unit to the em.
    fn outline(glyph: &str) -> Vec<Vec<Vec2>> {
        outline_in("Roboto", glyph)
    }

    /// The outline of `glyph` in `family`, Roboto or Inter, one unit to the em.
    fn outline_in(family: &str, glyph: &str) -> Vec<Vec<Vec2>> {
        let mut system = lumen_text::TextSystem::new();
        system.load_font_data(INTER.to_vec());
        let mut request = TextLayoutRequest::new(glyph);
        request.spans[0].font_size = SHAPE_EM as f32;
        request.spans[0].font_family = family.to_string();
        let layout = system.shape(&request).place([0.0, 0.0]);
        let GlyphOutline::Commands(commands) = system.glyph_outline(layout.glyphs[0].key) else {
            panic!("`{glyph}` has an outline");
        };
        flatten(
            &commands,
            1.0 / SHAPE_EM as f32,
            (SHAPE_EM * CURVE_TOLERANCE) as f32,
        )
    }

    fn position(mesh: &Mesh, index: u32) -> Vec3 {
        mesh.vertices[index as usize].position.into()
    }

    /// Directed edges between positions that aren't matched by their reverse exactly once.
    fn open_edges(mesh: &Mesh) -> usize {
        let key = |i: u32| {
            position(mesh, i)
                .to_array()
                .map(|c| (c * 1e5).round() as i64)
        };
        let mut edges: HashMap<([i64; 3], [i64; 3]), i32> = HashMap::default();
        for t in mesh.indices.as_chunks::<3>().0 {
            for i in 0..3 {
                let (a, b) = (key(t[i]), key(t[(i + 1) % 3]));
                if a != b {
                    *edges.entry((a, b)).or_default() += 1;
                }
            }
        }
        edges
            .iter()
            .filter(|&(&(a, b), &count)| count != 1 || edges.get(&(b, a)) != Some(&1))
            .count()
    }

    /// Triangles wound against the normals of their vertices.
    fn inward_faces(mesh: &Mesh) -> usize {
        mesh.indices
            .as_chunks::<3>()
            .0
            .iter()
            .filter(|t| {
                let [a, b, c] = [0, 1, 2].map(|i| position(mesh, t[i]));
                let normal: Vec3 = t
                    .iter()
                    .map(|&i| Vec3::from(mesh.vertices[i as usize].normal))
                    .sum();
                let face = (b - a).cross(c - a);
                face.length_squared() > 1e-14 && face.dot(normal) <= 0.0
            })
            .count()
    }

    #[test]
    fn glyphs_extrude_into_closed_solids_facing_out_with_open_counters() {
        let extrusions = [(0.0, 0), (0.03, 4), (0.05, 1), (0.2, 3)];
        let glyphs = ["o", "e", "8", "@", "A", "g", "B", "&"];
        let fonts = glyphs
            .map(|g| ("Roboto", g))
            .into_iter()
            .chain(glyphs.map(|g| ("Inter", g)));
        for (family, glyph) in fonts {
            let contours = outline_in(family, glyph);
            for (bevel, bevel_segments) in extrusions {
                let extrusion = Extrusion {
                    depth: 0.3,
                    bevel,
                    bevel_segments,
                };
                let mesh = lumen_3d::extrude(&contours, &extrusion);
                let case = format!("{family} `{glyph}` bevelled {bevel} in {bevel_segments}");
                assert!(!mesh.indices.is_empty(), "{case}");
                assert_eq!(open_edges(&mesh), 0, "{case} is closed");
                assert_eq!(inward_faces(&mesh), 0, "{case} faces out");
            }
        }
        // The middle of an "o" is its counter, which no cap covers.
        let o = lumen_3d::extrude(
            &outline("o"),
            &Extrusion {
                depth: 0.3,
                bevel: 0.0,
                bevel_segments: 0,
            },
        );
        let bounds = Bounds::of(o.vertices.iter().map(|v| Vec3::from(v.position)));
        let middle = ((bounds.min + bounds.max) * 0.5).truncate();
        let covered = o.indices.as_chunks::<3>().0.iter().any(|t| {
            let [a, b, c] = [0, 1, 2].map(|i| position(&o, t[i]).truncate() - middle);
            let sides = [(a, b), (b, c), (c, a)].map(|(p, q)| p.perp_dot(q) > 0.0);
            sides.iter().all(|&s| s) || sides.iter().all(|&s| !s)
        });
        assert!(!covered);
    }

    fn built(params: Text3dParams, glyphs: &mut GlyphMeshes, max: usize) -> Result<Built, String> {
        let request = layout(&params, SHAPE_EM).request(&[]);
        let generation = text_system(None).unwrap().font_generation();
        build(
            None,
            &TextKey::new(&params, request, generation),
            glyphs,
            max,
        )
        .map_err(|failure| match failure {
            BuildError::Lumen(error) => error.to_string(),
            BuildError::Limit(details) => details,
        })
    }

    fn text(content: &str) -> Text3dParams {
        Text3dParams {
            content: content.to_string(),
            ..Text3dParams::default()
        }
    }

    #[test]
    fn repeated_glyphs_share_one_mesh_and_whitespace_draws_nothing() {
        let mut glyphs = GlyphMeshes::default();
        let mesh = built(text("lll l"), &mut glyphs, 100)
            .unwrap()
            .mesh
            .unwrap();
        let one = built(text("l"), &mut GlyphMeshes::default(), 100)
            .unwrap()
            .mesh
            .unwrap();
        assert_eq!(mesh.vertices.len(), 4 * one.vertices.len());
        assert_eq!(glyphs.meshes.len(), 2, "an `l` and a space");

        for blank in ["", "   ", "\n\n"] {
            let mesh = built(text(blank), &mut glyphs, 100).unwrap().mesh.unwrap();
            assert!(mesh.indices.is_empty(), "{blank:?}");
        }
    }

    #[test]
    fn text_is_centred_on_its_position_by_default_and_anchored_by_its_alignment() {
        let bounds = |params: Text3dParams| {
            let mesh = built(params, &mut GlyphMeshes::default(), 100)
                .unwrap()
                .mesh
                .unwrap();
            Bounds::of(mesh.vertices.iter().map(|v| Vec3::from(v.position)))
        };
        let centred = bounds(text("HH"));
        assert!((centred.min.x + centred.max.x).abs() < 0.05, "{centred:?}");
        assert!((centred.min.z + 0.125).abs() < 1e-6 && (centred.max.z - 0.125).abs() < 1e-6);
        let left_top = bounds(Text3dParams {
            alignment_horizontal: TextAlignmentHorizontal::Left,
            alignment_vertical: TextAlignmentVertical::Top,
            ..text("HH")
        });
        assert!(
            left_top.min.x >= 0.0 && left_top.min.x < 0.15,
            "{left_top:?}"
        );
        assert!(left_top.max.y < 0.0, "{left_top:?}");
        // Capitals are about 0.7 of the font size tall.
        let height = centred.max.y - centred.min.y;
        assert!((height - 0.71).abs() < 0.02, "{height}");
    }

    #[test]
    fn text_past_the_glyph_limit_fails_counting_drawn_glyphs() {
        let mut glyphs = GlyphMeshes::default();
        assert!(built(text("ab  cd"), &mut glyphs, 4).is_ok());
        let error = built(text("abcde"), &mut glyphs, 4).err().unwrap();
        assert!(
            error.contains("text extrudes 5 glyphs, more than the limit of 4"),
            "{error}"
        );
    }
}
