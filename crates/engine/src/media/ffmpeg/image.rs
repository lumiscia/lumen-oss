use std::collections::{HashMap, HashSet, VecDeque};

use crate::media::MediaFrame;

/// Limits for one video source's cache. Every source has its own, so a job's ceiling is these times
/// its number of video sources. Evicted frames are only decoded again, so the limits never change
/// output. The byte limit holds the server's 30-frame lookahead at 1080p and 7 frames at 4K.
const MAX_CACHED_FRAMES: usize = 64;
const MAX_CACHED_BYTES: usize = 256 * 1024 * 1024;

struct CachedFrame {
    data: MediaFrame,
    bytes: usize,
    speculative: bool,
}

#[derive(Default)]
pub(super) struct FrameLruCache {
    entries: HashMap<u32, CachedFrame>,
    /// Least recently used first.
    order: VecDeque<u32>,
}

impl FrameLruCache {
    pub fn get(&mut self, frame: u32) -> Option<MediaFrame> {
        let entry = self.entries.get_mut(&frame)?;
        entry.speculative = false;
        let data = entry.data.clone();
        self.order.retain(|cached| *cached != frame);
        self.order.push_back(frame);
        Some(data)
    }

    pub fn contains(&self, frame: u32) -> bool {
        self.entries.contains_key(&frame)
    }

    /// Whether a frame of `bytes` fits without evicting anything.
    pub fn has_room_for(&self, bytes: usize) -> bool {
        self.order.len() < MAX_CACHED_FRAMES && self.cached_bytes() + bytes <= MAX_CACHED_BYTES
    }

    /// Caches a frame, evicting to stay within the limits. Speculative frames go first, newest
    /// first because those are furthest ahead of playback, then the least recently used. A frame
    /// that was asked for always stays, even if it alone is over the byte limit.
    pub fn insert(&mut self, frame: u32, data: MediaFrame, speculative: bool) {
        let bytes = match &data {
            MediaFrame::CpuRgba(cpu) => cpu.rgba.len(),
            MediaFrame::GpuVideo(gpu) => gpu.frame.estimated_rgba_bytes() as usize,
        };
        self.order.retain(|cached| *cached != frame);
        self.order.push_back(frame);
        self.entries.insert(
            frame,
            CachedFrame {
                data,
                bytes,
                speculative,
            },
        );

        while self.order.len() > 1
            && (self.order.len() > MAX_CACHED_FRAMES || self.cached_bytes() > MAX_CACHED_BYTES)
        {
            let victim = self
                .order
                .iter()
                .rposition(|cached| self.entries[cached].speculative)
                .unwrap_or(0);
            if let Some(evicted) = self.order.remove(victim) {
                self.entries.remove(&evicted);
            }
        }
    }

    pub fn retain(&mut self, frames: &[u32]) {
        let keep: HashSet<_> = frames.iter().copied().collect();
        self.entries.retain(|frame, _| keep.contains(frame));
        self.order.retain(|frame| keep.contains(frame));
    }

    fn cached_bytes(&self) -> usize {
        self.entries.values().map(|entry| entry.bytes).sum()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::media::CpuMediaFrame;

    const FRAME_BYTES: usize = 32 * 1024 * 1024;

    fn frame(bytes: usize) -> MediaFrame {
        MediaFrame::CpuRgba(Arc::new(CpuMediaFrame {
            rgba: Arc::new(vec![0; bytes]),
            width: 1,
            height: 1,
            row_bytes: 4,
        }))
    }

    fn cached_frames(cache: &FrameLruCache) -> Vec<u32> {
        cache.order.iter().copied().collect()
    }

    #[test]
    fn stays_within_the_frame_and_byte_limits() {
        let mut cache = FrameLruCache::default();
        for index in 0..100 {
            cache.insert(index, frame(4), false);
        }
        assert_eq!(cache.order.len(), MAX_CACHED_FRAMES);
        assert!(cache.contains(99) && !cache.contains(35));

        let mut cache = FrameLruCache::default();
        for index in 0..20 {
            cache.insert(index, frame(FRAME_BYTES), false);
        }
        assert_eq!(cached_frames(&cache), (12..20).collect::<Vec<_>>());
        assert!(!cache.has_room_for(FRAME_BYTES));
    }

    #[test]
    fn keeps_a_requested_frame_larger_than_the_byte_limit() {
        let mut cache = FrameLruCache::default();
        cache.insert(1, frame(FRAME_BYTES), false);
        cache.insert(2, frame(MAX_CACHED_BYTES + 1), false);
        assert_eq!(cached_frames(&cache), [2]);
    }

    #[test]
    fn eviction_order_survives_retain_and_reinsert() {
        let mut cache = FrameLruCache::default();
        for index in 0..8 {
            cache.insert(index, frame(FRAME_BYTES), false);
        }
        cache.retain(&[0, 1, 2, 3, 4, 5, 6]);
        cache.insert(0, frame(FRAME_BYTES), false);
        cache.insert(7, frame(FRAME_BYTES), false);
        cache.insert(8, frame(FRAME_BYTES), false);
        assert_eq!(cached_frames(&cache), [2, 3, 4, 5, 6, 0, 7, 8]);
        assert_eq!(cache.cached_bytes(), 8 * FRAME_BYTES);
    }

    #[test]
    fn requested_frames_displace_lookahead_and_not_the_reverse() {
        let mut cache = FrameLruCache::default();
        cache.insert(10, frame(FRAME_BYTES), false);
        for index in 11..18 {
            cache.insert(index, frame(FRAME_BYTES), true);
        }
        assert!(!cache.has_room_for(FRAME_BYTES));

        cache.insert(30, frame(FRAME_BYTES), false);
        assert!(cache.contains(10) && cache.contains(30) && !cache.contains(17));

        cache.insert(40, frame(FRAME_BYTES), true);
        assert!(cache.contains(10) && cache.contains(30) && !cache.contains(40));

        assert!(cache.get(11).is_some());
        cache.insert(50, frame(FRAME_BYTES), false);
        assert!(cache.contains(11) && !cache.contains(16));
    }
}
