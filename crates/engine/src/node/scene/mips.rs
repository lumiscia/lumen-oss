//! Mip chains for the rasters a scene samples, which `render_3d` filters trilinearly and
//! anisotropically when they are shown smaller than their size.

use std::sync::{Arc, Weak};

use lumen_3d::Bounds;
use lumen_3d::glam::{Mat4, Vec2, Vec3, Vec4};
use lumen_gpu::{
    LayoutEntry, LayoutKind, MipChain, RenderTargetSpec, Res, Size, TargetLoad, TexRef, wgpu,
};

use crate::gpu::frame_graph::{FrameCx, Raster};
use crate::node::NodeId;

const SHADER: &str = include_str!("mips.wgsl");
const LAYOUT: &[LayoutEntry] = &[LayoutEntry::staged(
    0,
    wgpu::ShaderStages::FRAGMENT,
    LayoutKind::Texture,
)];
pub(super) const RASTER_SAMPLER: u64 = 0x3d00_0005;
/// The most texels a pixel's footprint is sampled along its long side, as hardware allows.
const ANISOTROPY: u16 = 16;

/// The bytes of chains the cache keeps beyond those the latest frame used.
const BUDGET_BYTES: u64 = 256 << 20;
/// Frames a chain stays cached without being used.
const IDLE_FRAMES: u64 = 600;
/// A raster shown with no more texels than this per pixel keeps its detail unfiltered.
const MINIFIED_ABOVE: f32 = 1.01;
/// A model's raster with no side longer than this has no chain, since what it loses to aliasing
/// is too little to see.
const MODEL_RASTER_MIN_SIDE: u32 = 64;

/// The sampler `render_3d` reads a raster with when it has a mip chain.
pub(super) fn raster_sampler() -> wgpu::SamplerDescriptor<'static> {
    wgpu::SamplerDescriptor {
        label: Some("lumen 3d mipmapped raster sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Linear,
        min_filter: wgpu::FilterMode::Linear,
        mipmap_filter: wgpu::MipmapFilterMode::Linear,
        anisotropy_clamp: ANISOTROPY,
        ..wgpu::SamplerDescriptor::default()
    }
}

/// What keeps a raster's pixels the same from frame to frame, so that its mips are made once.
#[derive(Debug, Clone)]
pub(crate) enum Stable {
    /// The kept output of a node, until that is dropped.
    Slot {
        graph: usize,
        node: NodeId,
        generation: u64,
    },
    /// An image's texture, for as long as that texture lives.
    Texture(Weak<wgpu::Texture>),
}

impl Stable {
    fn same(&self, other: &Self) -> bool {
        match (self, other) {
            (
                Self::Slot {
                    graph,
                    node,
                    generation,
                },
                Self::Slot {
                    graph: other_graph,
                    node: other_node,
                    generation: other_generation,
                },
            ) => (graph, node, generation) == (other_graph, other_node, other_generation),
            (Self::Texture(texture), Self::Texture(other)) => {
                texture.ptr_eq(other) && texture.strong_count() > 0
            }
            _ => false,
        }
    }
}

struct Entry {
    size: Size,
    format: wgpu::TextureFormat,
    levels: u32,
    chain: MipChain,
    bytes: u64,
    last_used: u64,
    /// What the chain holds the mips of while that stays the same, or `None` for a chain made
    /// anew each frame.
    holds: Option<Stable>,
    /// Filled by the frame being recorded, which may yet be abandoned.
    pending: bool,
}

/// Mip chains, kept for rasters that stay the same and pooled for those that change.
#[derive(Default)]
pub(super) struct MipCache {
    entries: Vec<Entry>,
    bytes: u64,
}

/// The levels of a `size` texture's full mip chain.
fn full_levels(size: Size) -> u32 {
    size.width.max(size.height).max(1).ilog2() + 1
}

