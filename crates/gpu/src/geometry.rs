#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Size {
    pub width: u32,
    pub height: u32,
}

impl Size {
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    pub fn as_extent(self) -> wgpu::Extent3d {
        wgpu::Extent3d {
            width: self.width.max(1),
            height: self.height.max(1),
            depth_or_array_layers: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct RectI {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl RectI {
    pub const fn new(x: i32, y: i32, width: u32, height: u32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }

    pub const fn from_size(size: Size) -> Self {
        Self::new(0, 0, size.width, size.height)
    }

    pub const fn size(self) -> Size {
        Size::new(self.width, self.height)
    }

    pub const fn is_empty(self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// The smallest rect holding both. An empty rect adds nothing.
    pub fn union(self, other: Self) -> Self {
        if other.is_empty() {
            return self;
        }
        if self.is_empty() {
            return other;
        }
        let [x0, y0, x1, y1] = self.edges();
        let [ox0, oy0, ox1, oy1] = other.edges();
        Self::from_edges(x0.min(ox0), y0.min(oy0), x1.max(ox1), y1.max(oy1))
    }

    /// The overlap of both, empty at the clamped position when they do not overlap.
    pub fn intersection(self, other: Self) -> Self {
        let [x0, y0, x1, y1] = self.edges();
        let [ox0, oy0, ox1, oy1] = other.edges();
        let (x0, y0) = (x0.max(ox0), y0.max(oy0));
        Self::from_edges(x0, y0, x1.min(ox1).max(x0), y1.min(oy1).max(y0))
    }

    pub fn contains(self, other: Self) -> bool {
        let [x0, y0, x1, y1] = self.edges();
        let [ox0, oy0, ox1, oy1] = other.edges();
        x0 <= ox0 && y0 <= oy0 && ox1 <= x1 && oy1 <= y1
    }

    /// The smallest whole-pixel rect covering `[min_x, min_y, max_x, max_y]`: empty when the box
    /// is empty, and `None` when it is not finite, which is unknown rather than empty.
    pub fn covering(bounds: [f32; 4]) -> Option<Self> {
        let [min_x, min_y, max_x, max_y] = bounds.map(f64::from);
        if bounds.iter().any(|v| !v.is_finite()) {
            return None;
        }
        if !(min_x < max_x && min_y < max_y) {
            return Some(Self::new(0, 0, 0, 0));
        }
        let clamp = |value: f64| value.clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i64;
        Some(Self::from_edges(
            clamp(min_x.floor()),
            clamp(min_y.floor()),
            clamp(max_x.ceil()),
            clamp(max_y.ceil()),
        ))
    }

    fn edges(self) -> [i64; 4] {
        let (x, y) = (i64::from(self.x), i64::from(self.y));
        [x, y, x + i64::from(self.width), y + i64::from(self.height)]
    }

    fn from_edges(x0: i64, y0: i64, x1: i64, y1: i64) -> Self {
        let clamp = |value: i64| value.clamp(i64::from(i32::MIN), i64::from(i32::MAX));
        let (x0, y0) = (clamp(x0), clamp(y0));
        Self::new(
            x0 as i32,
            y0 as i32,
            (clamp(x1) - x0).clamp(0, i64::from(u32::MAX)) as u32,
            (clamp(y1) - y0).clamp(0, i64::from(u32::MAX)) as u32,
        )
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TextureDomain {
    pub storage_size: Size,
    pub display_rect: RectI,
    pub data_rect: RectI,
}

impl TextureDomain {
    pub const fn full_frame(size: Size) -> Self {
        let rect = RectI::from_size(size);
        Self {
            storage_size: size,
            display_rect: rect,
            data_rect: rect,
        }
    }

    /// A raster of `display` size that is transparent outside `data`, storing only `data`:
    /// texel `(0, 0)` holds the pixel at its origin. `data` is clipped to the display and
    /// kept at least one texel in size.
    pub fn bounded(display: Size, data: RectI) -> Self {
        let display_rect = RectI::from_size(display);
        let mut data_rect = display_rect.intersection(data);
        data_rect.width = data_rect.width.max(1).min(display.width.max(1));
        data_rect.height = data_rect.height.max(1).min(display.height.max(1));
        data_rect.x = data_rect
            .x
            .min(display.width.saturating_sub(data_rect.width) as i32);
        data_rect.y = data_rect
            .y
            .min(display.height.saturating_sub(data_rect.height) as i32);
        Self {
            storage_size: data_rect.size(),
            display_rect,
            data_rect,
        }
    }

    pub const fn display_size(&self) -> Size {
        self.display_rect.size()
    }

    /// Whether the texels cover the whole display.
    pub fn is_full_frame(&self) -> bool {
        self.data_rect == self.display_rect && self.storage_size == self.display_rect.size()
    }
}
