use super::program::RuntimeProgram;
use super::{
    BindKey, FrameError, KeyRes, MAX_EXTRA_TARGETS, PassBindings, ProgramRef, RecordedPass,
    Recorder, Res, Result, TargetLoad, TexRef,
};
use crate::renderer::ValidationScope;

/// A render pass kept open at submit so later draws onto the same attachments join it.
struct OpenRender {
    pass: wgpu::RenderPass<'static>,
    target: Option<TexRef>,
    extra: [Option<TexRef>; MAX_EXTRA_TARGETS],
    depth: Option<TexRef>,
    viewport_set: bool,
}

impl Recorder {
    /// Encodes and submits the resolved frame.
    pub fn submit(&mut self) -> Result<wgpu::SubmissionIndex> {
        let scope = ValidationScope::push(&self.device);
        self.resolve_bind_groups()?;
        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lumen frame graph"),
            });
        let mut compute: Option<wgpu::ComputePass<'static>> = None;
        let mut open: Option<OpenRender> = None;
        let mut offsets = Vec::with_capacity(4);
        let mut resolved = self.resolved.iter();
        for pass in &self.passes {
            match pass {
                RecordedPass::Compute {
                    program,
                    bindings,
                    dispatch,
                } => {
                    open = None;
                    let group = *resolved.next().expect("resolved bind group");
                    let RuntimeProgram::Compute(pipeline, _) = &self.programs[program.0 as usize]
                        .as_ref()
                        .expect("program")
                        .program
                    else {
                        return Err(FrameError::Misuse(
                            "recorded a render program in a compute pass".to_string(),
                        ));
                    };
                    let pass = compute.get_or_insert_with(|| {
                        encoder
                            .begin_compute_pass(&wgpu::ComputePassDescriptor {
                                label: Some("lumen frame graph compute"),
                                timestamp_writes: None,
                            })
                            .forget_lifetime()
                    });
                    dynamic_offsets(bindings, &mut offsets);
                    pass.set_pipeline(pipeline);
                    pass.set_bind_group(0, self.group(group), &offsets);
                    pass.dispatch_workgroups(dispatch[0], dispatch[1], dispatch[2]);
                }
                RecordedPass::Render {
                    program,
                    bindings,
                    target,
                    extra,
                    load,
                    depth,
                    vertices,
                    instances,
                    viewport,
                } => {
                    compute = None;
                    let group = *resolved.next().expect("resolved bind group");
                    let RuntimeProgram::Render(pipeline, _) = &self.programs[program.0 as usize]
                        .as_ref()
                        .expect("program")
                        .program
                    else {
                        return Err(FrameError::Misuse(
                            "recorded a compute program in a render pass".to_string(),
                        ));
                    };
                    let depth_tex = depth.map(|(texture, _)| texture);
                    let continues = matches!(load, TargetLoad::Load)
                        && depth.is_none_or(|(_, load)| matches!(load, TargetLoad::Load))
                        && open.as_ref().is_some_and(|open| {
                            open.target == *target
                                && open.extra == *extra
                                && open.depth == depth_tex
                                && (viewport.is_some() || !open.viewport_set)
                        });
                    if !continues {
                        drop(open.take());
                        let color_attachments: Vec<_> = std::iter::once(target)
                            .chain(extra)
                            .flatten()
                            .map(|&target| {
                                Some(wgpu::RenderPassColorAttachment {
                                    view: self.view(target),
                                    depth_slice: None,
                                    resolve_target: None,
                                    ops: wgpu::Operations {
                                        load: match load {
                                            TargetLoad::Clear => {
                                                wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT)
                                            }
                                            TargetLoad::Load => wgpu::LoadOp::Load,
                                        },
                                        store: wgpu::StoreOp::Store,
                                    },
                                })
                            })
                            .collect();
                        open = Some(OpenRender {
                            pass: encoder
                                .begin_render_pass(&wgpu::RenderPassDescriptor {
                                    label: None,
                                    color_attachments: &color_attachments,
                                    depth_stencil_attachment: depth.map(|(depth, load)| {
                                        wgpu::RenderPassDepthStencilAttachment {
                                            view: self.view(depth),
                                            depth_ops: Some(wgpu::Operations {
                                                load: match load {
                                                    TargetLoad::Clear => wgpu::LoadOp::Clear(1.0),
                                                    TargetLoad::Load => wgpu::LoadOp::Load,
                                                },
                                                store: wgpu::StoreOp::Store,
                                            }),
                                            stencil_ops: None,
                                        }
                                    }),
                                    occlusion_query_set: None,
                                    timestamp_writes: None,
                                    multiview_mask: None,
                                })
                                .forget_lifetime(),
                            target: *target,
                            extra: *extra,
                            depth: depth_tex,
                            viewport_set: false,
                        });
                    }
                    let open = open.as_mut().expect("open render pass");
                    dynamic_offsets(bindings, &mut offsets);
                    open.pass.set_pipeline(pipeline);
                    open.pass.set_bind_group(0, self.group(group), &offsets);
                    if let Some([x, y, width, height]) = *viewport {
                        open.pass.set_viewport(x, y, width, height, 0.0, 1.0);
                        open.viewport_set = true;
                    }
                    open.pass.draw(0..*vertices, 0..*instances);
                }
                RecordedPass::Copy {
                    source,
                    destination,
                    origin,
                    size,
                } => {
                    compute = None;
                    open = None;
                    encoder.copy_texture_to_texture(
                        self.texture(*source).as_image_copy(),
                        wgpu::TexelCopyTextureInfo {
                            texture: self.texture(*destination),
                            mip_level: 0,
                            origin: wgpu::Origin3d {
                                x: origin[0],
                                y: origin[1],
                                z: 0,
                            },
                            aspect: wgpu::TextureAspect::All,
                        },
                        size.as_extent(),
                    );
                }
                RecordedPass::Clear { texture } => {
                    compute = None;
                    open = None;
                    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                        label: None,
                        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                            view: self.view(*texture),
                            depth_slice: None,
                            resolve_target: None,
                            ops: wgpu::Operations {
                                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                                store: wgpu::StoreOp::Store,
                            },
                        })],
                        depth_stencil_attachment: None,
                        occlusion_query_set: None,
                        timestamp_writes: None,
                        multiview_mask: None,
                    });
                }
            }
        }
        drop(compute);
        drop(open);
        let submission = self.queue.submit([encoder.finish()]);
        self.stats.passes += self.passes.len() as u64;
        scope.finish()?;
        Ok(submission)
    }

    fn view(&self, tex: TexRef) -> &wgpu::TextureView {
        &self.entries[tex.0 as usize].as_ref().expect("texture").view
    }

    fn group(&self, group: u32) -> &wgpu::BindGroup {
        &self.groups[group as usize].as_ref().expect("bind group").0
    }

    fn resolve_bind_groups(&mut self) -> Result<()> {
        let passes = std::mem::take(&mut self.passes);
        let mut resolved = std::mem::take(&mut self.resolved);
        resolved.clear();
        let mut failed = None;
        for pass in &passes {
            let (program, bindings) = match pass {
                RecordedPass::Compute {
                    program, bindings, ..
                }
                | RecordedPass::Render {
                    program, bindings, ..
                } => (*program, bindings),
                _ => continue,
            };
            match self.bind_group(program, bindings) {
                Ok(group) => resolved.push(group),
                Err(error) => {
                    failed = Some(error);
                    break;
                }
            }
        }
        self.passes = passes;
        self.resolved = resolved;
        failed.map_or(Ok(()), Err)
    }

    fn bind_group(&mut self, program: ProgramRef, bindings: &PassBindings) -> Result<u32> {
        let key = bindings.key(program);
        if let Some(&group) = self.group_keys.get(&key) {
            self.stats.bind_group_hits += 1;
            self.groups[group as usize].as_mut().expect("bind group").1 = self.frame;
            return Ok(group);
        }
        self.stats.bind_group_misses += 1;
        let layout = match &self.programs[program.0 as usize]
            .as_ref()
            .ok_or_else(|| FrameError::Misuse("bind group for an evicted program".to_string()))?
            .program
        {
            RuntimeProgram::Compute(_, layout) | RuntimeProgram::Render(_, layout) => layout,
        };
        let uniforms = self.uniforms.buffer.as_ref();
        let storage = self.storage.buffer.as_ref();
        let entries: Vec<_> = bindings
            .as_slice()
            .iter()
            .map(|(binding, res)| wgpu::BindGroupEntry {
                binding: *binding,
                resource: match *res {
                    Res::Tex(tex) => wgpu::BindingResource::TextureView(
                        &self.entries[tex.0 as usize].as_ref().expect("texture").view,
                    ),
                    Res::Uniform { size, .. } => {
                        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: uniforms.expect("uniform buffer"),
                            offset: 0,
                            size: wgpu::BufferSize::new(u64::from(size)),
                        })
                    }
                    Res::Storage { offset, size } => {
                        wgpu::BindingResource::Buffer(wgpu::BufferBinding {
                            buffer: storage.expect("storage buffer"),
                            offset: u64::from(offset),
                            size: wgpu::BufferSize::new(u64::from(size)),
                        })
                    }
                    Res::Sampler(sampler) => {
                        wgpu::BindingResource::Sampler(&self.samplers[sampler.0 as usize].1)
                    }
                    Res::Buffer(buffer) => self.buffers[buffer.0 as usize]
                        .as_ref()
                        .expect("adopted buffer")
                        .as_entire_binding(),
                },
            })
            .collect();
        let group = self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: None,
            layout,
            entries: &entries,
        });
        let slot = match self.free_groups.pop() {
            Some(slot) => {
                self.groups[slot as usize] = Some((group, self.frame));
                slot
            }
            None => {
                self.groups.push(Some((group, self.frame)));
                self.groups.len() as u32 - 1
            }
        };
        self.group_keys.insert(key, slot);
        Ok(slot)
    }

    pub(super) fn purge_groups(&mut self, matches: impl Fn(&KeyRes) -> bool) {
        self.purge_group_keys(|key| {
            key.entries[..key.count as usize]
                .iter()
                .any(|(_, res)| matches(res))
        });
    }

    pub(super) fn purge_group_keys(&mut self, matches: impl Fn(&BindKey) -> bool) {
        let groups = &mut self.groups;
        let free = &mut self.free_groups;
        self.group_keys.retain(|key, group| {
            let purge = matches(key);
            if purge {
                groups[*group as usize] = None;
                free.push(*group);
            }
            !purge
        });
    }
}

fn dynamic_offsets(bindings: &PassBindings, offsets: &mut Vec<u32>) {
    offsets.clear();
    offsets.extend(bindings.as_slice().iter().filter_map(|(_, res)| match res {
        Res::Uniform { offset, .. } => Some(*offset),
        _ => None,
    }));
}
