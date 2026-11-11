//! Environment maps on the GPU: each source's prefiltered reflections and irradiance, made once,
//! and the map at its decoded size once a frame shows it as a background.

use std::collections::HashSet;
use std::sync::Arc;

use foldhash::HashMap;
use lumen_3d::environment::{self, EnvironmentError, Equirect};
use lumen_gpu::{BufRef, Recorder, TexRef, wgpu};

use super::collect::{evictions, storage_buffer};
use crate::error::{LumenError, MediaError, MediaFailure};
use crate::gpu::frame_graph::FrameCx;
use crate::media::{AssetKind, EnvironmentResolver, MediaIdentity, resolve_as, sniff};

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

/// Environment maps by their identity, and the split-sum table they are all lit with.
#[derive(Default)]
pub(crate) struct EnvironmentCache {
    maps: HashMap<String, Cached>,
    bytes: u64,
    brdf: Option<BufRef>,
}

impl EnvironmentCache {
    /// The map `resolver` serves as `identity`, with its background if `background`, marked drawn
    /// in `tick`. Its file is read again only to add a background it was first loaded without.
    fn load(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        source: &str,
        (identity, resolver): (&MediaIdentity, &dyn EnvironmentResolver),
        background: bool,
    ) -> Result<LoadedEnvironment, MediaError> {
        if let Some(cached) = self.maps.get_mut(identity.as_str())
            && (!background || cached.environment.background.is_some())
        {
            cached.last_used = tick;
            return Ok(cached.environment);
        }
        let bytes = resolver.data()?;
        let map = environment::decode(&bytes)
            .map_err(|error| environment_failure(source, &bytes, error))?;
        drop(bytes);
        let max = rec.device().limits().max_texture_dimension_2d;
        if map.width().max(map.height()) > max {
            return Err(MediaFailure::too_large(
                source,
                format!("the map is larger than this device's {max}-pixel texture limit"),
                u64::from(max),
                Some(u64::from(map.width().max(map.height()))),
            )
            .wanting(&[AssetKind::Environment])
            .into());
        }
        let mut cached = match self.maps.remove(identity.as_str()) {
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
        self.maps.insert(identity.as_str().to_string(), cached);
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

/// Why `bytes`, the file `source`, is not an environment map that can be read.
fn environment_failure(source: &str, bytes: &[u8], error: EnvironmentError) -> MediaError {
    let wanted = [AssetKind::Environment];
    let failure = match error {
        EnvironmentError::Unsupported(_) if !environment::is_environment_magic(bytes) => {
            match sniff(bytes).other_than(AssetKind::Environment) {
                Some(found) => MediaFailure::wrong_kind(
                    source,
                    &wanted,
                    found,
                    "the file is not a Radiance .hdr or OpenEXR .exr map",
                ),
                None => MediaFailure::decode(source, error.to_string()),
            }
        }
        EnvironmentError::Invalid(details) => MediaFailure::decode(source, details),
        EnvironmentError::Unsupported(details) => MediaFailure::unsupported(source, details),
        EnvironmentError::TooLarge {
            details,
            limit,
            actual,
        } => MediaFailure::too_large(source, details, limit, actual),
    };
    failure.wanting(&wanted).into()
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
    let at = cx.node_param(node_id, "environment_map");
    let in_node = |error: MediaError| {
        LumenError::from(error).media_used_as(&[AssetKind::Environment], || at.clone())
    };
    let media = cx.media().ok_or_else(|| {
        in_node(MediaError::source_not_found(
            source,
            "this render has no media store",
        ))
    })?;
    let (identity, resolver) =
        resolve_as::<dyn EnvironmentResolver>(media, source).map_err(in_node)?;
    let tick = cx.tick();
    let (rec, cache) = cx.scene_cache();
    let loaded = cache
        .environments
        .load(rec, tick, source, (&identity, &*resolver), background)
        .map_err(in_node)?;
    cx.drew_model(identity.as_str());
    cx.read_source(AssetKind::Environment, source, Some(&identity));
    Ok(Some(loaded))
}

#[cfg(test)]
mod tests {
    use lumen_3d::environment;

    use super::environment_failure;
    use crate::error::MediaReason;
    use crate::media::AssetKind;

    /// What reading `bytes` as an environment map fails with: its code and the kind found.
    fn read_as_environment(bytes: &[u8]) -> (&'static str, Option<AssetKind>) {
        let error = environment::decode(bytes).expect_err("not a readable map");
        let error = environment_failure("sky.hdr", bytes, error);
        let failure = error.failure().expect("a media failure");
        assert_eq!(failure.wanted, [AssetKind::Environment]);
        assert_eq!(failure.source, "sky.hdr");
        (failure.code(), failure.found)
    }

    #[test]
    fn an_environment_map_that_cant_be_read_fails_by_what_the_file_is() {
        let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
        png.extend_from_slice(&[0; 32]);
        assert_eq!(
            read_as_environment(&png),
            ("media.wrong_kind", Some(AssetKind::Image))
        );
        assert_eq!(
            read_as_environment(b"not a map at all"),
            ("media.decode", None)
        );
        assert_eq!(
            read_as_environment(b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 4 +X 8\n"),
            ("media.decode", None)
        );
    }

    #[test]
    fn an_environment_map_over_a_limit_reports_the_limit_and_the_size() {
        let bytes = b"#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y 1 +X 100000\n";
        let error = environment::decode(bytes).expect_err("over the limit");
        let error = environment_failure("sky.hdr", bytes, error);
        let failure = error.failure().unwrap();
        assert_eq!(failure.code(), "media.too_large");
        assert!(matches!(
            failure.reason,
            MediaReason::TooLarge { limit, actual: Some(100_000), .. } if limit == 8192
        ));
    }
}
