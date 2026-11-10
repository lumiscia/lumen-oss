//! Environment maps on the GPU: each source's prefiltered reflections and irradiance, made once,
//! and the map at its decoded size once a frame shows it as a background.

use std::collections::HashSet;
use std::sync::Arc;

use foldhash::HashMap;
use lumen_3d::environment::{self, Equirect};
use lumen_gpu::{BufRef, Recorder, TexRef, wgpu};

use super::collect::{evictions, storage_buffer};
use crate::error::{LumenError, MediaError};
use crate::gpu::frame_graph::FrameCx;
use crate::media::EnvironmentResolver;

/// The bytes of environment maps the cache keeps on the GPU beyond those the latest frame drew or
/// a kept output drew.
const ENVIRONMENT_BUDGET_BYTES: u64 = 256 << 20;

/// A map ready to light a scene.
#[derive(Clone, Copy)]
pub(crate) struct LoadedEnvironment {
    /// The reflection levels as the mip levels of one texture.
    pub(crate) reflections: TexRef,
    pub(crate) levels: u32,
    /// Irradiance over π as nine spherical-harmonic coefficients, each padded to four floats.
    pub(crate) irradiance: [[f32; 4]; 9],
    /// The map at its decoded size, once a background has needed it.
    pub(crate) background: Option<TexRef>,
}

struct Cached {
    environment: LoadedEnvironment,
    bytes: u64,
    last_used: u64,
}

/// Environment maps by their resolver's ID, and the split-sum table they are all lit with.
#[derive(Default)]
pub(crate) struct EnvironmentCache {
    maps: HashMap<String, Cached>,
    bytes: u64,
    brdf: Option<BufRef>,
}

enum LoadError {
    Media(MediaError),
    /// Why the file isn't a map that can be drawn.
    Map(String),
}

impl From<MediaError> for LoadError {
    fn from(error: MediaError) -> Self {
        Self::Media(error)
    }
}

impl EnvironmentCache {
    /// The map `resolver` serves, with its background if `background`, marked drawn in `tick`.
    /// Its file is read again only to add a background it was first loaded without.
    fn load(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        resolver: &dyn EnvironmentResolver,
        background: bool,
    ) -> Result<LoadedEnvironment, LoadError> {
        if let Some(cached) = self.maps.get_mut(resolver.id())
            && (!background || cached.environment.background.is_some())
        {
            cached.last_used = tick;
            return Ok(cached.environment);
        }
        let bytes = resolver.data()?;
        let map = environment::decode(&bytes).map_err(|error| LoadError::Map(error.to_string()))?;
        drop(bytes);
        let max = rec.device().limits().max_texture_dimension_2d;
        if map.width().max(map.height()) > max {
            return Err(LoadError::Map(format!(
                "the map is larger than this device's {max}-pixel texture limit"
            )));
        }
        let mut cached = match self.maps.remove(resolver.id()) {
            Some(cached) => {
                self.bytes -= cached.bytes;
                cached
            }
            None => {
                let prefiltered = environment::prefilter(&map);
                let levels: Vec<&Equirect> = prefiltered.levels.iter().collect();
                Cached {
                    environment: LoadedEnvironment {
                        reflections: upload(rec, "lumen 3d environment reflections", &levels),
                        levels: levels.len() as u32,
                        irradiance: prefiltered.irradiance.map(|c| c.extend(0.0).into()),
                        background: None,
                    },
                    bytes: levels.iter().map(|level| texel_bytes(level)).sum(),
                    last_used: tick,
                }
            }
        };
        if background {
            cached.environment.background =
                Some(upload(rec, "lumen 3d environment background", &[&map]));
            cached.bytes += texel_bytes(&map);
        }
        cached.last_used = tick;
        self.bytes += cached.bytes;
        let environment = cached.environment;
        self.maps.insert(resolver.id().to_string(), cached);
        Ok(environment)
    }

