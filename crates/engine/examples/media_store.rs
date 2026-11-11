//! A minimal `MediaStore`: images a host hands over in memory, each with the identity it was set
//! with. Run with `cargo run -p lumen-engine --example media_store`.

use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use lumen_engine::error::MediaError;
use lumen_engine::media::{
    AssetKind, CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame, MediaIdentity,
    MediaResolver, MediaStore, ResolvedMedia, VideoFrameResolver, resolve_as,
};

#[derive(Debug, Default)]
struct MemoryStore {
    images: RwLock<HashMap<String, (MediaIdentity, Arc<CpuMediaFrame>)>>,
}

impl MemoryStore {
    fn set_image(&self, source: &str, frame: CpuMediaFrame) {
        // New pixels get a new identity, so the engine stops drawing what it kept of the old ones.
        let image = (MediaIdentity::unique(), Arc::new(frame));
        self.images
            .write()
            .unwrap()
            .insert(source.to_string(), image);
    }
}

struct MemoryImage(Arc<CpuMediaFrame>);

impl ImageResolver for MemoryImage {
    fn metadata(&self) -> ImageMetadata {
        ImageMetadata {
            width: self.0.width,
            height: self.0.height,
        }
    }

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.0)))
    }
}

impl MediaStore for MemoryStore {
    // This store holds only images, so it serves one whatever kind is asked for, and the engine
    // reports a use that reads it as another kind as `media.wrong_kind`.
    fn resolve(&self, source: &str, _kind: AssetKind) -> Result<ResolvedMedia, MediaError> {
        let images = self.images.read().unwrap();
        let (identity, frame) = images
            .get(source)
            .ok_or_else(|| MediaError::source_not_found(source, "no image was set"))?;
        Ok(ResolvedMedia::new(
            identity.clone(),
            MediaResolver::Image(Box::new(MemoryImage(Arc::clone(frame)))),
        ))
    }
}

fn pixel(rgba: [u8; 4]) -> CpuMediaFrame {
    CpuMediaFrame {
        rgba: Arc::new(rgba.to_vec()),
        width: 1,
        height: 1,
        row_bytes: 4,
    }
}

fn main() {
    let store = MemoryStore::default();
    store.set_image("logo", pixel([255, 0, 0, 255]));
    let (red, image) = resolve_as::<dyn ImageResolver>(&store, "logo").unwrap();
    assert_eq!(image.metadata().width, 1);

    store.set_image("logo", pixel([0, 0, 255, 255]));
    let (blue, _) = resolve_as::<dyn ImageResolver>(&store, "logo").unwrap();
    assert_ne!(red, blue);

    let Err(error) = resolve_as::<dyn VideoFrameResolver>(&store, "logo") else {
        panic!("an image is not a video");
    };
    assert_eq!(error.to_string(), "`logo` is an image, not a video");
    println!("{error}");
}
