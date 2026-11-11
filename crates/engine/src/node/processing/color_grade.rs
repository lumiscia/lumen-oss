use foldhash::HashMap;
use lumen_gpu::{Recorder, TexRef, wgpu};

use crate::error::{LumenError, MediaError, MediaFailure};
use crate::media::{AssetKind, LutResolver, MediaIdentity, resolve_as};
use crate::node::processing::cube_lut::{CubeError, CubeLut};
use crate::node::{
    Deferred, DelegateEvalContext, DelegateEvaluable, NodeId, NodeParams,
    processing::RasterSampling,
};

use crate::gpu::params;

pub(crate) const SHADER: &str = concat!(
    include_str!("../../gpu/shaders/alpha.wgsl"),
    include_str!("../../gpu/shaders/color.wgsl"),
    include_str!("color_grade.wgsl")
);

pub const IDENTITY_LUT: &str = "identity";

/// Applies a LUT-driven color transform to a raster, looking up its sRGB-encoded colour. The
/// colour is unpremultiplied and clamped to 0..1 for the lookup, and alpha is left as it is.
#[derive(Debug, Clone, lumen_macros::Delegate)]
pub struct ColorGradeParams {
    /// An inline 1D LUT: `rgb1d:` and RGB stops separated by `;`, each three numbers from 0 to 1,
    /// such as `rgb1d:0,0,0;0.2,0.1,0.3;1,1,1`; or `identity`. A value outside 0..1 is an error:
    /// divide 0..255 values by 255. Set either this or `lut_source`.
    #[meta(name = "LUT", role = "lut", multiline, recommended_rows = 4)]
    pub lut: String,
    /// A 3D `.cube` LUT file (Adobe/Resolve) to grade with: a path or `media` alias, or
    /// `asset:<id>` for an asset of the host's. Tables up to 129 points per axis and 64 MiB are
    /// read, sampled with trilinear interpolation (`interpolation` `nearest` takes the nearest
    /// point), and `DOMAIN_MIN`/`DOMAIN_MAX` set the input range they cover. 1D `.cube` tables are
    /// not supported. Empty uses `lut`; setting both is an error.
    #[meta(name = "LUT source", role = "source_id")]
    #[media(lut)]
    pub lut_source: String,
    /// Blend amount for the LUT transform.
    #[meta(min = 0, max = 1, step = 0.01)]
    pub strength: f64,
    /// Sampling filter used when reading LUT data.
    #[meta(kind = "enum", enum_type = RasterSampling)]
    pub interpolation: RasterSampling,
}

impl Default for ColorGradeParams {
    fn default() -> Self {
        Self {
            lut: IDENTITY_LUT.to_string(),
            lut_source: String::new(),
            strength: 1.0,
            interpolation: RasterSampling::Linear,
        }
    }
}

/// Applies a LUT-driven color transform to a raster, looking up its sRGB-encoded colour.
#[derive(Debug, Clone, Default, lumen_macros::Node)]
#[node(kind = "color_grade", name = "Color Grade", category = "processing")]
pub struct ColorGrade {
    pub id: NodeId,
    #[params]
    pub params: ColorGradeParamsDelegate,

    #[input()]
    pub source: Option<NodeId>,
}

const CUBE_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(3, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
    lumen_gpu::LayoutEntry::compute(4, lumen_gpu::LayoutKind::Texture3d),
    lumen_gpu::LayoutEntry::compute(5, lumen_gpu::LayoutKind::Sampler),
];

const RECORD_LAYOUT: &[lumen_gpu::LayoutEntry] = &[
    lumen_gpu::LayoutEntry::compute(0, lumen_gpu::LayoutKind::Texture),
    lumen_gpu::LayoutEntry::compute(1, lumen_gpu::LayoutKind::Uniform),
    lumen_gpu::LayoutEntry::compute(2, lumen_gpu::LayoutKind::Storage),
    lumen_gpu::LayoutEntry::compute(3, crate::gpu::frame_graph::RECORD_STORAGE_TEXTURE),
];

const LUT_AND_LUT_SOURCE: &str = "`lut` and `lut_source` are both set; use one of them";

impl ColorGrade {
    /// The params whose literal values are wrong, with what is wrong, for checks that run before
    /// a render.
    pub(crate) fn literal_param_errors(&self) -> Vec<(&'static str, String)> {
        let params = &self.params;
        let mut errors = Vec::new();
        let lut = match &params.lut {
            Deferred::Value(lut) => Some(lut.as_str()),
            Deferred::Expr(_) => None,
        };
        if let Some(lut) = lut
            && let Err(details) = params::lut_stops(lut)
        {
            errors.push(("lut", details));
        }
        if let Deferred::Value(source) = &params.lut_source {
            if params::looks_like_inline_lut(source) {
                errors.push(("lut_source", params::INLINE_LUT_IN_LUT_SOURCE.to_string()));
            } else if !source.is_empty() && lut.is_some_and(|lut| !params::is_identity_lut(lut)) {
                errors.push(("lut", LUT_AND_LUT_SOURCE.to_string()));
            }
        }
        errors
    }
}

