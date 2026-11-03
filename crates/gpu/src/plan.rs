use std::sync::atomic::{AtomicU64, Ordering};

use crate::{
    BufferDesc, BufferId, BufferResource, ComputePassDesc, Pass, PassDesc, PassId, Program,
    ProgramDesc, ProgramId, RenderPassDesc, SamplerId, SamplerResource, TextureDesc, TextureId,
    TextureResource,
};

static NEXT_PLAN_ID: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone)]
pub struct RenderPlan {
    /// Distinguishes this plan from every other built in the process, so a renderer knows which
    /// one it prepared.
    pub(crate) id: u64,
    pub(crate) textures: Vec<TextureResource>,
    pub(crate) buffers: Vec<BufferResource>,
    pub(crate) samplers: Vec<SamplerResource>,
    pub(crate) programs: Vec<Program>,
    pub(crate) passes: Vec<Pass>,
}

impl RenderPlan {
    pub fn builder() -> RenderPlanBuilder {
        RenderPlanBuilder::default()
    }

    pub fn textures(&self) -> &[TextureResource] {
        &self.textures
    }

    pub fn buffers(&self) -> &[BufferResource] {
        &self.buffers
    }

    pub fn samplers(&self) -> &[SamplerResource] {
        &self.samplers
    }

    pub fn programs(&self) -> &[Program] {
        &self.programs
    }

    pub fn passes(&self) -> &[Pass] {
        &self.passes
    }
}

#[derive(Debug, Default)]
pub struct RenderPlanBuilder {
    textures: Vec<TextureResource>,
    buffers: Vec<BufferResource>,
    samplers: Vec<SamplerResource>,
    programs: Vec<Program>,
    passes: Vec<Pass>,
}

impl RenderPlanBuilder {
    pub fn texture(&mut self, label: impl Into<Option<String>>, desc: TextureDesc) -> TextureId {
        let id = TextureId(self.textures.len() as u32);
        self.textures.push(TextureResource {
            id,
            label: label.into(),
            desc,
        });
        id
    }

    pub fn buffer(&mut self, label: impl Into<Option<String>>, desc: BufferDesc) -> BufferId {
        let id = BufferId(self.buffers.len() as u32);
        self.buffers.push(BufferResource {
            id,
            label: label.into(),
            desc,
        });
        id
    }

    pub fn sampler(
        &mut self,
        label: impl Into<Option<String>>,
        desc: wgpu::SamplerDescriptor<'static>,
    ) -> SamplerId {
        let id = SamplerId(self.samplers.len() as u32);
        self.samplers.push(SamplerResource {
            id,
            label: label.into(),
            desc,
        });
        id
    }

    pub fn program(&mut self, desc: ProgramDesc) -> ProgramId {
        let id = ProgramId(self.programs.len() as u32);
        self.programs.push(Program { id, desc });
        id
    }

    pub fn render_pass(&mut self, desc: RenderPassDesc) -> PassId {
        self.pass(PassDesc::Render(desc))
    }

    pub fn compute_pass(&mut self, desc: ComputePassDesc) -> PassId {
        self.pass(PassDesc::Compute(desc))
    }

    pub fn copy_texture(&mut self, desc: crate::CopyTextureDesc) -> PassId {
        self.pass(PassDesc::CopyTexture(desc))
    }

    pub fn texture_count(&self) -> usize {
        self.textures.len()
    }

    pub fn buffer_count(&self) -> usize {
        self.buffers.len()
    }

    pub fn pass_count(&self) -> usize {
        self.passes.len()
    }

    pub fn texture_desc(&self, id: TextureId) -> Option<TextureDesc> {
        self.textures.get(id.0 as usize).map(|texture| texture.desc)
    }

    pub fn build(self) -> RenderPlan {
        RenderPlan {
            id: NEXT_PLAN_ID.fetch_add(1, Ordering::Relaxed),
            textures: self.textures,
            buffers: self.buffers,
            samplers: self.samplers,
            programs: self.programs,
            passes: self.passes,
        }
    }

    fn pass(&mut self, desc: PassDesc) -> PassId {
        let id = PassId(self.passes.len() as u32);
        self.passes.push(Pass { id, desc });
        id
    }
}
