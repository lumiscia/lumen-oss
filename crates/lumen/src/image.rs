use std::{path::Path, sync::Arc};

use ::image::ImageReader;

use crate::{
    error::MediaError,
    media::{ImageMetadata, ImageResolver, premultiply_rgba_in_place_if_needed},
};

#[derive(Debug, Clone)]
struct CachedImage {
    metadata: ImageMetadata,
    pixels: Arc<Vec<u8>>,
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

    fn resolve(&self) -> Result<Arc<Vec<u8>>, MediaError> {
        Ok(Arc::clone(&self.cached.pixels))
    }
}

fn load_cached_image(source: &str) -> Result<CachedImage, MediaError> {
    let reader = ImageReader::open(Path::new(source)).map_err(|err| MediaError::Decode {
        media_source: source.to_string(),
        details: format!("failed opening image source: {err}"),
    })?;
    let image = reader
        .with_guessed_format()
        .map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed determining image format: {err}"),
        })?
        .decode()
        .map_err(|err| MediaError::Decode {
            media_source: source.to_string(),
            details: format!("failed decoding image source: {err}"),
        })?;
    let mut rgba = image.to_rgba8().into_raw();
    premultiply_rgba_in_place_if_needed(&mut rgba);
    let metadata = ImageMetadata {
        width: image.width(),
        height: image.height(),
    };
    Ok(CachedImage {
        metadata,
        pixels: Arc::new(rgba),
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
    fn image_file_resolver_caches_decoded_pixels() {
        let path = temp_png_path();
        ImageBuffer::<Rgba<u8>, Vec<u8>>::from_pixel(2, 1, Rgba([128, 64, 32, 128]))
            .save(&path)
            .expect("write png fixture");

        let resolver = ImageFileResolver::open(path.to_string_lossy().to_string())
            .expect("open image resolver");
        let metadata = resolver.metadata();
        assert_eq!(metadata.width, 2);
        assert_eq!(metadata.height, 1);

        let first = resolver.resolve().expect("first resolve");
        let second = resolver.resolve().expect("second resolve");
        assert!(Arc::ptr_eq(&first, &second));
        assert_eq!(first.as_slice(), &[64, 32, 16, 128, 64, 32, 16, 128]);

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
