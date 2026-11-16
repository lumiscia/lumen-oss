use std::sync::Arc;

use super::{
    FRAME_TEXTURE, FrameError, IDLE_FRAMES, KeyRes, MAX_DYNAMIC_PROGRAMS, MipChain, Recorder,
    Result, SWEEP_FRAMES, TexRef,
};
use crate::Size;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) struct PoolKey {
    width: u32,
    height: u32,
    format: wgpu::TextureFormat,
    usage: wgpu::TextureUsages,
}

pub(super) struct PoolEntry {
    pub(super) texture: Arc<wgpu::Texture>,
    pub(super) view: wgpu::TextureView,
    key: PoolKey,
    bytes: u64,
    last_used: u64,
    /// Holders of the texture outside the pool, which leave it out of rotation until released.
    retained: u32,
    /// Adopted from outside, never handed out by `acquire`.
    pub(super) external: bool,
}

/// Textures of one size and format, handed out in the same order every frame so that a stable
/// frame finds the same textures in the same slots, and its bind groups with them.
#[derive(Default)]
pub(super) struct PoolBucket {
    textures: Vec<TexRef>,
    /// The first `next` of `textures` were handed out this frame.
    pub(super) next: usize,
    /// Those whose frame texture has had its last use, most recently freed last.
    pub(super) released: Vec<TexRef>,
}

/// A texture of the frame being recorded.
pub(super) struct FrameTexture {
    key: PoolKey,
    physical: Option<TexRef>,
    /// Holds taken with `retain` before it had a pooled texture.
    retains: u32,
    /// Written outside the passes, before they run, so it cannot share with an earlier pass.
    written_early: bool,
    /// The last pass using it.
    last_use: usize,
    /// Its pooled texture is back in the bucket.
    released: bool,
}

impl Recorder {
    /// A texture of this size, format and usage for this frame's passes, valid in them until
    /// `resolve`. Its contents are undefined, possibly another intermediate's pixels: whoever
    /// writes it must write everything later read, or `clear` it first.
    pub fn acquire(
        &mut self,
        size: Size,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> TexRef {
        if size.width.max(size.height) > self.largest_texture.width.max(self.largest_texture.height)
        {
            self.largest_texture = size;
        }
        self.frame_textures.push(FrameTexture {
            key: PoolKey {
                width: size.width.max(1),
                height: size.height.max(1),
                format,
                usage,
            },
            physical: None,
            retains: 0,
            written_early: false,
            last_use: 0,
            released: false,
        });
        TexRef(FRAME_TEXTURE | (self.frame_textures.len() as u32 - 1))
    }

    /// Sets the widest or tallest texture acquired so far to `size` and returns what it was.
    /// `begin_frame` resets it to zero by zero.
    pub fn replace_largest_acquired_texture(&mut self, size: Size) -> Size {
        std::mem::replace(&mut self.largest_texture, size)
    }

    /// `acquire` for a texture written between `resolve` and the passes running.
    pub fn acquire_written_early(
        &mut self,
        size: Size,
        format: wgpu::TextureFormat,
        usage: wgpu::TextureUsages,
    ) -> TexRef {
        let tex = self.acquire(size, format, usage);
        self.frame_textures
            .last_mut()
            .expect("frame texture")
            .written_early = true;
        tex
    }

    /// A pooled texture this frame hasn't handed out, or one no later pass reads.
    fn take_pooled(&mut self, key: PoolKey) -> TexRef {
        let frame = self.frame;
        let bucket = self.buckets.entry(key).or_default();
        if let Some(tex) = bucket.released.pop() {
            return tex;
        }
        if let Some(&tex) = bucket.textures.get(bucket.next) {
            bucket.next += 1;
            self.entries[tex.0 as usize]
                .as_mut()
                .expect("pooled texture")
                .last_used = frame;
            return tex;
        }
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("lumen frame graph target"),
            size: wgpu::Extent3d {
                width: key.width,
                height: key.height,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: key.format,
            usage: key.usage,
            view_formats: &[],
        });
        self.stats.textures_created += 1;
        let tex = self.insert(Arc::new(texture), key, false);
        let bucket = self.buckets.get_mut(&key).expect("bucket");
        bucket.textures.push(tex);
        bucket.next = bucket.textures.len();
        tex
    }

