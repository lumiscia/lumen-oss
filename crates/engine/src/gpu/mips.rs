//! Mip chains for rasters that nodes sample when they show them smaller than their size: kept for
//! rasters that stay the same from frame to frame, and made anew each frame for those that change.

use std::sync::Arc;

use lumen_gpu::{
    LayoutEntry, LayoutKind, MipChain, RenderTargetSpec, Res, Size, TargetLoad, TexRef, wgpu,
};

use crate::gpu::frame_graph::{FrameCx, Raster};
use crate::gpu::params::CropParams;
use crate::node::NodeId;

const SHADER: &str = include_str!("shaders/mips.wgsl");
const COPY_LAYOUT: &[LayoutEntry] = &[
    LayoutEntry::staged(0, wgpu::ShaderStages::FRAGMENT, LayoutKind::Texture),
    LayoutEntry::staged(1, wgpu::ShaderStages::FRAGMENT, LayoutKind::Uniform),
];
const HALVE_LAYOUT: &[LayoutEntry] = &[LayoutEntry::staged(
    0,
    wgpu::ShaderStages::FRAGMENT,
    LayoutKind::Texture,
)];

/// The programs that fill chains of working-space rasters, built ahead of the frames that use them.
pub(crate) fn programs(programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
    for (fragment, layout) in [("fs_copy", COPY_LAYOUT), ("fs_halve", HALVE_LAYOUT)] {
        programs.render(
            "mips",
            SHADER,
            "vs_main",
            fragment,
            layout,
            target_spec(super::color::WORKING_COLOR_FORMAT),
            0,
        )?;
    }
    Ok(())
}

fn target_spec(format: wgpu::TextureFormat) -> RenderTargetSpec {
    RenderTargetSpec::flat(format, wgpu::BlendState::REPLACE)
}

/// Frames a chain stays cached without being used.
const IDLE_FRAMES: u64 = 600;

/// What keeps a raster's pixels the same from frame to frame, so that its mips are made once.
#[derive(Debug, Clone)]
pub(crate) enum Stable {
    /// The kept output of a node, until that is dropped.
    Slot {
        graph: usize,
        node: NodeId,
        generation: u64,
    },
    /// An image, by the identity and size of its pixels.
    Image(crate::gpu::MediaTextureKey),
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
            (Self::Image(image), Self::Image(other)) => image == other,
            _ => false,
        }
    }
}

struct Entry {
    size: Size,
    /// Where in its raster the chain's first level starts.
    origin: [i32; 2],
    format: wgpu::TextureFormat,
    levels: u32,
    chain: MipChain,
    bytes: u64,
    last_used: u64,
    /// What the chain holds the mips of while that stays the same, or `None` for a chain made
    /// anew each frame.
    holds: Option<Stable>,
    /// The raster this frame filled the chain from, which another reader of it this frame shares.
    from: Option<TexRef>,
    /// Filled by the frame being recorded, which may yet be abandoned.
    pending: bool,
}

/// Mip chains, kept for rasters that stay the same and pooled for those that change.
#[derive(Default)]
pub(crate) struct MipCache {
    entries: Vec<Entry>,
    bytes: u64,
}

/// The levels of a `size` texture's full mip chain.
pub(crate) fn full_levels(size: Size) -> u32 {
    size.width.max(size.height).max(1).ilog2() + 1
}

