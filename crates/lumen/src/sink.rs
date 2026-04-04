//! Output sink traits and in-memory bitmap sink for testing.

use crate::{error::SinkError, raster::RasterFrame};

pub trait Sink: Send {
    // Frames arrive sorted
    fn write_frame(&mut self, frame: u32, data: &RasterFrame) -> Result<(), SinkError>;
    fn finalize(&mut self) -> Result<(), SinkError>;
}