    /// The split-sum table of `lumen_3d::environment::brdf_table`, made once.
    pub(super) fn brdf(&mut self, rec: &mut Recorder) -> BufRef {
        *self.brdf.get_or_insert_with(|| {
            let table = environment::brdf_table();
            storage_buffer(
                rec,
                "lumen 3d environment brdf",
                bytemuck::cast_slice(&table),
            )
        })
    }

    /// Frees maps, least recently drawn first, while the cache holds more than
    /// [`ENVIRONMENT_BUDGET_BYTES`], keeping those frame `tick` drew and those `kept` names.
    pub(super) fn trim(&mut self, rec: &mut Recorder, tick: u64, kept: &HashSet<&String>) {
        if self.bytes <= ENVIRONMENT_BUDGET_BYTES {
            return;
        }
        let freed: Vec<String> = evictions(
            self.maps.iter().map(|(id, cached)| {
                let protected = cached.last_used == tick || kept.contains(id);
                (id, cached.bytes, cached.last_used, protected)
            }),
            self.bytes,
            ENVIRONMENT_BUDGET_BYTES,
        )
        .into_iter()
        .cloned()
        .collect();
        for id in freed {
            if let Some(cached) = self.maps.remove(&id) {
                forget(rec, &cached.environment);
                self.bytes -= cached.bytes;
            }
        }
    }

    pub(super) fn release(&mut self, rec: &mut Recorder) {
        for (_, cached) in self.maps.drain() {
            forget(rec, &cached.environment);
        }
        self.bytes = 0;
        if let Some(brdf) = self.brdf.take() {
            rec.forget_buffer(brdf);
        }
    }
}

fn forget(rec: &mut Recorder, environment: &LoadedEnvironment) {
    rec.forget(environment.reflections);
    if let Some(background) = environment.background {
        rec.forget(background);
    }
}

fn texel_bytes(map: &Equirect) -> u64 {
    u64::from(map.width()) * u64::from(map.height()) * 8
}

/// Uploads `levels`, each half the size of the one before, as the mip levels of one
/// `Rgba16Float` texture.
fn upload(rec: &mut Recorder, label: &'static str, levels: &[&Equirect]) -> TexRef {
    let texture = rec.device().create_texture(&wgpu::TextureDescriptor {
        label: Some(label),
        size: wgpu::Extent3d {
            width: levels[0].width(),
            height: levels[0].height(),
            depth_or_array_layers: 1,
        },
        mip_level_count: levels.len() as u32,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba16Float,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    });
    for (mip, level) in (0..).zip(levels) {
        rec.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: mip,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&level.rgba16f()),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(level.width() * 8),
                rows_per_image: Some(level.height()),
            },
            wgpu::Extent3d {
                width: level.width(),
                height: level.height(),
                depth_or_array_layers: 1,
            },
        );
    }
    rec.adopt(Arc::new(texture))
}

/// The environment map `source` that `render_3d` node `node_id` lights its scene with, with its
/// background if `background`, or none for an empty source.
pub(super) fn environment(
    cx: &mut FrameCx<'_>,
    node_id: crate::node::NodeId,
    source: &str,
    background: bool,
) -> crate::Result<Option<LoadedEnvironment>> {
    if source.is_empty() {
        return Ok(None);
    }
    let media = cx
        .media()
        .ok_or_else(|| MediaError::source_not_found(source, "this render has no media store"))?;
    let resolver = media.get_environment_resolver(source)?;
    let tick = cx.tick();
    let (rec, cache) = cx.scene_cache();
    let loaded = cache
        .environments
        .load(rec, tick, &*resolver, background)
        .map_err(|failure| match failure {
            LoadError::Media(error) => LumenError::from(error),
            LoadError::Map(details) => super::render::render_error(
                node_id,
                format!("environment map `{source}`: {details}"),
            ),
        })?;
    cx.drew_model(resolver.id());
    Ok(Some(loaded))
}
