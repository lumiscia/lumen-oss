//! The textures nodes record into, and the programs and passes they record with.

use std::sync::Arc;

use lumen_gpu::{
    Dispatch, ProgramRef, Res, SamplerRef, Size, TexRef, TextureDesc, TextureDomain, wgpu,
};

use super::{
    CachedShader, ComputeProgram, FrameCx, Raster, RenderProgram, ShaderParts, dispatch_for,
    gpu_error,
};
use crate::{gpu::color::WORKING_COLOR_FORMAT as WORKING, node::NodeId};

pub(super) fn copyable_texture_desc(size: Size) -> TextureDesc {
    TextureDesc {
        domain: TextureDomain::full_frame(size),
        format: WORKING,
        usage: wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::STORAGE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT,
    }
}

/// `extent` rounded up to the next size class that textures are pooled by.
pub(crate) fn size_class(extent: u32) -> u32 {
    let step = match extent {
        0..256 => 16,
        256..512 => 32,
        512..1024 => 64,
        _ => 128,
    };
    extent.next_multiple_of(step)
}

/// A raster's domain within `display`, rounded out to a size class so that bounds moving from
/// frame to frame keep finding the same textures, and the bind groups cached on them.
pub(crate) fn quantized_domain(display: Size, data: lumen_gpu::RectI) -> TextureDomain {
    let domain = TextureDomain::bounded(display, data);
    let rect = domain.data_rect;
    if data.is_empty() {
        return domain;
    }
    let class = |extent: u32, limit: u32| size_class(extent).min(limit.max(1));
    let width = class(rect.width, display.width);
    let height = class(rect.height, display.height);
    let x = rect.x.min(display.width.saturating_sub(width) as i32);
    let y = rect.y.min(display.height.saturating_sub(height) as i32);
    TextureDomain::bounded(display, lumen_gpu::RectI::new(x, y, width, height))
}

