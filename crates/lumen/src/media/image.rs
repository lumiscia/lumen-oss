use std::{fs, path::Path, sync::Arc};

use crate::error::MediaError;

use super::{CpuMediaFrame, ImageMetadata, ImageResolver, MediaFrame};

#[derive(Debug, Clone)]
struct CachedImage {
    metadata: ImageMetadata,
    frame: Arc<CpuMediaFrame>,
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

    fn frame(&self) -> Result<MediaFrame, MediaError> {
        Ok(MediaFrame::CpuRgba(Arc::clone(&self.cached.frame)))
    }
}

fn load_cached_image(source: &str) -> Result<CachedImage, MediaError> {
    let encoded = fs::read(Path::new(source)).map_err(|err| MediaError::Decode {
        media_source: source.to_string(),
        details: format!("failed opening image source: {err}"),
    })?;
    let image = image::load_from_memory(&encoded).map_err(|err| MediaError::Decode {
        media_source: source.to_string(),
        details: format!("failed decoding image source: {err}"),
    })?;
    let rgba = image.to_rgba8();
    let metadata = ImageMetadata {
        width: rgba.width(),
        height: rgba.height(),
    };
    Ok(CachedImage {
        metadata,
        frame: Arc::new(CpuMediaFrame {
            row_bytes: metadata.width as usize * 4,
            width: metadata.width,
            height: metadata.height,
            rgba: Arc::new(rgba.into_raw()),
        }),
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

        let MediaFrame::CpuRgba(first) = resolver.frame().expect("first resolve") else {
            panic!("expected CPU frame");
        };
        let MediaFrame::CpuRgba(second) = resolver.frame().expect("second resolve") else {
            panic!("expected CPU frame");
        };
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.width, 2);
        assert_eq!(first.height, 1);

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
