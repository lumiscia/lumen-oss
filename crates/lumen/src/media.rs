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
    fn get_image_resolver(&self, id: String) -> Option<impl ImageResolver>;

    fn get_video_resolver(&self, id: String) -> Option<impl VideoResolver>;
}

// TODO: audio
