//! How the frame graph records the 2D vector nodes. `render_2d` reads its drawing without
//! recording it, and validation keeps geometry and drawings out of every raster input, so
//! recording one fails.

use bytemuck::{Pod, Zeroable};
use foldhash::HashMap;

use super::build::{DRAWING_SHADER, Keeping, build};
use super::collect::{Source, collect};
use super::text::{check_length, check_weight, shape_text};
use super::tiles::{TILE_SIZE, tile_lists};
use super::{Fill2d, Merge2d, Offset2d, Path2d, Render2d, Shadow2d, Shape2d, Stroke2d, Text2d};
use crate::error::{LumenError, RenderError};
use crate::gpu::frame_graph::{ComputeProgram, FrameCx, Programs, Raster, RecordNode};
use crate::node::{Deferred, JsonNode, Node, required_input};

fn not_a_raster<N: JsonNode>(node_id: crate::node::NodeId, outputs: &str) -> LumenError {
    RenderError::evaluating::<N>(
        node_id,
        format!("this node outputs {outputs}, not a raster"),
    )
    .into()
}

macro_rules! vector_nodes {
    ($($node:ty => $outputs:literal),* $(,)?) => {
        $(
            impl RecordNode for $node {
                fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
                    Ok(())
                }

                fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
                    Err(not_a_raster::<$node>(self.id, $outputs))
                }
            }
        )*
    };
}

vector_nodes! {
    Path2d => "geometry",
    Shape2d => "geometry",
    Offset2d => "geometry",
    Fill2d => "a drawing",
    Stroke2d => "a drawing",
    Shadow2d => "a drawing",
    Merge2d => "a drawing",
}

impl RecordNode for Text2d {
    fn programs(&self, _: &mut Programs<'_>) -> crate::Result<()> {
        Ok(())
    }

    fn record(&self, _: &mut FrameCx<'_>) -> crate::Result<Raster> {
        Err(not_a_raster::<Text2d>(self.id, "geometry"))
    }

    fn literal_param_errors(&self) -> Vec<(&'static str, String)> {
        let params = &self.params;
        let lengths = [
            ("font_size", &params.font_size, false),
            ("max_width", &params.max_width, false),
            ("letter_spacing", &params.letter_spacing, true),
            ("line_height", &params.line_height, false),
        ];
        let mut errors = Vec::new();
        for (param, value, signed) in lengths {
            if let Deferred::Value(value) = value
                && let Err(details) = check_length(param, *value, signed)
            {
                errors.push((param, details));
            }
        }
        if let Deferred::Value(weight) = &params.font_weight
            && let Err(details) = check_weight(*weight)
        {
            errors.push(("font_weight", details));
        }
        errors
    }

    fn literal_font_families(&self) -> Vec<(String, &str)> {
        match &self.params.font_family {
            Deferred::Value(family) => vec![("font_family".to_string(), family.as_str())],
            Deferred::Expr(_) => Vec::new(),
        }
    }
}

const RENDER_2D_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(3, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(4, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(6, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];
const RENDER_2D: ComputeProgram = ComputeProgram {
    label: "render_2d",
    shader: DRAWING_SHADER,
    entry: "cs_main",
    layout: RENDER_2D_LAYOUT,
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable, lumen_macros::ShaderLayout)]
#[shader_layout(wgsl = "DrawingParams", source = DRAWING_SHADER)]
struct DrawingParams {
    /// Canvas pixel of the output's first texel.
    origin: [i32; 2],
    tile_columns: u32,
    _pad: u32,
}

/// A storage buffer holding `items`, or one zeroed item when there are none, as a binding may
/// not be empty.
fn storage<T: Pod>(cx: &mut FrameCx<'_>, items: &[T]) -> lumen_gpu::Res {
    let zero = [T::zeroed()];
    let items = if items.is_empty() { &zero[..] } else { items };
    cx.rec.storage_parts(&[bytemuck::cast_slice(items)])
}

impl RecordNode for Render2d {
    fn programs(&self, programs: &mut Programs<'_>) -> crate::Result<()> {
        programs.compute(&RENDER_2D)
    }

    fn record(&self, cx: &mut FrameCx<'_>) -> crate::Result<Raster> {
        let root = required_input(self.id, "drawing", self.drawing)?;
        let record = collect(cx, self.id, root)?;
        let mut texts = HashMap::default();
        for geometry in &record.geometries {
            if let Source::Text { node, params } = &geometry.source
                && !texts.contains_key(node)
            {
                texts.insert(*node, shape_text(cx, *node, params)?);
            }
        }
        let canvas = cx.canvas_size();
        let canvas = [canvas.width as f32, canvas.height as f32];
        let paths: HashMap<_, _> = record
            .geometries
            .iter()
            .filter_map(|geometry| {
                let node = geometry.source.path_node()?;
                Some((node, cx.node(node)?))
            })
            .collect();
        let (scene, tick, cache, times) = cx.drawing_cache();
        let drawing = build(
            &record,
            &texts,
            &|node| {
                paths
                    .get(&node)
                    .is_some_and(|path| !path.is_frame_dependent())
            },
            &|node| times.time(paths[&node]),
            canvas,
            &mut Keeping {
                cache,
                scene,
                render: self.id,
                tick,
            },
        )?;
        if drawing.drawables.is_empty() {
            return Ok(cx.transparent(cx.canvas_size()));
        }
        let bounds = cx.bounds_of(Some(drawing.bounds()));
        let domain = cx.source_domain(bounds);
        let size = domain.storage_size;
        let origin = [domain.data_rect.x, domain.data_rect.y];
        let (tile_columns, tiles) = tile_lists(&drawing, origin, [size.width, size.height]);
        let tex = cx.storage_target(size);
        let params = cx.rec.uniform(&DrawingParams {
            origin,
            tile_columns,
            _pad: 0,
        });
        let bindings = [
            (0, params),
            (1, storage(cx, &drawing.drawables)),
            (2, storage(cx, &drawing.points)),
            (3, storage(cx, &drawing.bands)),
            (4, storage(cx, &drawing.paints)),
            (5, storage(cx, &tiles)),
            (6, lumen_gpu::Res::Tex(tex)),
        ];
        let program = cx.program(&RENDER_2D)?;
        cx.dispatch(
            program,
            &bindings,
            lumen_gpu::Dispatch {
                x: size.width.div_ceil(TILE_SIZE),
                y: size.height.div_ceil(TILE_SIZE),
                z: 1,
            },
        );
        Ok(Raster {
            tex,
            domain,
            bounds: None,
            reduced: None,
        })
    }
}