    /// Gives each of this frame's textures a pooled texture for the passes using it, and rewrites
    /// the passes to them. `output` stays intact after the frame. When `external` can stand in for
    /// it, its passes write `external` directly; otherwise a last pass copies it there.
    pub fn resolve(&mut self, output: TexRef, external: Option<TexRef>) -> Result<TexRef> {
        let output_frame = output.frame_index();
        if let Some(external) = external {
            let target = self.entry(external);
            let (target_key, target_usage) = (target.key, target.key.usage);
            let output_key = match output_frame {
                Some(index) => self.frame_textures[index].key,
                None => self.entry(output).key,
            };
            if (target_key.width, target_key.height) != (output_key.width, output_key.height) {
                return Err(FrameError::Misuse(format!(
                    "the external texture has size {}x{}, expected {}x{}",
                    target_key.width, target_key.height, output_key.width, output_key.height
                )));
            }
            if target_key.format != output_key.format {
                return Err(FrameError::Misuse(format!(
                    "the external texture has format {:?}, expected {:?}",
                    target_key.format, output_key.format
                )));
            }
            let stands_in = output_frame.is_some_and(|index| {
                let texture = &self.frame_textures[index];
                texture.retains == 0
                    && !texture.written_early
                    && (texture.key.width, texture.key.height)
                        == (target_key.width, target_key.height)
                    && texture.key.format == target_key.format
                    && target_usage.contains(texture.key.usage)
            });
            match output_frame {
                Some(index) if stands_in => self.frame_textures[index].physical = Some(external),
                _ => {
                    if !target_usage.contains(wgpu::TextureUsages::COPY_DST) {
                        return Err(FrameError::Misuse(format!(
                            "the external texture can't take a kept output: it has usage {target_usage:?}, without COPY_DST"
                        )));
                    }
                    let size = Size::new(target_key.width, target_key.height);
                    self.copy(output, external, [0, 0], size);
                    self.stats.external_copies += 1;
                }
            }
        }

        let mut passes = std::mem::take(&mut self.passes);
        for (index, pass) in passes.iter_mut().enumerate() {
            let textures = &mut self.frame_textures;
            pass.textures_mut(|tex| {
                if let Some(frame) = tex.frame_index() {
                    textures[frame].last_use = index;
                }
            });
        }
        let mut live_bytes = 0;
        let mut peak_bytes = 0;
        let mut unshared_bytes = 0;
        for index in 0..self.frame_textures.len() {
            if self.frame_textures[index].written_early {
                self.assign(index, &mut live_bytes, &mut peak_bytes, &mut unshared_bytes);
            }
        }
        let mut ended = Vec::new();
        for (index, pass) in passes.iter_mut().enumerate() {
            ended.clear();
            pass.textures_mut(|tex| {
                let Some(frame) = tex.frame_index() else {
                    return;
                };
                self.assign(frame, &mut live_bytes, &mut peak_bytes, &mut unshared_bytes);
                *tex = self.frame_textures[frame].physical.expect("assigned");
                if self.frame_textures[frame].last_use == index {
                    ended.push(frame);
                }
            });
            for frame in ended.drain(..) {
                let texture = &mut self.frame_textures[frame];
                if Some(frame) == output_frame || texture.retains > 0 || texture.released {
                    continue;
                }
                texture.released = true;
                let (key, tex) = (texture.key, texture.physical.expect("assigned"));
                live_bytes -= pool_entry_bytes(key);
                self.buckets
                    .get_mut(&key)
                    .expect("bucket")
                    .released
                    .push(tex);
            }
        }
        self.passes = passes;
        // What no pass uses still has to exist when something reads it after the frame.
        for index in 0..self.frame_textures.len() {
            if Some(index) == output_frame || self.frame_textures[index].retains > 0 {
                self.assign(index, &mut live_bytes, &mut peak_bytes, &mut unshared_bytes);
            }
        }
        for index in 0..self.frame_textures.len() {
            let texture = &self.frame_textures[index];
            let (retains, physical) = (texture.retains, texture.physical);
            if let Some(physical) = physical {
                for _ in 0..retains {
                    self.retain(physical);
                }
            }
        }
        self.stats.peak_frame_bytes = self.stats.peak_frame_bytes.max(peak_bytes);
        self.stats.peak_unshared_bytes = self.stats.peak_unshared_bytes.max(unshared_bytes);
        Ok(self.physical(output))
    }

