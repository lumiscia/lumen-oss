//! Builds the meshes of `extrude_3d` and `lathe_3d` nodes from the outlines wired into them, and
//! keeps them while their outlines stay the same.

use std::collections::HashSet;

use lumen_3d::glam::{Vec2, Vec3};
use lumen_3d::{
    Bounds, ExtrudeLimits, Extrusion, Lathe, LatheContour, LatheLimits, Mesh, extrude, lathe,
};
use lumen_gpu::Recorder;

use super::collect::GpuMesh;
use super::extrude_3d::{Extrude3dParams, MAX_BEVEL_SEGMENTS};
use super::lathe_3d::{Lathe3dParams, MAX_SEGMENTS, MIN_SEGMENTS};
use super::{Extrude3d, Lathe3d};
use crate::error::{At, RenderError};
use crate::gpu::frame_graph::FrameCx;
use crate::node::vector::drawable::DrawableTransform;
use crate::node::vector::drawing::{Contour, Outline, Source, flatten, resolve_outline};
use crate::node::vector::shape::ShapeGeometryKind;
use crate::node::{JsonNode, NodeId};

/// Greatest distance between a curve and the straight edges it is built from, in world units.
const CURVE_TOLERANCE: f64 = 0.001;
/// Most points an outline may flatten to, before it is held to a node's limits.
const MAX_OUTLINE_POINTS: usize = 16384;
/// Most meshes kept, counting those of outlines with nothing to draw.
const MAX_SOLIDS: usize = 256;
/// What one extrusion may take to build, from its outline to its mesh.
const EXTRUDE_LIMITS: ExtrudeLimits = ExtrudeLimits {
    max_contours: 256,
    max_points: MAX_OUTLINE_POINTS,
    max_bevel_points: 4096,
    max_vertices: 262_144,
    max_indices: 786_432,
};
/// What one lathe may take to build, which is mostly the profile's points times the steps around.
const LATHE_LIMITS: LatheLimits = LatheLimits {
    max_contours: 256,
    max_points: MAX_OUTLINE_POINTS,
    max_vertices: 262_144,
    max_indices: 786_432,
};

/// A solid's mesh on the GPU, with its index count and what it covers before it is placed.
#[derive(Debug, Clone, Copy)]
pub(super) struct SolidMesh {
    pub(super) mesh: GpuMesh,
    pub(super) index_count: u32,
    /// For an extrusion, where its caps' indices start, after those of its walls; a lathe has no
    /// caps, and holds its index count.
    pub(super) caps_start: u32,
    pub(super) bounds: Bounds,
}

/// What an outline is drawn from, as far as its mesh goes.
#[derive(Debug, Clone, PartialEq)]
enum SourceKey {
    Path(String),
    Shape {
        kind: ShapeGeometryKind,
        size: [u32; 2],
        border_radius: u32,
    },
}

/// How an outline becomes a solid.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Shape {
    Extrude {
        depth: u32,
        bevel: u32,
        bevel_segments: u32,
    },
    Lathe {
        segments: u32,
        sweep: u64,
    },
}

/// Everything a solid's mesh is built from. The tolerance curves are flattened to follows from the
/// units per pixel and the outline's own scale.
#[derive(Debug, Clone, PartialEq)]
struct SolidKey {
    source: SourceKey,
    /// The bits of the outline's position, scale, rotation and anchor.
    transform: [u64; 7],
    units_per_pixel: u64,
    shape: Shape,
}

impl SolidKey {
    fn new(outline: &Outline, units_per_pixel: f64, shape: Shape) -> Self {
        let DrawableTransform {
            position,
            scale,
            rotation,
            anchor,
            ..
        } = outline.transform;
        let source = match &outline.source {
            Source::Path { data, .. } => SourceKey::Path(data.clone()),
            Source::Shape {
                kind,
                size,
                border_radius,
            } => SourceKey::Shape {
                kind: *kind,
                size: size.map(f32::to_bits),
                border_radius: border_radius.to_bits(),
            },
        };
        Self {
            source,
            transform: [
                position.0, position.1, scale.0, scale.1, rotation, anchor.0, anchor.1,
            ]
            .map(f64::to_bits),
            units_per_pixel: units_per_pixel.to_bits(),
            shape,
        }
    }
}

struct CachedSolid {
    key: SolidKey,
    /// Names the mesh among those a kept output holds.
    id: String,
    mesh: Option<SolidMesh>,
    /// The mesh as built, for what samples its triangles; empty when it draws nothing.
    cpu: Mesh,
    /// Bytes of the mesh on the GPU and of `cpu`.
    bytes: u64,
    last_used: u64,
}

/// What the latest frames' `extrude_3d` and `lathe_3d` nodes drew.
#[derive(Default)]
pub(crate) struct SolidMeshes {
    solids: Vec<CachedSolid>,
    bytes: u64,
    next_id: u64,
}