impl crate::gpu::frame_graph::RecordNode for ColorGrade {
    fn programs(&self, programs: &mut crate::gpu::frame_graph::Programs<'_>) -> crate::Result<()> {
        programs.compute("color-grade", SHADER, "cs_main", RECORD_LAYOUT)?;
        programs.compute("color-grade-cube", SHADER, "cs_cube", CUBE_LAYOUT)
    }

    fn record(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let source = cx.required(self.id, "source", self.source)?;
        self.grade(cx, source)
    }
}

impl ColorGrade {
    /// Grades `source`. It has a frame of its own, which is gone before the next node's input
    /// records, so the colour table doesn't add to the stack a long chain of nodes uses.
    #[inline(never)]
    fn grade(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
        source: crate::gpu::frame_graph::Raster,
    ) -> crate::Result<crate::gpu::frame_graph::Raster> {
        let evaluated = self.params.eval(&DelegateEvalContext {
            node_id: self.id,
            expr: &cx.expr(),
        })?;
        let size = source.domain.storage_size;
        let mut gpu_params = params::ColorGradeParams {
            strength: evaluated.strength as f32,
            interpolation: evaluated.interpolation as u32,
            _pad: [0; 2],
            domain_min: [0.0; 4],
            domain_max: [1.0; 4],
        };

        if evaluated.lut_source.is_empty() {
            let lut = params::ColorGradeLut::parse(self.id, &evaluated.lut)?;
            let tex = cx.storage_target(size);
            let program = cx.program("color-grade", SHADER, "cs_main", RECORD_LAYOUT)?;
            let uniform = cx.rec.uniform(&gpu_params);
            let storage = cx.rec.storage_parts(&[bytemuck::bytes_of(&lut)]);
            cx.dispatch(
                program,
                &[
                    (0, lumen_gpu::Res::Tex(source.tex)),
                    (1, uniform),
                    (2, storage),
                    (3, lumen_gpu::Res::Tex(tex)),
                ],
                params::dispatch_for(size),
            );
            return Ok(crate::gpu::frame_graph::Raster { tex, ..source });
        }

        if params::looks_like_inline_lut(&evaluated.lut_source) {
            return Err(params::lut_error(self.id, params::INLINE_LUT_IN_LUT_SOURCE));
        }
        if !params::is_identity_lut(&evaluated.lut) {
            return Err(params::lut_error(self.id, LUT_AND_LUT_SOURCE));
        }
        let loaded = self.load_cube(cx, &evaluated.lut_source)?;
        gpu_params.domain_min = [
            loaded.domain_min[0],
            loaded.domain_min[1],
            loaded.domain_min[2],
            0.0,
        ];
        gpu_params.domain_max = [
            loaded.domain_max[0],
            loaded.domain_max[1],
            loaded.domain_max[2],
            0.0,
        ];
        let sampler = cx
            .rec
            .sampler(CUBE_SAMPLER + gpu_params.interpolation as u64, || {
                let filter = if gpu_params.interpolation == 0 {
                    wgpu::FilterMode::Nearest
                } else {
                    wgpu::FilterMode::Linear
                };
                wgpu::SamplerDescriptor {
                    label: Some("lumen color grade lut sampler"),
                    address_mode_u: wgpu::AddressMode::ClampToEdge,
                    address_mode_v: wgpu::AddressMode::ClampToEdge,
                    address_mode_w: wgpu::AddressMode::ClampToEdge,
                    mag_filter: filter,
                    min_filter: filter,
                    ..wgpu::SamplerDescriptor::default()
                }
            });
        let tex = cx.storage_target(size);
        let program = cx.program("color-grade-cube", SHADER, "cs_cube", CUBE_LAYOUT)?;
        let uniform = cx.rec.uniform(&gpu_params);
        cx.dispatch(
            program,
            &[
                (0, lumen_gpu::Res::Tex(source.tex)),
                (1, uniform),
                (3, lumen_gpu::Res::Tex(tex)),
                (4, lumen_gpu::Res::Tex(loaded.tex)),
                (5, lumen_gpu::Res::Sampler(sampler)),
            ],
            params::dispatch_for(size),
        );
        Ok(crate::gpu::frame_graph::Raster { tex, ..source })
    }

    /// The table `source` names, loaded once per identity.
    fn load_cube(
        &self,
        cx: &mut crate::gpu::frame_graph::FrameCx<'_>,
        source: &str,
    ) -> crate::Result<LoadedLut> {
        let at = cx.node_param(self.id, "lut_source");
        let in_node = |error: MediaError| {
            LumenError::from(error).media_used_as(&[AssetKind::Lut], || at.clone())
        };
        let media = cx.media().ok_or_else(|| {
            in_node(MediaError::source_not_found(
                source,
                "this render has no media store",
            ))
        })?;
        let (identity, resolver) = resolve_as::<dyn LutResolver>(media, source).map_err(in_node)?;
        let tick = cx.tick();
        let (rec, cache) = cx.lut_cache();
        let loaded = cache
            .load(rec, tick, source, &identity, &*resolver)
            .map_err(in_node)?;
        cx.read_source(AssetKind::Lut, source, Some(&identity));
        Ok(loaded)
    }
}