    fn assign(
        &mut self,
        frame: usize,
        live_bytes: &mut u64,
        peak_bytes: &mut u64,
        unshared_bytes: &mut u64,
    ) {
        if self.frame_textures[frame].physical.is_some() {
            return;
        }
        let key = self.frame_textures[frame].key;
        let tex = self.take_pooled(key);
        self.frame_textures[frame].physical = Some(tex);
        let bytes = pool_entry_bytes(key);
        *live_bytes += bytes;
        *peak_bytes = (*peak_bytes).max(*live_bytes);
        *unshared_bytes += bytes;
    }

    /// The pooled or adopted texture `tex` names once the frame is resolved.
    pub fn physical(&self, tex: TexRef) -> TexRef {
        match tex.frame_index() {
            Some(index) => self.frame_textures[index]
                .physical
                .expect("a frame texture nothing used or kept"),
            None => tex,
        }
    }

    /// Registers a texture made elsewhere, such as an atlas or image, for binding.
    pub fn adopt(&mut self, texture: Arc<wgpu::Texture>) -> TexRef {
        let key = PoolKey {
            width: texture.width(),
            height: texture.height(),
            format: texture.format(),
            usage: texture.usage(),
        };
        self.insert(texture, key, true)
    }

    fn insert(&mut self, texture: Arc<wgpu::Texture>, key: PoolKey, external: bool) -> TexRef {
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        self.insert_view(texture, view, key, pool_entry_bytes(key), external)
    }

    fn insert_view(
        &mut self,
        texture: Arc<wgpu::Texture>,
        view: wgpu::TextureView,
        key: PoolKey,
        bytes: u64,
        external: bool,
    ) -> TexRef {
        if external {
            self.adopted_bytes += bytes;
        } else {
            self.pool_bytes += bytes;
        }
        let entry = PoolEntry {
            texture,
            view,
            key,
            bytes,
            last_used: self.frame,
            retained: 0,
            external,
        };
        match self.free_slots.pop() {
            Some(slot) => {
                self.entries[slot as usize] = Some(entry);
                TexRef(slot)
            }
            None => {
                self.entries.push(Some(entry));
                TexRef(self.entries.len() as u32 - 1)
            }
        }
    }

    /// Registers a texture with a mip chain made elsewhere, both whole, to sample every level,
    /// and as a texture per level, to draw into or sample alone.
    pub fn adopt_mips(&mut self, texture: Arc<wgpu::Texture>) -> MipChain {
        let usage = texture.usage();
        let format = texture.format();
        let levels = (0..texture.mip_level_count())
            .map(|level| {
                let key = PoolKey {
                    width: (texture.width() >> level).max(1),
                    height: (texture.height() >> level).max(1),
                    format,
                    usage,
                };
                let view = texture.create_view(&wgpu::TextureViewDescriptor {
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..wgpu::TextureViewDescriptor::default()
                });
                self.insert_view(Arc::clone(&texture), view, key, 0, true)
            })
            .collect();
        let key = PoolKey {
            width: texture.width(),
            height: texture.height(),
            format,
            usage,
        };
        let bytes = (0..texture.mip_level_count())
            .map(|level| {
                format.theoretical_memory_footprint(wgpu::Extent3d {
                    width: (key.width >> level).max(1),
                    height: (key.height >> level).max(1),
                    depth_or_array_layers: 1,
                })
            })
            .sum();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        let sampled = self.insert_view(texture, view, key, bytes, true);
        MipChain { sampled, levels }
    }