impl SolidMeshes {
    pub(super) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(super) fn release(&mut self, rec: &mut Recorder) {
        for solid in self.solids.drain(..) {
            if let Some(mesh) = solid.mesh {
                mesh.mesh.forget(rec);
            }
        }
        self.bytes = 0;
    }

    /// Frees meshes, least recently drawn first, while more than [`MAX_SOLIDS`] or `budget` bytes
    /// are kept, sparing those frame `tick` drew and those `kept` names.
    pub(super) fn trim(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        kept: &HashSet<&String>,
        budget: u64,
    ) {
        while self.solids.len() > MAX_SOLIDS || self.bytes > budget {
            let oldest = self
                .solids
                .iter()
                .enumerate()
                .filter(|(_, solid)| solid.last_used != tick && !kept.contains(&solid.id))
                .min_by_key(|(_, solid)| solid.last_used)
                .map(|(index, _)| index);
            let Some(index) = oldest else {
                break;
            };
            let solid = self.solids.swap_remove(index);
            if let Some(mesh) = solid.mesh {
                mesh.mesh.forget(rec);
            }
            self.bytes -= solid.bytes;
        }
    }

    /// The id of the kept solid whose mesh is `mesh`.
    pub(super) fn id_of(&self, mesh: &GpuMesh) -> Option<&str> {
        self.solids
            .iter()
            .find(|solid| solid.mesh.is_some_and(|kept| kept.mesh == *mesh))
            .map(|solid| solid.id.as_str())
    }

    /// The mesh of the solid kept as `id`, as it was built.
    pub(super) fn cpu_mesh(&self, id: &str) -> Option<&Mesh> {
        self.solids
            .iter()
            .find(|solid| solid.id == id)
            .map(|solid| &solid.cpu)
    }

    /// Adds the solid `mesh` built from `key`, drawn in frame `tick`, and returns where it is.
    fn insert(
        &mut self,
        rec: &mut Recorder,
        key: SolidKey,
        (mesh, caps_start): (Mesh, Option<u32>),
        tick: u64,
    ) -> usize {
        // Held on the GPU and on the CPU.
        let bytes = 2
            * (std::mem::size_of_val(mesh.vertices.as_slice())
                + std::mem::size_of_val(mesh.indices.as_slice())) as u64;
        let drawn = (!mesh.indices.is_empty()).then(|| SolidMesh {
            mesh: GpuMesh::upload(rec, &mesh),
            index_count: mesh.indices.len() as u32,
            caps_start: caps_start.unwrap_or(mesh.indices.len() as u32),
            bounds: Bounds::of(
                mesh.vertices
                    .iter()
                    .map(|vertex| Vec3::from(vertex.position)),
            ),
        });
        let (bytes, cpu) = if drawn.is_some() {
            (bytes, mesh)
        } else {
            (0, Mesh::default())
        };
        self.next_id += 1;
        self.bytes += bytes;
        self.solids.push(CachedSolid {
            key,
            id: format!("solid_3d mesh {}", self.next_id),
            mesh: drawn,
            cpu,
            bytes,
            last_used: tick,
        });
        self.solids.len() - 1
    }
}

fn error<N: JsonNode>(node_id: NodeId, details: impl Into<String>) -> crate::error::LumenError {
    RenderError::evaluating::<N>(node_id, details).into()
}

/// The outline of the geometry node `geometry`, as an `N` that needs one without offsets would
/// find it. `verb` says what `N` does with it.
fn outline_of<N: JsonNode>(
    cx: &FrameCx<'_>,
    node_id: NodeId,
    geometry: NodeId,
    verb: &str,
) -> crate::Result<Outline> {
    let expr = cx.expr();
    let outline = resolve_outline::<N>(&expr, geometry, |at| {
        cx.node(at)
            .ok_or_else(|| RenderError::MissingNode { at: At::node(at) }.into())
    })?;
    if outline.offsets {
        return Err(error::<N>(
            node_id,
            format!(
                "{} can't {verb} an offset_2d outline yet; remove the offset_2d",
                N::KIND
            ),
        ));
    }
    Ok(outline)
}

/// The mesh in `key`, built by `build` unless a frame drew it before, and drawn in this frame.
/// `None` when it draws nothing.
fn mesh_of<N: JsonNode>(
    cx: &mut FrameCx<'_>,
    node_id: NodeId,
    key: SolidKey,
    build: impl FnOnce() -> Result<(Mesh, Option<u32>), String>,
) -> crate::Result<Option<SolidMesh>> {
    let tick = cx.tick();
    let (rec, cache) = cx.scene_cache();
    let solids = &mut cache.solids;
    let index = match solids.solids.iter().position(|solid| solid.key == key) {
        Some(index) => index,
        None => {
            let mesh = build().map_err(|details| error::<N>(node_id, details))?;
            solids.insert(rec, key, mesh, tick)
        }
    };
    let cached = &mut solids.solids[index];
    cached.last_used = tick;
    let (id, mesh) = (cached.id.clone(), cached.mesh);
    cx.drew_model(&id);
    Ok(mesh)
}