impl MipCache {
    /// A chain of `levels` levels for a raster of `size` and `format`, and whether it already
    /// holds that raster's mips. A raster that stays `stable` keeps its chain; otherwise the chain
    /// is the frame's alone.
    fn claim(
        &mut self,
        rec: &mut lumen_gpu::Recorder,
        tick: u64,
        size: Size,
        format: wgpu::TextureFormat,
        levels: u32,
        stable: Option<Stable>,
    ) -> (MipChain, bool) {
        let fits = |entry: &Entry| entry.size == size && entry.format == format;
        if let Some(stable) = &stable
            && let Some(entry) = self.entries.iter_mut().find(|entry| {
                fits(entry)
                    && entry.levels >= levels
                    && entry.holds.as_ref().is_some_and(|held| held.same(stable))
            })
        {
            entry.last_used = tick;
            return (entry.chain.clone(), true);
        }
        let reusable = self
            .entries
            .iter()
            .position(|entry| fits(entry) && entry.levels == levels && entry.last_used != tick);
        let index = reusable.unwrap_or_else(|| {
            let texture = rec.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("lumen 3d raster mips"),
                size: wgpu::Extent3d {
                    width: size.width,
                    height: size.height,
                    depth_or_array_layers: 1,
                },
                mip_level_count: levels,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
                usage: wgpu::TextureUsages::TEXTURE_BINDING
                    | wgpu::TextureUsages::RENDER_ATTACHMENT,
                view_formats: &[],
            });
            let chain = rec.adopt_mips(Arc::new(texture));
            let bytes = (0..levels)
                .map(|level| {
                    format.theoretical_memory_footprint(wgpu::Extent3d {
                        width: (size.width >> level).max(1),
                        height: (size.height >> level).max(1),
                        depth_or_array_layers: 1,
                    })
                })
                .sum();
            self.bytes += bytes;
            self.entries.push(Entry {
                size,
                format,
                levels,
                chain,
                bytes,
                last_used: tick,
                holds: None,
                pending: false,
            });
            self.entries.len() - 1
        });
        let entry = &mut self.entries[index];
        entry.last_used = tick;
        entry.pending = stable.is_some();
        entry.holds = stable;
        (entry.chain.clone(), false)
    }

    /// Forgets what a frame that was dropped before it ran was to fill.
    pub(super) fn discard_pending(&mut self) {
        for entry in &mut self.entries {
            if std::mem::take(&mut entry.pending) {
                entry.holds = None;
            }
        }
    }

    /// Keeps what the frame that ran filled.
    pub(super) fn commit(&mut self) {
        for entry in &mut self.entries {
            entry.pending = false;
        }
    }

    /// Frees chains not used in frame `tick` that have idled, and the least recently used of the
    /// rest while the cache holds more than [`BUDGET_BYTES`].
    pub(super) fn trim(&mut self, rec: &mut lumen_gpu::Recorder, tick: u64) {
        let idle_before = tick.saturating_sub(IDLE_FRAMES);
        let mut order: Vec<usize> = (0..self.entries.len())
            .filter(|&index| self.entries[index].last_used != tick)
            .collect();
        order.sort_unstable_by_key(|&index| self.entries[index].last_used);
        let mut freed = Vec::new();
        for index in order {
            let entry = &self.entries[index];
            if entry.last_used > idle_before && self.bytes <= BUDGET_BYTES {
                break;
            }
            self.bytes -= entry.bytes;
            rec.forget_mips(&entry.chain);
            freed.push(index);
        }
        freed.sort_unstable();
        for index in freed.into_iter().rev() {
            self.entries.swap_remove(index);
        }
    }

    pub(super) fn release(&mut self, rec: &mut lumen_gpu::Recorder) {
        for entry in self.entries.drain(..) {
            rec.forget_mips(&entry.chain);
        }
        self.bytes = 0;
    }
}