    /// Lets go of a chain from `adopt_mips`.
    pub fn forget_mips(&mut self, chain: &MipChain) {
        for &tex in chain.levels.iter().chain([&chain.sampled]) {
            self.forget(tex);
        }
    }

    /// Takes a pooled texture out of the pool without destroying it, so whoever holds it keeps its
    /// pixels. Does nothing to an adopted texture.
    pub fn disown(&mut self, tex: TexRef) {
        let Some(entry) = &self.entries[tex.0 as usize] else {
            return;
        };
        if entry.external {
            return;
        }
        let (key, bytes) = (entry.key, entry.bytes);
        if let Some(bucket) = self.buckets.get_mut(&key) {
            bucket.textures.retain(|other| *other != tex);
            bucket.released.retain(|other| *other != tex);
            bucket.next = bucket.next.min(bucket.textures.len());
        }
        self.pool_bytes -= bytes;
        self.entries[tex.0 as usize] = None;
        self.free_slots.push(tex.0);
        self.purge_groups(|res| matches!(res, KeyRes::Tex(other) if *other == tex));
    }

    /// Forgets a texture from `adopt`, along with the bind groups using it.
    pub fn forget(&mut self, tex: TexRef) {
        self.remove_entry(tex);
        self.purge_groups(|res| matches!(res, KeyRes::Tex(other) if *other == tex));
    }

    fn remove_entry(&mut self, tex: TexRef) {
        if let Some(entry) = self.entries[tex.0 as usize].take() {
            self.free_slots.push(tex.0);
            if entry.external {
                self.adopted_bytes -= entry.bytes;
            } else {
                self.pool_bytes -= entry.bytes;
                self.stats.textures_destroyed += 1;
                entry.texture.destroy();
            }
        }
    }

    /// Takes the texture out of rotation until as many `release` calls follow, so its contents
    /// survive later frames.
    pub fn retain(&mut self, tex: TexRef) {
        if let Some(index) = tex.frame_index() {
            let texture = &mut self.frame_textures[index];
            if texture.physical.is_none() {
                texture.retains += 1;
                return;
            }
        }
        let tex = self.physical(tex);
        let entry = self.entries[tex.0 as usize].as_mut().expect("texture");
        entry.retained += 1;
        let (first, external, key) = (entry.retained == 1, entry.external, entry.key);
        if first
            && !external
            && let Some(bucket) = self.buckets.get_mut(&key)
            && let Some(index) = bucket.textures.iter().position(|other| *other == tex)
        {
            bucket.textures.remove(index);
            if index < bucket.next {
                bucket.next -= 1;
            }
        }
    }

    pub fn release(&mut self, tex: TexRef) {
        if let Some(index) = tex.frame_index() {
            let texture = &mut self.frame_textures[index];
            if texture.physical.is_none() {
                texture.retains -= 1;
                return;
            }
        }
        let tex = self.physical(tex);
        let entry = self.entries[tex.0 as usize].as_mut().expect("texture");
        entry.retained -= 1;
        entry.last_used = self.frame;
        if entry.retained == 0 && !entry.external {
            let key = entry.key;
            self.buckets.entry(key).or_default().textures.push(tex);
        }
    }

    /// A resolved texture.
    pub fn texture(&self, tex: TexRef) -> &Arc<wgpu::Texture> {
        &self.entry(self.physical(tex)).texture
    }

    pub fn adopted_bytes(&self) -> u64 {
        self.adopted_bytes
    }

