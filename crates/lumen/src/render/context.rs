use crate::time::Rational;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FrameContext {
    pub frame: u64,
    pub time_seconds: f64,
    pub width: u32,
    pub height: u32,
    pub device_scale: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RendererContext {
    pub width: u32,
    pub height: u32,
    pub frame_rate: Rational,
}
