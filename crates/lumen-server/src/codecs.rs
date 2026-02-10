use ac_ffmpeg::codec::CodecTag;

pub struct Codecs {
    h264: CodecTag,
}

impl Codecs {
    pub fn new(h264: CodecTag) -> Self {
        Self { h264 }
    }
}