impl FrameCx<'_> {
    /// `raster` stored at its full display size, copied there when it is bounded.
    pub(crate) fn full_frame(&mut self, raster: Raster) -> Raster {
        if raster.domain.is_full_frame() {
            return raster;
        }
        let size = raster.domain.display_size();
        let key = (raster.tex, size.width, size.height);
        if let Some(full) = self.st.full_frames.get(&key) {
            return *full;
        }
        let tex = self
            .rec
            .acquire(size, WORKING, copyable_texture_desc(size).usage);
        // A reused texture still holds an earlier frame, and the copy only writes the data.
        self.rec.clear(tex);
        let data = raster.domain.data_rect;
        self.rec.copy(
            raster.tex,
            tex,
            [data.x as u32, data.y as u32],
            raster.domain.storage_size,
        );
        let full = Raster {
            tex,
            domain: TextureDomain::full_frame(size),
            bounds: None,
            reduced: None,
        };
        self.st.full_frames.insert(key, full);
        full
    }

    /// The pixels of a box a source draws in this frame, or `None` when it should fill the canvas.
    pub(crate) fn bounds_of(&self, bounds: Option<[f32; 4]>) -> Option<lumen_gpu::RectI> {
        if self.settings.full_frame_sources {
            return None;
        }
        bounds.and_then(lumen_gpu::RectI::covering)
    }

    pub(crate) fn source_domain(&self, bounds: Option<lumen_gpu::RectI>) -> TextureDomain {
        self.domain_within(self.canvas_size(), bounds)
    }

    pub(crate) fn domain_within(
        &self,
        display: Size,
        bounds: Option<lumen_gpu::RectI>,
    ) -> TextureDomain {
        match bounds {
            Some(bounds) => quantized_domain(display, bounds),
            None => TextureDomain::full_frame(display),
        }
    }

    /// A storage target that starts transparent, for a writer that may not cover all of it.
    pub(crate) fn cleared_storage_target(&mut self, size: Size) -> TexRef {
        let tex = self
            .rec
            .acquire(size, WORKING, copyable_texture_desc(size).usage);
        self.rec.clear(tex);
        tex
    }

    pub(crate) fn storage_target(&mut self, size: Size) -> TexRef {
        self.rec
            .acquire(size, WORKING, TextureDesc::storage(size, WORKING).usage)
    }

    pub(crate) fn float_target(&mut self, size: Size) -> TexRef {
        let format = wgpu::TextureFormat::Rgba16Float;
        self.rec
            .acquire(size, format, TextureDesc::storage(size, format).usage)
    }

    pub(crate) fn render_target(&mut self, size: Size) -> TexRef {
        self.rec.acquire(
            size,
            WORKING,
            TextureDesc::render_target(size, WORKING).usage,
        )
    }

    /// The frame's output. An output kept from an earlier frame is copied into an external target,
    /// so targets made from its usage take copies.
    pub(crate) fn output_target(&mut self, size: Size, format: wgpu::TextureFormat) -> TexRef {
        let usage = wgpu::TextureUsages::COPY_SRC
            | wgpu::TextureUsages::COPY_DST
            | wgpu::TextureUsages::TEXTURE_BINDING
            | wgpu::TextureUsages::RENDER_ATTACHMENT;
        self.rec.acquire(size, format, usage)
    }

    pub(crate) fn program(&mut self, program: &ComputeProgram) -> crate::Result<ProgramRef> {
        self.rec
            .compute_program(program.label, program.shader, program.entry, program.layout)
            .map_err(gpu_error)
    }

    pub(crate) fn render_program(
        &mut self,
        program: &RenderProgram,
        target: lumen_gpu::RenderTargetSpec,
        variant: u64,
    ) -> crate::Result<ProgramRef> {
        self.rec
            .render_program(
                program.label,
                program.shader,
                program.vertex,
                program.fragment,
                program.layout,
                target,
                variant,
            )
            .map_err(gpu_error)
    }

    /// Runs `program` from `source` into `target` over `size`.
    pub(crate) fn spatial_pass<T: bytemuck::Pod>(
        &mut self,
        program: &ComputeProgram,
        source: TexRef,
        params: &T,
        target: TexRef,
        size: Size,
    ) -> crate::Result<()> {
        let program = self.program(program)?;
        let uniform = self.rec.uniform(params);
        self.rec.compute(
            program,
            &[(0, Res::Tex(source)), (1, uniform), (2, Res::Tex(target))],
            dispatch_for(size),
        );
        Ok(())
    }

    /// A filter over `source`, writing a raster of the same domain, metadata and bounds.
    pub(crate) fn filter<T: bytemuck::Pod>(
        &mut self,
        program: &ComputeProgram,
        source: Raster,
        params: &T,
    ) -> crate::Result<Raster> {
        let size = source.domain.storage_size;
        let tex = self.storage_target(size);
        self.spatial_pass(program, source.tex, params, tex, size)?;
        Ok(Raster { tex, ..source })
    }

    pub(crate) fn dispatch(
        &mut self,
        program: ProgramRef,
        bindings: &[(u32, Res)],
        dispatch: Dispatch,
    ) {
        self.rec.compute(program, bindings, dispatch);
    }

    /// A transparent raster of `display` size, which stores a texel nothing writes.
    pub(crate) fn transparent(&mut self, display: Size) -> Raster {
        let tex = match self.st.transparent {
            Some(tex) => tex,
            None => {
                let texture = self.rec.device().create_texture(&wgpu::TextureDescriptor {
                    label: Some("lumen frame graph transparent"),
                    size: Size::new(1, 1).as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: WORKING,
                    usage: copyable_texture_desc(Size::new(1, 1)).usage,
                    view_formats: &[],
                });
                let tex = self.rec.adopt(Arc::new(texture));
                self.st.transparent = Some(tex);
                tex
            }
        };
        Raster {
            tex,
            domain: TextureDomain::bounded(display, lumen_gpu::RectI::new(0, 0, 0, 0)),
            bounds: None,
            reduced: None,
        }
    }

    /// Whether `raster` is one [`Self::transparent`] made.
    pub(crate) fn is_transparent(&self, raster: &Raster) -> bool {
        Some(raster.tex) == self.st.transparent
    }

    /// A one-texel float texture for gaussian levels, or a running sum, a pass binds but does not
    /// read.
    pub(crate) fn unused_float(&mut self) -> TexRef {
        match self.st.dummy_float {
            Some(tex) => tex,
            None => {
                let texture = self.rec.device().create_texture(&wgpu::TextureDescriptor {
                    label: Some("lumen frame graph unused level"),
                    size: Size::new(1, 1).as_extent(),
                    mip_level_count: 1,
                    sample_count: 1,
                    dimension: wgpu::TextureDimension::D2,
                    format: wgpu::TextureFormat::Rgba16Float,
                    usage: wgpu::TextureUsages::TEXTURE_BINDING,
                    view_formats: &[],
                });
                let tex = self.rec.adopt(Arc::new(texture));
                self.st.dummy_float = Some(tex);
                tex
            }
        }
    }

    pub(crate) fn sampler(
        &mut self,
        key: u64,
        desc: impl FnOnce() -> wgpu::SamplerDescriptor<'static>,
    ) -> SamplerRef {
        self.rec.sampler(key, desc)
    }

    /// The parsed and assembled form of a runtime shader source, built once per source.
    pub(crate) fn cached_shader(
        &mut self,
        source: &str,
        inputs: usize,
        build: impl FnOnce() -> crate::Result<ShaderParts>,
    ) -> crate::Result<CachedShader> {
        self.st
            .shaders
            .cached_shader(self.st.tick, source, inputs, build)
    }

    pub(crate) fn dynamic_program(
        &mut self,
        node: NodeId,
        label: &str,
        shader: &CachedShader,
    ) -> crate::Result<ProgramRef> {
        super::dynamic_program(self.rec, node, label, shader)
    }
}
