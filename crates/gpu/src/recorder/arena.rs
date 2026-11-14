use super::{BUMP_ALIGNMENT, FrameError, KeyRes, Recorder, Res, Result};

pub(super) struct Arena {
    label: &'static str,
    usage: wgpu::BufferUsages,
    pub(super) cpu: Vec<u8>,
    pub(super) buffer: Option<wgpu::Buffer>,
    pub(super) capacity: u64,
    /// The largest range one binding may cover.
    max_binding: u64,
    /// The largest the buffer may grow.
    max_buffer: u64,
    /// The frame asked for more than the device takes, so it must not be uploaded.
    pub(super) overflowed: bool,
}

impl Arena {
    pub(super) fn new(
        label: &'static str,
        usage: wgpu::BufferUsages,
        max_binding: u64,
        max_buffer: u64,
    ) -> Self {
        Self {
            label,
            usage,
            cpu: Vec::new(),
            buffer: None,
            capacity: 0,
            max_binding,
            // Offsets are `u32`.
            max_buffer: max_buffer.min(1 << 31),
            overflowed: false,
        }
    }

    /// A zeroed range of `size` bytes, or `None`, marking the frame as too large, when the
    /// device can't bind that much or the buffer can't grow to hold it.
    fn alloc(&mut self, size: u64) -> Option<(u32, &mut [u8])> {
        let offset = (self.cpu.len() as u64).next_multiple_of(u64::from(BUMP_ALIGNMENT));
        if size > self.max_binding || Self::capacity_for(offset + size) > self.max_buffer {
            self.overflowed = true;
            return None;
        }
        self.cpu.resize((offset + size) as usize, 0);
        Some((offset as u32, &mut self.cpu[offset as usize..]))
    }

    /// The buffer size that holds `bytes` of frame data.
    fn capacity_for(bytes: u64) -> u64 {
        bytes.next_power_of_two().max(1 << 16)
    }

    /// Writes this frame's data, growing the buffer first when needed. Returns whether it grew,
    /// which invalidates bind groups on the old one.
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue) -> bool {
        let needed = (self.cpu.len() as u64).max(u64::from(BUMP_ALIGNMENT));
        let grew = needed > self.capacity;
        if grew {
            self.capacity = Self::capacity_for(needed);
            self.buffer = Some(device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(self.label),
                size: self.capacity,
                usage: self.usage | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }));
        }
        if !self.cpu.is_empty() {
            queue.write_buffer(self.buffer.as_ref().expect("arena buffer"), 0, &self.cpu);
        }
        grew
    }

    pub(super) fn release(&mut self) {
        if let Some(buffer) = self.buffer.take() {
            buffer.destroy();
        }
        self.cpu = Vec::new();
        self.capacity = 0;
    }
}

impl Recorder {
    /// Places `data` in the uniform bump buffer.
    pub fn uniform_bytes(&mut self, data: &[u8]) -> Res {
        let size = (data.len() as u64).max(16);
        let Some((offset, slot)) = self.uniforms.alloc(size) else {
            return Res::Uniform {
                offset: 0,
                size: 16,
            };
        };
        slot[..data.len()].copy_from_slice(data);
        self.stats.uniform_bytes += size;
        Res::Uniform {
            offset,
            size: size as u32,
        }
    }

    pub fn uniform<T: bytemuck::Pod>(&mut self, value: &T) -> Res {
        self.uniform_bytes(bytemuck::bytes_of(value))
    }

    /// Places `parts` one after another in the storage bump buffer. The range is rounded up to a
    /// power of two, so sizes moving within a class keep the offsets of everything after them,
    /// and the bind groups cached by those offsets. A range the device can't take makes
    /// [`Self::upload`] fail for this frame.
    pub fn storage_parts(&mut self, parts: &[&[u8]]) -> Res {
        let total: usize = parts.iter().map(|part| part.len()).sum();
        let size = (total as u64)
            .max(u64::from(BUMP_ALIGNMENT))
            .next_power_of_two();
        let Some((offset, slot)) = self.storage.alloc(size) else {
            return Res::Storage {
                offset: 0,
                size: BUMP_ALIGNMENT,
            };
        };
        let mut at = 0;
        for part in parts {
            slot[at..at + part.len()].copy_from_slice(part);
            at += part.len();
        }
        self.stats.storage_bytes += size;
        Res::Storage {
            offset,
            size: size as u32,
        }
    }

    /// Holds the storage bump buffer to `max_binding` bytes a binding and `max_buffer` bytes in
    /// all, where the device allows more.
    pub fn limit_storage(&mut self, max_binding: u64, max_buffer: u64) {
        self.storage.max_binding = self.storage.max_binding.min(max_binding);
        self.storage.max_buffer = self.storage.max_buffer.min(max_buffer);
    }

    /// Writes this frame's bump buffers, rebuilding bind groups on any that had to grow. Fails,
    /// writing nothing, when the frame recorded more data than the device's buffers hold.
    pub fn upload(&mut self) -> Result<()> {
        for arena in [&self.uniforms, &self.storage] {
            if arena.overflowed {
                return Err(FrameError::TooLarge(format!(
                    "the frame's data doesn't fit the GPU: {} can bind at most {} bytes at once \
                     and hold {} bytes in all",
                    arena.label, arena.max_binding, arena.max_buffer
                )));
            }
        }
        let uniforms_grew = self.uniforms.upload(&self.device, &self.queue);
        let storage_grew = self.storage.upload(&self.device, &self.queue);
        if uniforms_grew || storage_grew {
            self.purge_group_keys(|key| {
                key.entries[..key.count as usize].iter().any(|(_, res)| {
                    (uniforms_grew && matches!(res, KeyRes::Uniform { .. }))
                        || (storage_grew && matches!(res, KeyRes::Storage { .. }))
                })
            });
        }
        Ok(())
    }
}
