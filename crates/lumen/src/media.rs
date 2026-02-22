pub trait ImageResolver {
    fn id(&self) -> String;

    fn width(&self) -> u32;

    fn height(&self) -> u32;

    fn resolve(&mut self) -> Vec<u8>;
}

pub trait VideoResolver {
    fn id(&self) -> String;

    fn width(&self) -> u32;

    fn height(&self) -> u32;

    fn resolve_frame(&mut self, frame: u32) -> Vec<u8>;
}

pub trait MediaStore {
    fn get_image_resolver(&mut self, id: &str) -> Option<Box<dyn ImageResolver>>;

    fn get_video_resolver(&mut self, id: &str) -> Option<Box<dyn VideoResolver>>;
}

// TODO: audio
