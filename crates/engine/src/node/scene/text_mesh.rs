//! Builds the meshes of `text_3d` nodes and keeps them while their text stays the same.

use std::collections::HashSet;
use std::sync::Arc;

use foldhash::HashMap;
use lumen_3d::glam::{Vec2, Vec3};
use lumen_3d::model::{MAX_INDICES, MAX_VERTICES};
use lumen_3d::{Bounds, ExtrudeLimit, ExtrudeLimits, Extrusion, LimitExceeded, Mesh};
use lumen_gpu::Recorder;
use lumen_text::{GlyphKey, GlyphOutline, OutlineCommand, TextLayoutRequest};
use lyon_path::iterator::PathIterator;
use lyon_path::math::point;
use lyon_path::{Path, PathEvent};

use super::collect::GpuMesh;
use super::text_3d::{MIN_FONT_SIZE, Text3dParams};
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
/// Bytes of glyph meshes kept in memory for building other text from, which the least recently
/// used give way to. A glyph mesh larger than this is not kept.
const GLYPH_BUDGET_BYTES: u64 = 64 << 20;
/// Most glyph meshes kept, counting glyphs with nothing to draw.
const MAX_GLYPH_MESHES: usize = 4096;
/// Most commands in one glyph's outline.
const MAX_GLYPH_COMMANDS: usize = 4096;
/// Most work a node's distinct glyphs may take to build together, where a glyph's work is the
/// square of its flattened points, which is what joining and filling its contours can cost.
/// Four glyphs at the limit of points, or thousands of ordinary ones.
const MAX_GLYPH_WORK: u64 = 4 * 8192 * 8192;
/// What one glyph may take to build, from its outline to its mesh. A glyph past any of them fails
/// the text, naming the limit.
const GLYPH_LIMITS: ExtrudeLimits = ExtrudeLimits {
    max_contours: 64,
    max_points: 8192,
    max_bevel_points: 3072,
    max_vertices: 262_144,
    max_indices: 786_432,
};

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
                depth: params.depth as f32,
                bevel: params.bevel_size as f32,
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

    /// The extrusion that glyph meshes are built with, in ems, and the factor that scales their Z
    /// when they are placed. A glyph mesh depends on the text's size and depth only where a
    /// bevel ties them together, so animating either of a plain extrusion builds nothing again.
    fn glyph_extrusion(&self) -> (Extrusion, f32) {
        let Extrusion {
            depth,
            bevel,
            bevel_segments,
        } = self.extrusion;
        if depth > 0.0 && bevel > 0.0 && bevel_segments > 0 {
            let em = self.scale * SHAPE_EM as f32;
            let extrusion = Extrusion {
                depth: depth / em,
                bevel: bevel / em,
                bevel_segments,
            };
            (extrusion, em)
        } else {
            let extrusion = Extrusion {
                depth: if depth > 0.0 { 1.0 } else { 0.0 },
                bevel: 0.0,
                bevel_segments: 0,
            };
            (extrusion, depth)
        }
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

/// What a glyph's mesh is built from besides the glyph: its extrusion in ems.
type GlyphBuild = [u32; 3];

struct CachedGlyph {
    /// `None` for a glyph with no outline.
    mesh: Option<Arc<Mesh>>,
    /// What building it took of [`MAX_GLYPH_WORK`].
    work: u64,
    bytes: u64,
    last_used: u64,
}

/// Each glyph's mesh in ems at an extrusion, for the fonts of one generation, within
/// [`GLYPH_BUDGET_BYTES`] and [`MAX_GLYPH_MESHES`].
#[derive(Default)]
struct GlyphMeshes {
    font_generation: u64,
    meshes: HashMap<(GlyphKey, GlyphBuild), CachedGlyph>,
    bytes: u64,
    /// Orders the uses of glyph meshes.
    clock: u64,
}

impl GlyphMeshes {
    /// Forgets every mesh unless they are of `font_generation`'s fonts.
    fn use_fonts(&mut self, font_generation: u64) {
        if self.font_generation != font_generation {
            *self = Self {
                font_generation,
                clock: self.clock,
                ..Self::default()
            };
        }
    }

    /// The mesh of `glyph` and the work it took to build, if it is kept, which counts as using it.
    fn get(&mut self, glyph: &(GlyphKey, GlyphBuild)) -> Option<(Option<Arc<Mesh>>, u64)> {
        self.clock += 1;
        let cached = self.meshes.get_mut(glyph)?;
        cached.last_used = self.clock;
        Some((cached.mesh.clone(), cached.work))
    }

    /// Whether `glyph` is kept and has anything to draw, or `None` if it isn't kept.
    fn draws(&self, glyph: &(GlyphKey, GlyphBuild)) -> Option<bool> {
        let cached = self.meshes.get(glyph)?;
        Some(
            cached
                .mesh
                .as_ref()
                .is_some_and(|mesh| !mesh.indices.is_empty()),
        )
    }

    /// Keeps `mesh` unless it is larger than the budget, freeing the least recently used meshes
    /// while more than the budget is kept.
    fn insert(&mut self, glyph: (GlyphKey, GlyphBuild), mesh: Option<Arc<Mesh>>, work: u64) {
        let bytes = mesh.as_ref().map_or(0, |mesh| glyph_bytes(mesh));
        if bytes > GLYPH_BUDGET_BYTES {
            return;
        }
        self.clock += 1;
        self.bytes += bytes;
        let replaced = self.meshes.insert(
            glyph,
            CachedGlyph {
                mesh,
                work,
                bytes,
                last_used: self.clock,
            },
        );
        self.bytes -= replaced.map_or(0, |replaced| replaced.bytes);
        while self.bytes > GLYPH_BUDGET_BYTES || self.meshes.len() > MAX_GLYPH_MESHES {
            let Some((&oldest, _)) = self
                .meshes
                .iter()
                .min_by_key(|(_, cached)| cached.last_used)
            else {
                break;
            };
            self.bytes -= self
                .meshes
                .remove(&oldest)
                .map_or(0, |removed| removed.bytes);
        }
    }
}

/// The bytes a mesh's vertices and indices take on the GPU.
fn mesh_bytes(mesh: &Mesh) -> u64 {
    (std::mem::size_of_val(mesh.vertices.as_slice())
        + std::mem::size_of_val(mesh.indices.as_slice())) as u64
}

/// The bytes a kept glyph mesh holds in memory: what its buffers have room for, and the mesh
/// with its reference counts.
fn glyph_bytes(mesh: &Mesh) -> u64 {
    (mesh.vertices.capacity() * std::mem::size_of::<lumen_3d::Vertex>()
        + mesh.indices.capacity() * std::mem::size_of::<u32>()
        + std::mem::size_of::<Mesh>()
        + 2 * std::mem::size_of::<usize>()) as u64
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
        let bytes = mesh.as_ref().map_or(0, mesh_bytes);
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
        max_width: params.max_width * pixels,
        letter_spacing: params.letter_spacing * pixels,
        font_features: params.font_features.clone(),
        line_height: params.line_height * pixels,
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
    params
        .check()
        .map_err(|details| text_error(node_id, details))?;
    if params.font_size < MIN_FONT_SIZE {
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
            let limits = BuildLimits {
                max_glyphs,
                glyph: GLYPH_LIMITS,
                max_work: MAX_GLYPH_WORK,
            };
            let built =
                build(media, &key, &mut texts.glyphs, &limits).map_err(
                    |failure| match failure {
                        BuildError::Lumen(error) => error,
                        BuildError::Rejected(details) => text_error(node_id, details),
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
    /// Why the text can't be extruded: it is more than a node or a glyph may take, or its mesh
    /// is unusable.
    Rejected(String),
}

/// What one text may take to build.
struct BuildLimits {
    /// Most glyphs it may draw.
    max_glyphs: usize,
    glyph: ExtrudeLimits,
    /// Most work its distinct glyphs may take together, see [`MAX_GLYPH_WORK`].
    max_work: u64,
}

/// Shapes the text of `key` and puts its glyphs' meshes together, building those that `glyphs`
/// lacks. Each glyph is held to `limits.glyph` as it is built, and counts against what the node
/// may extrude as soon as it is built, so the build stops at the first glyph past either. Each
/// distinct glyph's work counts before its contours are joined, whether its mesh was kept or not,
/// so the same text fails the same way whatever was built before it.
fn build(
    media: Option<&dyn MediaStore>,
    key: &TextKey,
    glyphs: &mut GlyphMeshes,
    limits: &BuildLimits,
) -> Result<Built, BuildError> {
    let BuildLimits {
        max_glyphs,
        glyph: ref glyph_limits,
        max_work,
    } = *limits;
    glyphs.use_fonts(key.font_generation);
    let (extrusion, z_scale) = key.glyph_extrusion();
    let build_key: GlyphBuild = [
        extrusion.depth.to_bits(),
        extrusion.bevel.to_bits(),
        extrusion.bevel_segments,
    ];
    let (layout, measurement, outlined) = {
        let mut system = text_system(media).map_err(BuildError::Lumen)?;
        let shaped = system.shape_changed(&key.request);
        let layout = shaped.place([0.0, 0.0]);
        // Whether each glyph without a mesh yet has an outline to draw.
        let mut outlined: HashMap<GlyphKey, bool> = HashMap::default();
        for placed in &layout.glyphs {
            let glyph = placed.key.outline_key();
            if glyphs.draws(&(glyph, build_key)).is_some() || outlined.contains_key(&glyph) {
                continue;
            }
            let commands = match system.glyph_outline(glyph) {
                GlyphOutline::Commands(commands) => commands.len(),
                GlyphOutline::Unavailable => 0,
            };
            if commands > MAX_GLYPH_COMMANDS {
                return Err(GlyphFailure::TooManyCommands(commands).in_node(
                    placed.item.glyph + 1,
                    glyph_limits,
                    glyph_limits,
                ));
            }
            outlined.insert(glyph, commands > 0);
        }
        (layout, shaped.measurement, outlined)
    };

    let drawn = layout
        .glyphs
        .iter()
        .filter(|glyph| {
            let glyph = glyph.key.outline_key();
            glyphs
                .draws(&(glyph, build_key))
                .unwrap_or_else(|| outlined.get(&glyph) == Some(&true))
        })
        .count();
    if drawn > max_glyphs {
        return Err(BuildError::Rejected(format!(
            "text extrudes {drawn} glyphs, more than the limit of {max_glyphs} a text node can \
             draw; split it across nodes, shorten it, or raise the host's text glyph limit \
             (`--max-text-glyphs` of lumen-local and lumen-server, `set_max_text_glyphs` of the \
             renderer)"
        )));
    }

    let em = key.scale * SHAPE_EM as f32;
    let width = key.request.max_width.unwrap_or(measurement.width);
    let origin = [key.anchor[0] * width, key.anchor[1] * measurement.height];
    let (mut vertex_count, mut index_count, mut skipped) = (0, 0, 0);
    let mut work_left = max_work;
    // The glyphs whose work has counted.
    let mut counted: HashSet<GlyphKey> = HashSet::new();
    let mut parts = Vec::new();
    for glyph in &layout.glyphs {
        let outline_key = glyph.key.outline_key();
        let character = glyph.item.glyph + 1;
        let mesh = match glyphs.get(&(outline_key, build_key)) {
            Some((mesh, work)) => {
                if counted.insert(outline_key) {
                    work_left = work_left.checked_sub(work).ok_or_else(|| {
                        GlyphFailure::TooMuchWork.in_node(character, glyph_limits, glyph_limits)
                    })?;
                }
                mesh
            }
            None => {
                let limits = ExtrudeLimits {
                    max_vertices: glyph_limits.max_vertices.min(MAX_VERTICES - vertex_count),
                    max_indices: glyph_limits.max_indices.min(MAX_INDICES - index_count),
                    ..*glyph_limits
                };
                let (mesh, work) =
                    glyph_mesh(media, outline_key, &extrusion, &limits, work_left)
                        .map_err(|failure| failure.in_node(character, &limits, glyph_limits))?;
                work_left -= work;
                counted.insert(outline_key);
                glyphs.insert((outline_key, build_key), mesh.clone(), work);
                mesh
            }
        };
        match mesh {
            Some(mesh) if !mesh.indices.is_empty() => {
                vertex_count += mesh.vertices.len();
                index_count += mesh.indices.len();
                if vertex_count > MAX_VERTICES || index_count > MAX_INDICES {
                    return Err(BuildError::Rejected(node_limit_message()));
                }
                let offset = Vec3::new(
                    (glyph.position[0] - origin[0]) * key.scale,
                    (origin[1] - glyph.position[1]) * key.scale,
                    0.0,
                );
                parts.push((offset, mesh));
            }
            Some(_) => {}
            None => skipped += 1,
        }
    }
    let mut mesh = Mesh {
        vertices: Vec::with_capacity(vertex_count),
        indices: Vec::with_capacity(index_count),
    };
    let scale = Vec3::new(em, em, z_scale);
    for (offset, part) in parts {
        let base = mesh.vertices.len() as u32;
        for vertex in &part.vertices {
            let mut vertex = *vertex;
            vertex.position = (Vec3::from(vertex.position) * scale + offset).into();
            if !(vertex.position.iter().all(|c| c.is_finite())) {
                return Err(BuildError::Rejected(
                    "the extruded text has a vertex that is not finite; check the node's sizes, \
                     which may be too large"
                        .to_string(),
                ));
            }
            mesh.vertices.push(vertex);
        }
        mesh.indices
            .extend(part.indices.iter().map(|index| base + index));
    }
    Ok(Built {
        mesh: Some(mesh),
        skipped,
    })
}

fn node_limit_message() -> String {
    format!(
        "the extruded text has more than {MAX_VERTICES} vertices or {MAX_INDICES} indices; split \
         it across nodes, shorten it, or use fewer `bevel_segments`"
    )
}

/// Why a glyph's mesh could not be built.
enum GlyphFailure {
    Lumen(LumenError),
    TooManyCommands(usize),
    /// The node's glyphs take more than its work to build.
    TooMuchWork,
    Limit(LimitExceeded),
}

impl GlyphFailure {
    /// The failure of the glyph of the text's character number `character`, built with `limits`.
    fn in_node(
        self,
        character: u32,
        limits: &ExtrudeLimits,
        glyph_limits: &ExtrudeLimits,
    ) -> BuildError {
        match self {
            Self::Lumen(error) => BuildError::Lumen(error),
            Self::TooManyCommands(commands) => BuildError::Rejected(format!(
                "the glyph of character {character} has an outline of {commands} commands, more \
                 than the limit of {MAX_GLYPH_COMMANDS} for one glyph; use another font"
            )),
            Self::TooMuchWork => BuildError::Rejected(format!(
                "the text's glyphs are too complex to extrude in one node, as of the glyph of \
                 character {character}; split it across nodes, shorten it, or use a font with \
                 simpler outlines"
            )),
            // The glyph fits what one may take, but not what the node has left.
            Self::Limit(LimitExceeded {
                limit: ExtrudeLimit::Vertices,
                ..
            }) if limits.max_vertices < glyph_limits.max_vertices => {
                BuildError::Rejected(node_limit_message())
            }
            Self::Limit(LimitExceeded {
                limit: ExtrudeLimit::Indices,
                ..
            }) if limits.max_indices < glyph_limits.max_indices => {
                BuildError::Rejected(node_limit_message())
            }
            Self::Limit(exceeded) => BuildError::Rejected(format!(
                "the glyph of character {character} is too complex to extrude: it has {exceeded} \
                 for one glyph; {}",
                match exceeded.limit {
                    ExtrudeLimit::BevelPoints => "use a plain `bevel_size` of 0 or another font",
                    ExtrudeLimit::Vertices | ExtrudeLimit::Indices =>
                        "use fewer `bevel_segments` or another font",
                    ExtrudeLimit::Contours | ExtrudeLimit::Points | ExtrudeLimit::Crossings => {
                        "use another font"
                    }
                }
            )),
        }
    }
}

/// The mesh of the glyph `glyph` in ems, or `None` if its font has no outline for it, and the
/// work it took, which fails it before its contours are joined if it is more than `work_left`.
fn glyph_mesh(
    media: Option<&dyn MediaStore>,
    glyph: GlyphKey,
    extrusion: &Extrusion,
    limits: &ExtrudeLimits,
    work_left: u64,
) -> Result<(Option<Arc<Mesh>>, u64), GlyphFailure> {
    let outline = text_system(media)
        .map_err(GlyphFailure::Lumen)?
        .glyph_outline(glyph);
    let GlyphOutline::Commands(commands) = outline else {
        return Ok((None, 0));
    };
    if commands.len() > MAX_GLYPH_COMMANDS {
        return Err(GlyphFailure::TooManyCommands(commands.len()));
    }
    let tolerance = (SHAPE_EM * CURVE_TOLERANCE) as f32;
    let contours = flatten(&commands, 1.0 / SHAPE_EM as f32, tolerance, limits)
        .map_err(GlyphFailure::Limit)?;
    let points = contours.iter().map(Vec::len).sum::<usize>() as u64;
    let work = points * points;
    if work > work_left {
        return Err(GlyphFailure::TooMuchWork);
    }
    let mut mesh = lumen_3d::extrude(&contours, extrusion, limits).map_err(GlyphFailure::Limit)?;
    mesh.vertices.shrink_to_fit();
    mesh.indices.shrink_to_fit();
    Ok((Some(Arc::new(mesh)), work))
}

/// The closed polygons of an outline in pixels, flattened to within `tolerance` pixels and
/// scaled by `scale`. Fails as soon as the outline has more contours or flattened points than
/// `limits` allow.
fn flatten(
    commands: &[OutlineCommand],
    scale: f32,
    tolerance: f32,
    limits: &ExtrudeLimits,
) -> Result<Vec<Vec<Vec2>>, LimitExceeded> {
    let starts = commands
        .iter()
        .filter(|command| matches!(command, OutlineCommand::MoveTo(_)))
        .count();
    if starts > limits.max_contours {
        return Err(LimitExceeded {
            limit: ExtrudeLimit::Contours,
            max: limits.max_contours,
        });
    }
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
    let mut points = 0;
    let at = |p: lyon_path::math::Point| Vec2::new(p.x, p.y) * scale;
    for event in builder.build().iter().flattened(tolerance) {
        match event {
            PathEvent::Begin { at: p } => current.push(at(p)),
            PathEvent::Line { to, .. } => current.push(at(to)),
            PathEvent::End { .. } => contours.push(std::mem::take(&mut current)),
            _ => {}
        }
        points += 1;
        if points > limits.max_points {
            return Err(LimitExceeded {
                limit: ExtrudeLimit::Points,
                max: limits.max_points,
            });
        }
    }
    Ok(contours)
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
            &ExtrudeLimits::NONE,
        )
        .unwrap()
    }

    fn extrude(contours: &[Vec<Vec2>], extrusion: &Extrusion) -> Mesh {
        lumen_3d::extrude(contours, extrusion, &ExtrudeLimits::NONE).unwrap()
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
                let mesh = extrude(&contours, &extrusion);
                let case = format!("{family} `{glyph}` bevelled {bevel} in {bevel_segments}");
                assert!(!mesh.indices.is_empty(), "{case}");
                assert_eq!(open_edges(&mesh), 0, "{case} is closed");
                assert_eq!(inward_faces(&mesh), 0, "{case} faces out");
            }
        }
        // The middle of an "o" is its counter, which no cap covers.
        let o = extrude(
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
        built_within(params, glyphs, max, &GLYPH_LIMITS)
    }

    fn built_within(
        params: Text3dParams,
        glyphs: &mut GlyphMeshes,
        max: usize,
        limits: &ExtrudeLimits,
    ) -> Result<Built, String> {
        built_with_work(params, glyphs, max, limits, MAX_GLYPH_WORK)
    }

    fn built_with_work(
        params: Text3dParams,
        glyphs: &mut GlyphMeshes,
        max_glyphs: usize,
        limits: &ExtrudeLimits,
        max_work: u64,
    ) -> Result<Built, String> {
        let request = layout(&params, SHAPE_EM).request(&[]);
        let generation = text_system(None).unwrap().font_generation();
        build(
            None,
            &TextKey::new(&params, request, generation),
            glyphs,
            &BuildLimits {
                max_glyphs,
                glyph: *limits,
                max_work,
            },
        )
        .map_err(|failure| match failure {
            BuildError::Lumen(error) => error.to_string(),
            BuildError::Rejected(details) => details,
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

    /// Text of every glyph of Roboto in a few of its blocks, past what a node may extrude when
    /// bevelled.
    fn every_glyph() -> String {
        ('!'..='\u{24F}')
            .chain('\u{2000}'..='\u{2BFF}')
            .filter(|c| !c.is_whitespace())
            .collect()
    }

    fn bevelled(content: &str, bevel_size: f64) -> Text3dParams {
        Text3dParams {
            bevel_size,
            bevel_segments: 16,
            depth: 0.3,
            ..text(content)
        }
    }

    fn mesh_of(vertices: usize) -> Option<Arc<Mesh>> {
        let vertex = lumen_3d::Vertex {
            position: [0.0; 3],
            normal: [0.0, 0.0, 1.0],
            uv: [0.0; 2],
        };
        Some(Arc::new(Mesh {
            vertices: vec![vertex; vertices],
            indices: vec![0; 3],
        }))
    }

    /// The key of the first glyph of an `l`.
    fn some_glyph() -> GlyphKey {
        let params = text("l");
        let request = layout(&params, SHAPE_EM).request(&[]);
        let layout = text_system(None).unwrap().shape(&request).place([0.0, 0.0]);
        layout.glyphs[0].key.outline_key()
    }

    #[test]
    fn a_glyph_past_a_limit_fails_the_text_naming_it() {
        let limits = |change: fn(&mut ExtrudeLimits)| {
            let mut limits = GLYPH_LIMITS;
            change(&mut limits);
            built_within(
                bevelled("@", 0.03),
                &mut GlyphMeshes::default(),
                100,
                &limits,
            )
            .err()
            .expect("the glyph is past the limit")
        };
        let contours = limits(|l| l.max_contours = 1);
        assert!(
            contours.contains("character 1") && contours.contains("more than 1 contours"),
            "{contours}"
        );
        let points = limits(|l| l.max_points = 50);
        assert!(points.contains("more than 50 points"), "{points}");
        let bevel = limits(|l| l.max_bevel_points = 50);
        assert!(
            bevel.contains("more than 50 points to fit a bevel to"),
            "{bevel}"
        );
        let vertices = limits(|l| l.max_vertices = 1000);
        assert!(vertices.contains("more than 1000 vertices"), "{vertices}");
        let indices = limits(|l| l.max_indices = 1000);
        assert!(indices.contains("more than 1000 indices"), "{indices}");
        // Without a bevel, the same glyph needs no fitting.
        let mut tight = GLYPH_LIMITS;
        tight.max_bevel_points = 50;
        assert!(built_within(text("@"), &mut GlyphMeshes::default(), 100, &tight).is_ok());
    }

    #[test]
    fn an_outline_past_the_flattening_limits_stops_before_building_its_points() {
        let square = |size: f32, curves: usize| {
            let mut commands = vec![OutlineCommand::MoveTo([0.0, 0.0])];
            for _ in 0..curves {
                commands.push(OutlineCommand::CurveTo(
                    [size, 0.0],
                    [size, size],
                    [0.0, size],
                ));
            }
            commands.push(OutlineCommand::Close);
            commands
        };
        let limits = ExtrudeLimits {
            max_points: 1000,
            ..GLYPH_LIMITS
        };
        // Curves ten thousand times the size of a pixel flatten to millions of points.
        let huge = flatten(&square(1e7, 1), 1.0, 0.128, &limits).unwrap_err();
        assert_eq!(huge.limit, ExtrudeLimit::Points);
        assert!(flatten(&square(30.0, 4), 1.0, 0.128, &limits).is_ok());
        let many: Vec<_> = (0..65)
            .flat_map(|_| [OutlineCommand::MoveTo([0.0, 0.0]), OutlineCommand::Close])
            .collect();
        let many = flatten(&many, 1.0, 0.128, &limits).unwrap_err();
        assert_eq!(many.limit, ExtrudeLimit::Contours);
    }

    #[test]
    fn the_node_limit_stops_the_build_at_the_first_glyph_past_it() {
        let letters = every_glyph();
        let distinct = letters.chars().count();
        let mut glyphs = GlyphMeshes::default();
        let error = built(bevelled(&letters, 0.03), &mut glyphs, 100_000)
            .err()
            .expect("the text has more vertices than a node may extrude");
        assert!(error.contains("more than 4000000 vertices"), "{error}");
        assert!(
            glyphs.meshes.len() < distinct / 2,
            "{} of {distinct} glyphs were built",
            glyphs.meshes.len()
        );
    }

    #[test]
    fn glyphs_past_the_work_of_a_node_fail_it_whether_or_not_their_meshes_are_kept() {
        let mut glyphs = GlyphMeshes::default();
        let work = |glyphs: &mut GlyphMeshes, max_work| {
            built_with_work(text("@&8"), glyphs, 100, &GLYPH_LIMITS, max_work)
        };
        let error = work(&mut glyphs, 100).err().expect("an `@` is more work");
        assert!(
            error.contains("too complex to extrude in one node") && error.contains("character 1"),
            "{error}"
        );
        assert!(
            glyphs.meshes.is_empty(),
            "the glyph failed before it was built"
        );
        work(&mut glyphs, MAX_GLYPH_WORK).unwrap();
        let spent: u64 = glyphs.meshes.values().map(|glyph| glyph.work).sum();
        assert!(spent > 0 && spent < MAX_GLYPH_WORK / 1000, "{spent}");
        // Kept meshes count the same, once for each distinct glyph.
        assert!(work(&mut glyphs, spent).is_ok() && work(&mut glyphs, spent - 1).is_err());
        assert!(built_with_work(text("@@@&8"), &mut glyphs, 100, &GLYPH_LIMITS, spent).is_ok());
        // Every glyph of a font's Latin and symbol blocks is a fraction of a node's work.
        let mut glyphs = GlyphMeshes::default();
        built(text(&every_glyph()), &mut glyphs, 100_000).unwrap();
        let spent: u64 = glyphs.meshes.values().map(|glyph| glyph.work).sum();
        let distinct = glyphs.meshes.len();
        assert!(
            distinct > 200 && spent < MAX_GLYPH_WORK / 4,
            "{spent} for {distinct} glyphs"
        );
    }

    #[test]
    fn kept_glyph_meshes_count_the_room_of_their_buffers() {
        let mut mesh = Mesh {
            vertices: Vec::with_capacity(1000),
            indices: Vec::with_capacity(3000),
        };
        mesh.indices.extend([0, 0, 0]);
        assert!(glyph_bytes(&mesh) > 1000 * 32 + 3000 * 4);
        let mut glyphs = GlyphMeshes::default();
        built(text("@"), &mut glyphs, 100).unwrap();
        for glyph in glyphs.meshes.values() {
            let mesh = glyph.mesh.as_ref().unwrap();
            assert_eq!(mesh.vertices.capacity(), mesh.vertices.len());
            assert_eq!(mesh.indices.capacity(), mesh.indices.len());
        }
    }

    #[test]
    fn glyph_meshes_stay_within_their_budget_and_entry_limit() {
        let mut glyphs = GlyphMeshes::default();
        let glyph = |index: u32| {
            let mut key = some_glyph();
            key.0.glyph_id = index as u16;
            (key, [index, 0, 0])
        };
        // Meshes of two fifths of the budget each: only two fit.
        let third =
            (GLYPH_BUDGET_BYTES * 2 / 5 / std::mem::size_of::<lumen_3d::Vertex>() as u64) as usize;
        for index in 0..5 {
            glyphs.insert(glyph(index), mesh_of(third), 0);
            assert!(glyphs.bytes <= GLYPH_BUDGET_BYTES);
        }
        assert_eq!(glyphs.meshes.len(), 2);
        assert!(glyphs.get(&glyph(4)).is_some() && glyphs.get(&glyph(3)).is_some());
        assert!(
            glyphs.get(&glyph(2)).is_none(),
            "the least recently used goes first"
        );
        // A mesh used more recently outlives newer ones.
        glyphs.get(&glyph(3));
        glyphs.insert(glyph(5), mesh_of(third), 0);
        assert!(glyphs.get(&glyph(3)).is_some() && glyphs.get(&glyph(4)).is_none());
        // A mesh bigger than the whole budget is not kept, and nothing else is freed for it.
        let before = glyphs.meshes.len();
        glyphs.insert(glyph(6), mesh_of(third * 3), 0);
        assert!(glyphs.get(&glyph(6)).is_none() && glyphs.meshes.len() == before);
        // Glyphs with nothing to keep still count toward the entry limit.
        for index in 100..100 + 2 * MAX_GLYPH_MESHES as u32 {
            glyphs.insert(glyph(index), None, 0);
        }
        assert!(glyphs.meshes.len() <= MAX_GLYPH_MESHES);
        glyphs.use_fonts(glyphs.font_generation + 1);
        assert!(glyphs.meshes.is_empty() && glyphs.bytes == 0);
    }

    #[test]
    fn animating_the_bevel_keeps_the_glyph_cache_within_its_budget() {
        let mut glyphs = GlyphMeshes::default();
        for step in 0..120 {
            let bevel = 0.01 + step as f64 * 0.0001;
            built(bevelled("@&8", bevel), &mut glyphs, 100).unwrap();
            assert!(glyphs.bytes <= GLYPH_BUDGET_BYTES);
        }
        // A build that fails the node's limit leaves the cache within its budget too.
        let letters = every_glyph();
        built(bevelled(&letters, 0.02), &mut glyphs, 100_000)
            .err()
            .unwrap();
        assert!(glyphs.bytes <= GLYPH_BUDGET_BYTES && glyphs.meshes.len() <= MAX_GLYPH_MESHES);
    }

    #[test]
    fn glyph_meshes_are_in_ems_and_survive_a_change_of_size_or_plain_depth() {
        let mut glyphs = GlyphMeshes::default();
        let bounds = |built: Built| {
            let mesh = built.mesh.unwrap();
            Bounds::of(mesh.vertices.iter().map(|v| Vec3::from(v.position)))
        };
        let small = bounds(built(text("Ab"), &mut glyphs, 100).unwrap());
        let kept = glyphs.meshes.len();
        let large = bounds(
            built(
                Text3dParams {
                    font_size: 2.0,
                    ..text("Ab")
                },
                &mut glyphs,
                100,
            )
            .unwrap(),
        );
        let deep = bounds(
            built(
                Text3dParams {
                    depth: 0.75,
                    ..text("Ab")
                },
                &mut glyphs,
                100,
            )
            .unwrap(),
        );
        assert_eq!(glyphs.meshes.len(), kept, "nothing was built again");
        assert!((large.max.x - small.max.x * 2.0).abs() < 1e-5);
        assert!((large.max.y - small.max.y * 2.0).abs() < 1e-5);
        assert!((small.max.z - 0.125).abs() < 1e-6 && (deep.max.z - 0.375).abs() < 1e-6);
        assert!((deep.max.x - small.max.x).abs() < 1e-6);
        // A bevel ties the glyph to the text's size.
        built(bevelled("A", 0.02), &mut glyphs, 100).unwrap();
        let kept = glyphs.meshes.len();
        built(
            Text3dParams {
                font_size: 2.0,
                ..bevelled("A", 0.02)
            },
            &mut glyphs,
            100,
        )
        .unwrap();
        assert_eq!(glyphs.meshes.len(), kept + 1);
    }

    #[test]
    fn a_mesh_with_a_vertex_that_is_not_finite_is_not_built() {
        for depth in [1e40, f64::INFINITY] {
            let error = built(
                Text3dParams { depth, ..text("l") },
                &mut GlyphMeshes::default(),
                100,
            )
            .err()
            .expect("an infinite depth reaches no mesh");
            assert!(error.contains("not finite"), "{error}");
        }
    }
}
