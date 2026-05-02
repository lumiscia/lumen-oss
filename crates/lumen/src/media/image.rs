use std::{fs, path::Path, sync::Arc};

use skia_safe::{Data, Image};

use crate::{error::MediaError, gpu_image::GpuImageFrame};

use super::{ImageMetadata, ImageResolver};

#[derive(Debug, Clone)]
struct CachedImage {
    metadata: ImageMetadata,
    frame: Arc<GpuImageFrame>,
}

#[derive(Debug, Clone)]
pub struct ImageFileResolver {
    id: String,
    cached: CachedImage,
}

impl ImageFileResolver {
    pub fn open(source: impl Into<String>) -> Result<Self, MediaError> {
        let source = source.into();
        let cached = load_cached_image(&source)?;
        Ok(Self { id: source, cached })
    }
}

impl ImageResolver for ImageFileResolver {
    fn id(&self) -> &str {
        &self.id
    }

    fn metadata(&self) -> ImageMetadata {
        self.cached.metadata
    }

    fn gpu_image(&self) -> Result<Arc<GpuImageFrame>, MediaError> {
        Ok(Arc::clone(&self.cached.frame))
    }
}

fn load_cached_image(source: &str) -> Result<CachedImage, MediaError> {
    let encoded = fs::read(Path::new(source)).map_err(|err| MediaError::Decode {
        media_source: source.to_string(),
        details: format!("failed opening image source: {err}"),
    })?;
    let data = Data::new_copy(encoded.as_slice());
    let image = Image::from_encoded(data).ok_or_else(|| MediaError::Decode {
        media_source: source.to_string(),
        details: "failed decoding image source with Skia".to_string(),
    })?;
    let metadata = ImageMetadata {
        width: image.width().max(0) as u32,
        height: image.height().max(0) as u32,
    };
    Ok(CachedImage {
        metadata,
        frame: Arc::new(GpuImageFrame::new(image)),
    })
}

#[cfg(test)]
mod tests {
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    use super::*;
    use ::image::{ImageBuffer, Rgba};

    #[test]
    fn image_file_resolver_caches_decoded_image_handles() {
        let path = temp_png_path();
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_pixel(2, 1, Rgba([128, 64, 32, 128]))
            .save(&path)
            .expect("write png fixture");

        let resolver = ImageFileResolver::open(path.to_string_lossy().to_string())
            .expect("open image resolver");
        let metadata = resolver.metadata();
        assert_eq!(metadata.width, 2);
        assert_eq!(metadata.height, 1);

        let first = resolver.gpu_image().expect("first resolve");
        let second = resolver.gpu_image().expect("second resolve");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.storage_width, 2);
        assert_eq!(first.storage_height, 1);

        let _ = fs::remove_file(path);
    }

    fn temp_png_path() -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system time")
            .as_nanos();
        std::env::temp_dir().join(format!("lumen-image-resolver-{unique}.png"))
    }
}