/// The texture holding `raster`, the output of `node`, with its first `levels` mip levels, for
/// shading to sample trilinearly. Fills the levels in passes of this frame unless a chain kept
/// from earlier frames holds them already.
pub(super) fn mipped(
    cx: &mut FrameCx<'_>,
    node: NodeId,
    raster: Raster,
    levels: u32,
) -> crate::Result<TexRef> {
    let size = raster.domain.storage_size;
    let format = cx.rec.format(raster.tex);
    let levels = levels.clamp(1, full_levels(size));
    let stable = cx.stable_raster(node, raster);
    let tick = cx.tick();
    let (rec, cache) = cx.scene_cache();
    let (chain, filled) = cache.mips.claim(rec, tick, size, format, levels, stable);
    if !filled {
        fill(cx.rec, raster.tex, &chain, levels).map_err(crate::gpu::frame_graph::gpu_error)?;
    }
    Ok(chain.sampled)
}

/// Records the passes that fill the first `levels` levels of `chain` from the raster `source`:
/// the first a copy of it, and each of the others the area-weighted average of the one above.
fn fill(
    rec: &mut lumen_gpu::Recorder,
    source: TexRef,
    chain: &MipChain,
    levels: u32,
) -> anyhow::Result<()> {
    let format = rec.format(source);
    let mut from = source;
    for (level, &target) in chain.levels.iter().take(levels as usize).enumerate() {
        let fragment = if level == 0 { "fs_copy" } else { "fs_halve" };
        let program = rec.render_program(
            "scene-3d-mips",
            SHADER,
            "vs_main",
            fragment,
            LAYOUT,
            RenderTargetSpec::flat(format, wgpu::BlendState::REPLACE),
            0,
        )?;
        rec.draw(
            program,
            &[(0, Res::Tex(from))],
            target,
            TargetLoad::Clear,
            3,
            1,
            None,
        );
        from = target;
    }
    Ok(())
}

/// How many mip levels a raster of `size` shown at up to `texels_per_pixel` needs.
pub(super) fn levels_for(size: Size, texels_per_pixel: f32) -> u32 {
    let needed = if texels_per_pixel.is_finite() {
        texels_per_pixel.max(1.0).log2().floor() as u32 + 2
    } else {
        u32::MAX
    };
    needed.clamp(1, full_levels(size))
}

/// Whether a raster of `size` that a model shows on a surface of unknown shape has a chain.
pub(super) fn model_raster_is_mipped(size: Size) -> bool {
    size.width.max(size.height) > MODEL_RASTER_MIN_SIDE
}

/// Whether showing `texels_per_pixel` of a raster in a pixel loses detail without mipmaps.
pub(super) fn is_minified(texels_per_pixel: f32) -> bool {
    texels_per_pixel > MINIFIED_ABOVE
}

/// The most texels of a plane's raster, `size` of them across the unit square in the XY plane that
/// `world` places, that a pixel of a `target`-sized render covers under `view_proj`, at the
/// square's corners and centre; infinite where it comes up to the camera, and `None` where it is
/// off screen.
pub(super) fn plane_texels_per_pixel(
    world: &Mat4,
    view_proj: &Mat4,
    size: Size,
    target: Size,
) -> Option<f32> {
    let to_clip = *view_proj * *world;
    let points = [
        (-0.5, -0.5),
        (0.5, -0.5),
        (-0.5, 0.5),
        (0.5, 0.5),
        (0.0, 0.0),
    ];
    let clip = points.map(|(x, y)| to_clip * Vec4::new(x, y, 0.0, 1.0));
    let outside = |inside: fn(Vec4) -> bool| clip.iter().all(|&point| !inside(point));
    if outside(|c| c.x >= -c.w)
        || outside(|c| c.x <= c.w)
        || outside(|c| c.y >= -c.w)
        || outside(|c| c.y <= c.w)
        || outside(|c| c.z >= 0.0 && c.z <= c.w)
    {
        return None;
    }
    if clip.iter().any(|c| c.w <= 1e-6) {
        return Some(f32::INFINITY);
    }
    let half = Vec2::new(target.width as f32, target.height as f32) * 0.5;
    // Where a step of a texel along `axis` moves the point in the target's pixels.
    let step = |c: Vec4, axis: Vec4| {
        let d = to_clip * axis;
        (Vec2::new(d.x, d.y) * c.w - Vec2::new(c.x, c.y) * d.w) / (c.w * c.w) * half
    };
    let (du, dv) = (
        Vec4::new(1.0 / size.width.max(1) as f32, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 1.0 / size.height.max(1) as f32, 0.0, 0.0),
    );
    let mut worst = 0.0f32;
    for c in clip {
        let (u, v) = (step(c, du), step(c, dv));
        // The smaller singular value of the step's matrix is how far a texel moves along the
        // direction it moves least.
        let sum = u.length_squared() + v.length_squared();
        let det = u.x * v.y - u.y * v.x;
        let spread = (sum * sum - 4.0 * det * det).max(0.0).sqrt();
        let least = ((sum - spread) * 0.5).max(0.0).sqrt();
        worst = worst.max(if least > 0.0 {
            1.0 / least
        } else {
            f32::INFINITY
        });
    }
    Some(worst)
}