/// The sampler key of `color_grade`'s table, plus the interpolation.
const CUBE_SAMPLER: u64 = 0x6c75_7463_7562_6500;

/// A table ready to grade with.
#[derive(Clone, Copy)]
pub(crate) struct LoadedLut {
    tex: TexRef,
    domain_min: [f32; 3],
    domain_max: [f32; 3],
}

struct CachedLut {
    lut: LoadedLut,
    bytes: u64,
    last_used: u64,
}

/// `.cube` tables on the GPU by their media identity, uploaded once.
#[derive(Default)]
pub(crate) struct LutCache {
    luts: HashMap<String, CachedLut>,
    bytes: u64,
}

impl LutCache {
    /// The table `resolver` serves as `identity`, marked used in `tick`. Its file is read and
    /// uploaded only the first time.
    pub(crate) fn load(
        &mut self,
        rec: &mut Recorder,
        tick: u64,
        source: &str,
        identity: &MediaIdentity,
        resolver: &dyn LutResolver,
    ) -> Result<LoadedLut, MediaError> {
        if let Some(cached) = self.luts.get_mut(identity.as_str()) {
            cached.last_used = tick;
            return Ok(cached.lut);
        }
        let bytes = resolver.data()?;
        let cube = CubeLut::parse(&bytes).map_err(|error| lut_failure(source, error))?;
        drop(bytes);
        let size = cube.size;
        let texels: Vec<[u16; 4]> = cube
            .texels
            .iter()
            .map(|[r, g, b]| [r, g, b, &1.0].map(|value| half::f16::from_f32(*value).to_bits()))
            .collect();
        let max = rec.device().limits().max_texture_dimension_3d;
        if size > max {
            return Err(MediaFailure::too_large(
                source,
                format!("the table is larger than this device's {max}-point 3D texture limit"),
                u64::from(max),
                Some(u64::from(size)),
            )
            .wanting(&[AssetKind::Lut])
            .into());
        }
        let extent = wgpu::Extent3d {
            width: size,
            height: size,
            depth_or_array_layers: size,
        };
        let texture = rec.device().create_texture(&wgpu::TextureDescriptor {
            label: Some("lumen color grade lut"),
            size: extent,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba16Float,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        rec.queue().write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            bytemuck::cast_slice(&texels),
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(size * 8),
                rows_per_image: Some(size),
            },
            extent,
        );
        let bytes = u64::from(size).pow(3) * 8;
        let lut = LoadedLut {
            tex: rec.adopt(std::sync::Arc::new(texture)),
            domain_min: cube.domain_min,
            domain_max: cube.domain_max,
        };
        self.bytes += bytes;
        self.luts.insert(
            identity.as_str().to_string(),
            CachedLut {
                lut,
                bytes,
                last_used: tick,
            },
        );
        Ok(lut)
    }

    /// Frees tables, least recently used first, while the cache holds more than `budget` bytes,
    /// keeping those frame `tick` used.
    pub(crate) fn trim(&mut self, rec: &mut Recorder, tick: u64, budget: u64) {
        while self.bytes > budget {
            let Some((id, _)) = self
                .luts
                .iter()
                .filter(|(_, cached)| cached.last_used != tick)
                .min_by_key(|(id, cached)| (cached.last_used, *id))
                .map(|(id, cached)| (id.clone(), cached.last_used))
            else {
                return;
            };
            if let Some(cached) = self.luts.remove(&id) {
                rec.forget(cached.lut.tex);
                self.bytes -= cached.bytes;
            }
        }
    }

    pub(crate) fn bytes(&self) -> u64 {
        self.bytes
    }

    pub(crate) fn release(&mut self, rec: &mut Recorder) {
        for (_, cached) in self.luts.drain() {
            rec.forget(cached.lut.tex);
        }
        self.bytes = 0;
    }
}

/// Reads `bytes` as the `.cube` table `source` names, and fails as a render that grades with it
/// would, short of the device's 3D texture size limit.
pub fn check_lut(source: &str, bytes: &[u8]) -> Result<(), MediaError> {
    CubeLut::parse(bytes)
        .map(drop)
        .map_err(|error| lut_failure(source, error))
}

/// Why `error`, from reading `source` as a `.cube` file, makes it unusable.
fn lut_failure(source: &str, error: CubeError) -> MediaError {
    let failure = match error {
        CubeError::Invalid(details) => MediaFailure::decode(source, details),
        CubeError::Unsupported(details) => MediaFailure::unsupported(source, details),
        CubeError::TooLarge {
            details,
            limit,
            actual,
        } => MediaFailure::too_large(source, details, limit, Some(actual)),
    };
    failure.wanting(&[AssetKind::Lut]).into()
}