impl MipCache {
    /// A chain of `levels` levels for the `size` part of the raster `source`, from `origin`, of
    /// `format`, and whether it already holds that part's mips. A raster that stays `stable` keeps
    /// its chain; otherwise the chain is the frame's, shared by whatever else reads `source`
    /// in it.
    #[allow(clippy::too_many_arguments, reason = "what identifies a chain")]
    fn claim(
        &mut self,
        rec: &mut lumen_gpu::Recorder,
        tick: u64,
        source: TexRef,
        origin: [i32; 2],
        size: Size,
        format: wgpu::TextureFormat,
        levels: u32,
        stable: Option<Stable>,
    ) -> (MipChain, bool) {
        let fits =
            |entry: &Entry| entry.size == size && entry.origin == origin && entry.format == format;
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            fits(entry)
                && entry.levels >= levels
                && ((entry.last_used == tick && entry.from == Some(source))
                    || stable.as_ref().is_some_and(|stable| {
                        entry.holds.as_ref().is_some_and(|held| held.same(stable))
                    }))
        }) {
            entry.last_used = tick;
            return (entry.chain.clone(), true);
        }
        let reusable = self.entries.iter().position(|entry| {
            entry.size == size
                && entry.format == format
                && entry.levels == levels
                && entry.last_used != tick
        });
        let index = reusable.unwrap_or_else(|| {
            let texture = rec.device().create_texture(&wgpu::TextureDescriptor {
                label: Some("lumen raster mips"),
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
                origin,
                format,
                levels,
                chain,
                bytes,
                last_used: tick,
                holds: None,
                from: None,
                pending: false,
            });
            self.entries.len() - 1
        });
        let entry = &mut self.entries[index];
        entry.last_used = tick;
        entry.origin = origin;
        entry.from = Some(source);
        entry.pending = stable.is_some();
        entry.holds = stable;
        (entry.chain.clone(), false)
    }

    /// Forgets what a frame that was dropped before it ran was to fill.
    pub(crate) fn discard_pending(&mut self) {
        for entry in &mut self.entries {
            if std::mem::take(&mut entry.pending) {
                entry.holds = None;
            }
        }
    }

    /// Keeps what the frame that ran filled.
    pub(crate) fn commit(&mut self) {
        for entry in &mut self.entries {
            entry.pending = false;
        }
    }

    /// Frees chains not used in frame `tick` that have idled, and the least recently used of the
    /// rest while the cache holds more than `budget` bytes.
    pub(crate) fn trim(&mut self, rec: &mut lumen_gpu::Recorder, tick: u64, budget: u64) {
        let idle_before = tick.saturating_sub(IDLE_FRAMES);
        let mut order: Vec<usize> = (0..self.entries.len())
            .filter(|&index| self.entries[index].last_used != tick)
            .collect();
        order.sort_unstable_by_key(|&index| self.entries[index].last_used);
        let mut freed = Vec::new();
        for index in order {
            let entry = &self.entries[index];
            if entry.last_used > idle_before && self.bytes <= budget {
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

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn release(&mut self, rec: &mut lumen_gpu::Recorder) {
        for entry in self.entries.drain(..) {
            rec.forget_mips(&entry.chain);
        }
        self.bytes = 0;
    }
}

/// The texture holding `raster`, the output of `node`, or its `region` alone, with its first
/// `levels` mip levels. Fills the levels in passes of this frame unless a chain kept from earlier
/// frames, or one an earlier reader of `raster` made in this one, holds them already.
pub(crate) fn mipped(
    cx: &mut FrameCx<'_>,
    node: NodeId,
    raster: Raster,
    region: Option<CropParams>,
    levels: u32,
) -> crate::Result<TexRef> {
    let (origin, size) = match region {
        Some(region) => (region.origin, Size::new(region.size[0], region.size[1])),
        None => ([0, 0], raster.domain.storage_size),
    };
    let format = cx.rec.format(raster.tex);
    let levels = levels.clamp(1, full_levels(size));
    let stable = cx.stable_raster(node, raster);
    let tick = cx.tick();
    let (rec, cache) = cx.mip_cache();
    let (chain, filled) = cache.claim(rec, tick, raster.tex, origin, size, format, levels, stable);
    if !filled {
        fill(cx.rec, raster.tex, origin, &chain, levels)
            .map_err(crate::gpu::frame_graph::gpu_error)?;
    }
    Ok(chain.sampled)
}

/// Records the passes that fill the first `levels` levels of `chain` from the raster `source`: the
/// first a copy of it from `origin`, and each of the others the area-weighted average of the one
/// above.
fn fill(
    rec: &mut lumen_gpu::Recorder,
    source: TexRef,
    origin: [i32; 2],
    chain: &MipChain,
    levels: u32,
) -> anyhow::Result<()> {
    let format = rec.format(source);
    let mut from = source;
    for (level, &target) in chain.levels.iter().take(levels as usize).enumerate() {
        let (fragment, layout) = if level == 0 {
            ("fs_copy", COPY_LAYOUT)
        } else {
            ("fs_halve", HALVE_LAYOUT)
        };
        let program = rec.render_program(
            "mips",
            SHADER,
            "vs_main",
            fragment,
            layout,
            target_spec(format),
            0,
        )?;
        let bindings = if level == 0 {
            let region = rec.uniform(&[origin[0], origin[1], 0, 0]);
            vec![(0, Res::Tex(from)), (1, region)]
        } else {
            vec![(0, Res::Tex(from))]
        };
        rec.draw(program, &bindings, target, TargetLoad::Clear, 3, 1, None);
        from = target;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A texture's levels, `levels` of them, as the GPU fills them from `texels`, `size` across.
    /// The textures are `Rgba8UnormSrgb`, so the shader reads and writes premultiplied linear
    /// light, as rasters hold it, and each byte is that light encoded.
    fn gpu_levels(texels: &[u8], size: Size, levels: u32) -> Vec<Vec<u8>> {
        gpu_levels_as(wgpu::TextureFormat::Rgba8UnormSrgb, texels, size, levels)
    }

    /// `gpu_levels` for textures of `format`, whose texels `texels` holds as their bytes.
    fn gpu_levels_as(
        format: wgpu::TextureFormat,
        texels: &[u8],
        size: Size,
        levels: u32,
    ) -> Vec<Vec<u8>> {
        let texel_bytes = format.block_copy_size(None).expect("a colour format");
        let gpu = pollster::block_on(lumen_gpu::Renderer::new()).expect("mip tests need a GPU");
        let (device, queue) = (gpu.device.clone(), gpu.queue.clone());
        let make = |size: Size, mips: u32, usage| {
            Arc::new(device.create_texture(&wgpu::TextureDescriptor {
                label: None,
                size: size.as_extent(),
                mip_level_count: mips,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format,
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
                bytes_per_row: Some(size.width * texel_bytes),
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
        fill(&mut rec, source, [0, 0], &chain, levels).expect("recorded");
        rec.resolve(chain.sampled, None).expect("resolved");
        rec.upload().expect("uploaded");
        rec.submit().expect("submitted");
        (0..levels)
            .map(|level| {
                let side = Size::new((size.width >> level).max(1), (size.height >> level).max(1));
                let row = (side.width * texel_bytes) as usize;
                let padded = row.next_multiple_of(wgpu::COPY_BYTES_PER_ROW_ALIGNMENT as usize);
                let buffer = device.create_buffer(&wgpu::BufferDescriptor {
                    label: None,
                    size: (padded * side.height as usize) as u64,
                    usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
                    mapped_at_creation: false,
                });
                let mut encoder = device.create_command_encoder(&Default::default());
                let mut from = chain_texture.as_image_copy();
                from.mip_level = level;
                encoder.copy_texture_to_buffer(
                    from,
                    wgpu::TexelCopyBufferInfo {
                        buffer: &buffer,
                        layout: wgpu::TexelCopyBufferLayout {
                            offset: 0,
                            bytes_per_row: Some(padded as u32),
                            rows_per_image: None,
                        },
                    },
                    side.as_extent(),
                );
                queue.submit([encoder.finish()]);
                buffer.slice(..).map_async(wgpu::MapMode::Read, |_| {});
                device
                    .poll(wgpu::PollType::wait_indefinitely())
                    .expect("read");
                let bytes = buffer
                    .slice(..)
                    .get_mapped_range()
                    .expect("mapped")
                    .to_vec();
                bytes
                    .chunks(padded)
                    .flat_map(|line| line[..row].to_vec())
                    .collect()
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
                        for (sum, &value) in light.iter_mut().zip(texel) {
                            *sum += decode(value) * weight;
                        }
                        alpha += f64::from(texel[3]) / 255.0 * weight;
                        total += weight;
                    }
                }
                for sum in light {
                    out.push(encode(sum / total).round() as u8);
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

    #[test]
    fn a_working_space_level_keeps_light_above_one() {
        // Rasters are `Rgba16Float`, linear light that may pass 1: the level averages it as is.
        let half = |value: f32| -> [u8; 2] {
            let bits: u16 = match value {
                0.0 => 0,
                0.5 => 0x3800,
                1.0 => 0x3c00,
                2.0 => 0x4000,
                8.0 => 0x4800,
                _ => unreachable!("only exact halves"),
            };
            bits.to_le_bytes()
        };
        let texels: Vec<u8> = [
            [8.0, 8.0, 8.0, 1.0],
            [0.0, 0.0, 0.0, 1.0],
            [2.0, 2.0, 2.0, 0.5],
            [0.0, 0.0, 0.0, 0.0],
        ]
        .iter()
        .flatten()
        .flat_map(|&value| half(value))
        .collect();
        let levels = gpu_levels_as(
            wgpu::TextureFormat::Rgba16Float,
            &texels,
            Size::new(2, 2),
            2,
        );
        assert_eq!(levels[0], texels);
        // (2.5, 2.5, 2.5, 0.625) as halves.
        let mean: Vec<u8> = [0x4100u16, 0x4100, 0x4100, 0x3900]
            .iter()
            .flat_map(|bits| bits.to_le_bytes())
            .collect();
        assert_eq!(levels[1], mean);
    }
}