    pub fn texture_bytes(&self, tex: TexRef) -> u64 {
        match tex.frame_index() {
            Some(index) => pool_entry_bytes(self.frame_textures[index].key),
            None => self.entry(tex).bytes,
        }
    }

    pub fn format(&self, tex: TexRef) -> wgpu::TextureFormat {
        match tex.frame_index() {
            Some(index) => self.frame_textures[index].key.format,
            None => self.entry(tex).key.format,
        }
    }

    pub fn usage(&self, tex: TexRef) -> wgpu::TextureUsages {
        match tex.frame_index() {
            Some(index) => self.frame_textures[index].key.usage,
            None => self.entry(tex).key.usage,
        }
    }

    fn entry(&self, tex: TexRef) -> &PoolEntry {
        self.entries[tex.0 as usize].as_ref().expect("texture")
    }

    /// Ends the frame, dropping textures and bind groups idle past their limit and, when the
    /// textures this frame left unused are over budget, those used least recently.
    pub fn end_frame(&mut self) {
        self.evict_dynamic_programs(MAX_DYNAMIC_PROGRAMS);
        // Retained textures are out of rotation, and budgeted by whoever retains them.
        let mut idle: u64 = self
            .buckets
            .iter()
            .map(|(key, bucket)| {
                (bucket.textures.len() - bucket.next) as u64 * pool_entry_bytes(*key)
            })
            .sum();
        let over_budget = idle > self.budget;
        if !over_budget && !self.frame.is_multiple_of(SWEEP_FRAMES) {
            return;
        }
        let frame = self.frame;
        let idle_before = frame.saturating_sub(IDLE_FRAMES);
        let mut removed = foldhash::HashSet::default();
        let mut surplus: Vec<(u64, PoolKey)> = Vec::new();
        let entries = &self.entries;
        for (key, bucket) in &mut self.buckets {
            while bucket.textures.len() > bucket.next {
                let tail = *bucket.textures.last().expect("tail");
                let last_used = entries[tail.0 as usize]
                    .as_ref()
                    .expect("texture")
                    .last_used;
                if last_used > idle_before {
                    surplus.push((last_used, *key));
                    break;
                }
                bucket.textures.pop();
                removed.insert(tail);
                idle -= pool_entry_bytes(*key);
            }
        }
        for tex in removed.clone() {
            self.remove_entry(tex);
        }
        // Over budget: textures not used this frame go oldest first.
        surplus.sort_unstable_by_key(|(last_used, _)| *last_used);
        let mut surplus = surplus.into_iter();
        while idle > self.budget {
            let Some((_, key)) = surplus.next() else {
                break;
            };
            while idle > self.budget {
                let bucket = self.buckets.get_mut(&key).expect("bucket");
                if bucket.textures.len() <= bucket.next {
                    break;
                }
                let tail = bucket.textures.pop().expect("tail");
                removed.insert(tail);
                idle -= pool_entry_bytes(key);
                self.remove_entry(tail);
            }
        }
        self.buckets.retain(|_, bucket| !bucket.textures.is_empty());
        if !removed.is_empty() {
            self.purge_groups(|res| matches!(res, KeyRes::Tex(tex) if removed.contains(tex)));
        }
        let idle_groups = frame.saturating_sub(IDLE_FRAMES);
        let groups = &mut self.groups;
        let free = &mut self.free_groups;
        self.group_keys.retain(|_, group| {
            let keep = groups[*group as usize]
                .as_ref()
                .is_some_and(|g| g.1 > idle_groups);
            if !keep {
                groups[*group as usize] = None;
                free.push(*group);
            }
            keep
        });
    }
}

fn pool_entry_bytes(key: PoolKey) -> u64 {
    key.format.theoretical_memory_footprint(wgpu::Extent3d {
        width: key.width,
        height: key.height,
        depth_or_array_layers: 1,
    })
}