/// Whether any of `bounds` is in view under `view_proj`. An empty box, whose place is unknown, is.
pub(super) fn bounds_in_view(bounds: &Bounds, view_proj: &Mat4) -> bool {
    if bounds.is_empty() {
        return true;
    }
    let clip: Vec<Vec4> = (0..8)
        .map(|corner| {
            let pick =
                |bit: u32, low: f32, high: f32| if corner >> bit & 1 == 0 { low } else { high };
            let point = Vec3::new(
                pick(0, bounds.min.x, bounds.max.x),
                pick(1, bounds.min.y, bounds.max.y),
                pick(2, bounds.min.z, bounds.max.z),
            );
            *view_proj * point.extend(1.0)
        })
        .collect();
    let outside = |inside: fn(Vec4) -> bool| clip.iter().all(|&point| !inside(point));
    !(outside(|c| c.x >= -c.w)
        || outside(|c| c.x <= c.w)
        || outside(|c| c.y >= -c.w)
        || outside(|c| c.y <= c.w)
        || outside(|c| c.z >= 0.0 && c.z <= c.w))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A camera `distance` along +Z from a plane at the origin, whose unit square is `width`
    /// across, over a 100 x 100 render with a 90 degree field of view.
    fn texels_per_pixel(width: f32, tilt: f32, distance: f32, texels: u32) -> Option<f32> {
        let world = Mat4::from_rotation_x(tilt) * Mat4::from_scale(Vec3::splat(width));
        let view = Mat4::from_translation(Vec3::new(0.0, 0.0, -distance));
        let projection = Mat4::perspective_rh(std::f32::consts::FRAC_PI_2, 1.0, 0.1, 100.0);
        plane_texels_per_pixel(
            &world,
            &(projection * view),
            Size::new(texels, texels),
            Size::new(100, 100),
        )
    }

    #[test]
    fn a_plane_covering_its_texels_is_not_minified_and_a_smaller_one_is() {
        // At distance 10 a 90 degree view is 20 units across 100 pixels: 5 pixels a unit.
        let facing = |width: f32, texels: u32| texels_per_pixel(width, 0.0, 10.0, texels).unwrap();
        assert!(!is_minified(facing(20.0, 100)));
        assert!(!is_minified(facing(20.0, 50)));
        assert!((facing(20.0, 200) - 2.0).abs() < 0.01);
        assert!(is_minified(facing(4.0, 100)));
        // Tilted back 60 degrees, its far half is squeezed to a fifth of the way it was.
        let tilted = texels_per_pixel(20.0, 1.047, 10.0, 100).unwrap();
        assert!(is_minified(tilted), "{tilted}");
    }

    #[test]
    fn a_plane_behind_the_camera_needs_no_mips() {
        assert_eq!(texels_per_pixel(1.0, 0.0, -10.0, 100), None);
    }

    #[test]
    fn levels_cover_the_footprint_and_one_more() {
        let size = Size::new(1024, 512);
        assert_eq!(levels_for(size, 1.0), 2);
        assert_eq!(levels_for(size, 3.9), 3);
        assert_eq!(levels_for(size, 4.0), 4);
        assert_eq!(levels_for(size, 1e9), 11);
        assert_eq!(levels_for(Size::new(1, 1), 8.0), 1);
        assert_eq!(levels_for(Size::new(256, 256), f32::INFINITY), 9);
    }

    /// A texture's levels, `levels` of them, as the GPU fills them from `texels`, premultiplied
    /// and encoded as rasters are stored, `size` across.
    fn gpu_levels(texels: &[u8], size: Size, levels: u32) -> Vec<Vec<u8>> {
        let gpu = pollster::block_on(lumen_gpu::Renderer::new()).expect("mip tests need a GPU");
        let (device, queue) = (gpu.device.clone(), gpu.queue.clone());
        let make = |size: Size, mips: u32, usage| {
            Arc::new(device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: size.as_extent(),
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba8Unorm,
                usage,
                view_formats: &[],
            }))
        };
        let source = make(
            size,
            1,
            wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        );
        queue.write_texture(
            source.as_image_copy(),
            texels,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size.width * 4),
                rows_per_image: Some(size.height),
            },
            size.as_extent(),
        );
        let chain_texture = make(
            size,
            levels,
            wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::COPY_SRC,
        );
        let mut rec = lumen_gpu::Recorder::new(device.clone(), queue.clone());
        rec.begin_frame();
        let source = rec.adopt(source);
        let chain = rec.adopt_mips(Arc::clone(&chain_texture));
        fill(&mut rec, source, &chain, levels).expect("recorded");
        rec.resolve(chain.sampled, None).expect("resolved");
        rec.upload().expect("uploaded");
        rec.submit().expect("submitted");
        (0..levels)
            .map(|level| {
                let side = Size::new((size.width >> level).max(1), (size.height >> level).max(1));
                let copy = make(
                    side,
                    1,
                    wgpu::TextureUsages::COPY_DST | wgpu::TextureUsages::COPY_SRC,
                );
                let mut encoder = device.create_command_encoder(&Default::default());
                let mut from = chain_texture.as_image_copy();
                from.mip_level = level;
                encoder.copy_texture_to_texture(from, copy.as_image_copy(), side.as_extent());
                queue.submit([encoder.finish()]);
                gpu.read_texture_rgba8(&copy, side).expect("read")
            })
            .collect()
    }

    fn decode(value: u8) -> f64 {
        let c = f64::from(value) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    }

    fn encode(light: f64) -> f64 {
        let c = light.clamp(0.0, 1.0);
        let c = if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        c * 255.0
    }

    /// The level below `texels`, `from` across, by averaging the premultiplied light of every
    /// texel under each of its own, each weighted by the area it shares with it exactly.
    fn reference(texels: &[u8], from: (usize, usize)) -> Vec<u8> {
        let to = ((from.0 / 2).max(1), (from.1 / 2).max(1));
        // Texel `i` of `to` spans `[i * from, (i + 1) * from)` in units of `1 / (from * to)`, and
        // source texel `j` spans `[j * to, (j + 1) * to)`.
        let overlap = |i: usize, j: usize, from: usize, to: usize| {
            let low = (i * from).max(j * to);
            let high = ((i + 1) * from).min((j + 1) * to);
            high.saturating_sub(low) as f64
        };
        let mut out = Vec::new();
        for y in 0..to.1 {
            for x in 0..to.0 {
                let (mut light, mut alpha, mut total) = ([0.0; 3], 0.0, 0.0);
                for sy in 0..from.1 {
                    for sx in 0..from.0 {
                        let weight = overlap(x, sx, from.0, to.0) * overlap(y, sy, from.1, to.1);
                        if weight == 0.0 {
                            continue;
                        }
                        let texel = &texels[(sy * from.0 + sx) * 4..][..4];
                        let a = f64::from(texel[3]) / 255.0;
                        if a > 0.0 {
                            for (sum, &value) in light.iter_mut().zip(texel) {
                                // Premultiplied linear light of the straight colour.
                                *sum += decode((f64::from(value) / a).round().min(255.0) as u8)
                                    * a
                                    * weight;
                            }
                        }
                        alpha += a * weight;
                        total += weight;
                    }
                }
                for sum in light {
                    let straight = if alpha > 0.0 { sum / alpha } else { 0.0 };
                    out.push((encode(straight) * alpha / total).round() as u8);
                }
                out.push((alpha / total * 255.0).round() as u8);
            }
        }
        out
    }

    /// Texels of premultiplied colour: opaque ones, translucent ones, and clear ones.
    fn varied(width: usize, height: usize) -> Vec<u8> {
        let mut seed = 0x2545_f491u32;
        let mut next = move || {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (seed >> 24) as u8
        };
        (0..width * height)
            .flat_map(|_| {
                let alpha = match next() % 4 {
                    0 => 0,
                    1 => 255,
                    _ => next(),
                };
                let mut texel = [0u8; 4];
                for value in &mut texel[..3] {
                    *value = (u32::from(next()) * u32::from(alpha) / 255) as u8;
                }
                texel[3] = alpha;
                texel
            })
            .collect()
    }

    fn assert_levels_follow_the_reference(texels: &[u8], width: u32, height: u32, tolerance: u8) {
        let levels = Size::new(width, height).width.max(height).ilog2() + 1;
        let gpu = gpu_levels(texels, Size::new(width, height), levels);
        assert_eq!(gpu[0], texels, "the first level is the raster");
        for level in 1..levels as usize {
            let from = (
                (width as usize >> (level - 1)).max(1),
                (height as usize >> (level - 1)).max(1),
            );
            let expected = reference(&gpu[level - 1], from);
            assert_eq!(gpu[level].len(), expected.len());
            let worst = gpu[level]
                .iter()
                .zip(&expected)
                .map(|(got, want)| got.abs_diff(*want))
                .max()
                .unwrap();
            assert!(
                worst <= tolerance,
                "{width}x{height} level {level} is off by {worst}: {:?} against {expected:?}",
                gpu[level]
            );
        }
    }

    #[test]
    fn a_level_is_the_linear_light_average_of_the_texels_it_covers() {
        // Black and white average to the grey that is half the light, not half the code.
        let checker: Vec<u8> = (0..16)
            .flat_map(|i| {
                let value = if (i % 4 + i / 4) % 2 == 0 { 255 } else { 0 };
                [value, value, value, 255]
            })
            .collect();
        let levels = gpu_levels(&checker, Size::new(4, 4), 3);
        assert!(
            levels[2]
                .chunks(4)
                .all(|t| t[0].abs_diff(188) <= 1 && t[3] == 255)
        );
        assert_levels_follow_the_reference(&checker, 4, 4, 1);
        assert_levels_follow_the_reference(&varied(8, 8), 8, 8, 2);
    }

    #[test]
    fn an_odd_side_averages_all_its_texels() {
        // White, black, white: two thirds of the light, not black.
        let row = [255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255];
        let across = gpu_levels(&row, Size::new(3, 1), 2);
        assert!(across[1][..3].iter().all(|&c| c.abs_diff(213) <= 1));
        assert_levels_follow_the_reference(&row, 3, 1, 1);
        // The same down a column.
        assert_levels_follow_the_reference(&row, 1, 3, 1);
        let column = gpu_levels(&row, Size::new(1, 3), 2);
        assert!(column[1][..3].iter().all(|&c| c.abs_diff(213) <= 1));
        for (width, height) in [(7, 5), (5, 9), (13, 3), (15, 15)] {
            assert_levels_follow_the_reference(
                &varied(width, height),
                width as u32,
                height as u32,
                2,
            );
        }
    }
}