/// `contour` in world units, which are `units_per_pixel` to the pixel, with Y flipped.
fn world_points(contour: &Contour, units_per_pixel: f64) -> impl Iterator<Item = Vec2> + '_ {
    contour
        .points
        .iter()
        .map(move |[x, y]| Vec2::new((x * units_per_pixel) as f32, (-y * units_per_pixel) as f32))
}

fn contours(outline: &Outline, units_per_pixel: f64) -> Result<Vec<Contour>, String> {
    flatten(
        outline,
        units_per_pixel,
        CURVE_TOLERANCE,
        MAX_OUTLINE_POINTS,
    )
    .map_err(|details| match outline.source {
        Source::Path { node, .. } => format!("path_2d node {node}: {details}"),
        Source::Shape { .. } => details,
    })
}

/// The mesh of `node_id`'s outline, the geometry node `geometry`, with `params` at this frame,
/// and the scale along Z it is placed with. A plain extrusion is built a unit deep and scaled to
/// its depth, so animating the depth builds nothing again; a bevel ties the mesh to its depth.
pub(super) fn extrude_mesh(
    cx: &mut FrameCx<'_>,
    node_id: NodeId,
    geometry: NodeId,
    params: &Extrude3dParams,
) -> crate::Result<Option<(SolidMesh, f32)>> {
    params
        .check()
        .map_err(|details| error::<Extrude3d>(node_id, details))?;
    let outline = outline_of::<Extrude3d>(cx, node_id, geometry, "extrude")?;
    let units = params.units_per_pixel;
    let mut extrusion = Extrusion {
        depth: params.depth as f32,
        bevel: params.bevel_size as f32,
        bevel_segments: params.bevel_segments.min(MAX_BEVEL_SEGMENTS),
    };
    let bevelled = extrusion.bevel > 0.0 && extrusion.bevel_segments > 0;
    let mut depth_scale = 1.0;
    if !bevelled && extrusion.depth > 0.0 {
        depth_scale = extrusion.depth;
        extrusion = Extrusion {
            depth: 1.0,
            bevel: 0.0,
            bevel_segments: 0,
        };
    }
    let shape = Shape::Extrude {
        depth: extrusion.depth.to_bits(),
        bevel: extrusion.bevel.to_bits(),
        bevel_segments: extrusion.bevel_segments,
    };
    let key = SolidKey::new(&outline, units, shape);
    mesh_of::<Extrude3d>(cx, node_id, key, || {
        let outlines: Vec<Vec<Vec2>> = contours(&outline, units)?
            .iter()
            .map(|contour| world_points(contour, units).collect())
            .collect();
        extrude(&outlines, &extrusion, &EXTRUDE_LIMITS)
            .map(|solid| (solid.mesh, Some(solid.caps_start)))
            .map_err(|limit| format!("the extruded outline has {limit}"))
    })
    .map(|solid| solid.map(|solid| (solid, depth_scale)))
}

/// The mesh of `node_id`'s profile, the geometry node `geometry`, with `params` at this frame.
pub(super) fn lathe_mesh(
    cx: &mut FrameCx<'_>,
    node_id: NodeId,
    geometry: NodeId,
    params: &Lathe3dParams,
) -> crate::Result<Option<SolidMesh>> {
    params
        .check()
        .map_err(|details| error::<Lathe3d>(node_id, details))?;
    let outline = outline_of::<Lathe3d>(cx, node_id, geometry, "spin")?;
    let units = params.units_per_pixel;
    let spin = Lathe {
        segments: params.segments.clamp(MIN_SEGMENTS, MAX_SEGMENTS),
        sweep: params.sweep.clamp(0.0, 360.0).to_radians() as f32,
    };
    let shape = Shape::Lathe {
        segments: spin.segments,
        sweep: f64::from(spin.sweep).to_bits(),
    };
    let key = SolidKey::new(&outline, units, shape);
    mesh_of::<Lathe3d>(cx, node_id, key, || {
        let profile: Vec<LatheContour> = contours(&outline, units)?
            .iter()
            .map(|contour| LatheContour {
                points: world_points(contour, units).collect(),
                closed: contour.closed,
            })
            .collect();
        lathe(&profile, &spin, &LATHE_LIMITS)
            .map(|mesh| (mesh, None))
            .map_err(|limit| {
                format!("the spun profile has {limit}; use fewer `segments` or a simpler profile")
            })
    })
}
