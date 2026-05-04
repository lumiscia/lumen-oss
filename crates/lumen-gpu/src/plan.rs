use crate::{
    BufferDesc, BufferId, BufferResource, ComputePassDesc, NodeKey, ParamKey, ParamSlot,
    ParamTarget, Pass, PassDesc, PassId, Program, ProgramDesc, ProgramId, RenderPassDesc,
    SamplerId, SamplerResource, TextureDesc, TextureId, TextureResource,
};

#[derive(Debug, Clone)]
pub struct RenderPlan {
    pub(crate) textures: Vec<TextureResource>,
    pub(crate) buffers: Vec<BufferResource>,
    pub(crate) samplers: Vec<SamplerResource>,
    pub(crate) programs: Vec<Program>,
    pub(crate) passes: Vec<Pass>,
    params: Vec<ParamSlot>,
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

    pub fn params(&self) -> &[ParamSlot] {
        &self.params
    }
}

#[derive(Debug, Default)]
pub struct RenderPlanBuilder {
    textures: Vec<TextureResource>,
    buffers: Vec<BufferResource>,
    samplers: Vec<SamplerResource>,
    programs: Vec<Program>,
    passes: Vec<Pass>,
    params: Vec<ParamSlot>,
}

impl RenderPlanBuilder {
    pub fn texture(&mut self, label: impl Into<Option<String>>, desc: TextureDesc) -> TextureId {
        let id = TextureId(self.textures.len() as u32);
        self.textures.push(TextureResource {
            id,
            label: label.into(),
            desc,
            owner: None,
        });
        id
    }

    pub fn texture_for(
        &mut self,
        owner: NodeKey,
        label: impl Into<Option<String>>,
        desc: TextureDesc,
    ) -> TextureId {
        let id = self.texture(label, desc);
        self.textures[id.0 as usize].owner = Some(owner);
        id
    }

    pub fn buffer(&mut self, label: impl Into<Option<String>>, desc: BufferDesc) -> BufferId {
        let id = BufferId(self.buffers.len() as u32);
        self.buffers.push(BufferResource {
            id,
            label: label.into(),
            desc,
            owner: None,
        });
        id
    }

    pub fn buffer_for(
        &mut self,
        owner: NodeKey,
        label: impl Into<Option<String>>,
        desc: BufferDesc,
    ) -> BufferId {
        let id = self.buffer(label, desc);
        self.buffers[id.0 as usize].owner = Some(owner);
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
        self.programs.push(Program {
            id,
            owner: None,
            desc,
        });
        id
    }

    pub fn program_for(&mut self, owner: NodeKey, desc: ProgramDesc) -> ProgramId {
        let id = self.program(desc);
        self.programs[id.0 as usize].owner = Some(owner);
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

    pub fn param(&mut self, key: ParamKey, target: ParamTarget) -> &mut Self {
        self.params.push(ParamSlot { key, target });
        self
    }

    pub fn build(self) -> RenderPlan {
        RenderPlan {
            textures: self.textures,
            buffers: self.buffers,
            samplers: self.samplers,
            programs: self.programs,
            passes: self.passes,
            params: self.params,
        }
    }

    fn pass(&mut self, desc: PassDesc) -> PassId {
        let id = PassId(self.passes.len() as u32);
        self.passes.push(Pass { id, desc });
        id
    }
}
