use super::drawable::{DrawableMotion, DrawableTransform};
use super::path::{POINT_CLOSED, POINT_END};
use super::renderer::{PathPoint, PathUniforms, STYLE_FILL_ENABLED, STYLE_STROKE_ENABLED};

/// Most bands a path is split into.
const MAX_PATH_BANDS: usize = 1024;
/// Band entries reserved per point. Paths that would list more get fewer, taller bands.
const BAND_ENTRIES_PER_POINT: usize = 8;

/// Length in `u32`s of the bands buffer for a path of up to `point_capacity` points.
pub(super) fn band_capacity(point_capacity: usize) -> usize {
    2 * MAX_PATH_BANDS + BAND_ENTRIES_PER_POINT * point_capacity.max(1)
}

/// Splits the path into horizontal bands in untransformed space, listing for each band
/// the segments that can affect a pixel in it, and fills in the culling fields of
/// `uniforms`. A pixel's horizontal crossing ray only meets segments spanning its row,
/// all of which are listed in its band, so the fill rule needs no other bands.
///
/// The buffer starts with a `(start, count)` pair per band, followed by the index of the
/// first point of each listed segment. An empty buffer means nothing is drawn. A blurred
/// path's culling covers every tap of its `motion`.
pub(super) fn bin_path(
    uniforms: &mut PathUniforms,
    points: &[PathPoint],
    transform: &DrawableTransform,
    motion: &DrawableMotion,
    capacity: usize,
) -> Vec<u32> {
    uniforms.screen_bounds = [f32::MAX, f32::MAX, f32::MIN, f32::MIN];
    uniforms.band_count = 0;
    let fill = uniforms.flags & STYLE_FILL_ENABLED != 0;
    let stroke = uniforms.flags & STYLE_STROKE_ENABLED != 0;
    // The open end of a contour joins its start only for the fill's crossing count.
    let segments: Vec<(u32, [f64; 2])> = points
        .iter()
        .enumerate()
        .filter_map(|(index, point)| {
            let is_end = point.flags & POINT_END != 0;
            let next = if is_end {
                point.contour_start as usize
            } else {
                index + 1
            };
            let open_end = is_end && point.flags & POINT_CLOSED == 0;
            let (y0, y1) = (point.position[1], points.get(next)?.position[1]);
            (next != index && (fill || !open_end))
                .then(|| (index as u32, [f64::from(y0.min(y1)), f64::from(y0.max(y1))]))
        })
        .collect();
    if segments.is_empty() || !(fill || stroke) || !motion.is_visible(transform) {
        return Vec::new();
    }

    // Untransformed distance within which a segment can still cover part of a pixel: a
    // canvas pixel of antialiasing, plus half the stroke.
    let stroke_reach = if stroke {
        f64::from(uniforms.stroke_width).max(0.0) * 0.5
    } else {
        0.0
    };
    let reach = 1.0 / motion.smallest_scale(transform) + stroke_reach;
    let (min, max) = points
        .iter()
        .fold(([f32::MAX; 2], [f32::MIN; 2]), |(min, max), point| {
            (
                [min[0].min(point.position[0]), min[1].min(point.position[1])],
                [max[0].max(point.position[0]), max[1].max(point.position[1])],
            )
        });
    // Covers rounding differences between this binning and the shader's band lookup.
    let pad = reach + 1e-4 * (1.0 + f64::from(min[1].abs().max(max[1].abs())));
    let origin = f64::from(min[1]) - pad;
    let height = f64::from(max[1]) + pad - origin;

    let mut band_count = (height / reach)
        .floor()
        .clamp(1.0, segments.len().min(MAX_PATH_BANDS) as f64) as usize;
    let band_range = |band_count: usize, [low, high]: [f64; 2]| {
        let band = |y: f64| {
            ((y - origin) * band_count as f64 / height).clamp(0.0, (band_count - 1) as f64) as usize
        };
        band(low - pad)..=band(high + pad)
    };
    while band_count > 1
        && 2 * band_count
            + segments
                .iter()
                .map(|(_, span)| band_range(band_count, *span).count())
                .sum::<usize>()
            > capacity
    {
        band_count /= 2;
    }

    let mut data = vec![0; 2 * band_count];
    for (_, span) in &segments {
        for band in band_range(band_count, *span) {
            data[2 * band + 1] += 1;
        }
    }
    let mut start = 2 * band_count as u32;
    for band in 0..band_count {
        data[2 * band] = start;
        start += data[2 * band + 1];
    }
    data.resize(start as usize, 0);
    let mut cursors: Vec<u32> = (0..band_count).map(|band| data[2 * band]).collect();
    for (index, span) in &segments {
        for band in band_range(band_count, *span) {
            data[cursors[band] as usize] = *index;
            cursors[band] += 1;
        }
    }

    let reach = reach as f32;
    let [x, y] = uniforms.position;
    uniforms.screen_bounds = motion.canvas_bounds(
        uniforms.transform,
        [min[0] - reach + x, min[1] - reach + y],
        [max[0] + reach + x, max[1] + reach + y],
    );
    uniforms.band_origin = origin as f32;
    uniforms.band_scale = (band_count as f64 / height) as f32;
    uniforms.band_count = band_count as u32;
    uniforms.reach = reach;
    data
}

#[cfg(test)]
mod tests {
    use super::*;
    use bytemuck::Zeroable;

    fn identity() -> DrawableTransform {
        DrawableTransform {
            position: (0.0, 0.0),
            scale: (1.0, 1.0),
            rotation: 0.0,
            anchor: (0.0, 0.0),
            opacity: 1.0,
        }
    }

    fn bin(vertices: &[[f32; 2]], capacity: usize) -> (PathUniforms, Vec<u32>) {
        let transform = identity();
        let mut uniforms = PathUniforms::zeroed();
        uniforms.flags = STYLE_FILL_ENABLED;
        uniforms.transform = transform.to_gpu();
        let points = super::super::path::closed_contour(vertices);
        let data = bin_path(
            &mut uniforms,
            &points,
            &transform,
            &DrawableMotion::default(),
            capacity,
        );
        (uniforms, data)
    }

    fn band(data: &[u32], band: usize) -> &[u32] {
        let (start, count) = (data[2 * band] as usize, data[2 * band + 1] as usize);
        &data[start..start + count]
    }

    #[test]
    fn lists_each_segment_in_the_bands_it_reaches() {
        // A tall triangle: its two slanted sides span every band, its base only the last.
        let (uniforms, data) = bin(&[[0.0, 0.0], [10.0, 100.0], [-10.0, 100.0]], 4096);
        let count = uniforms.band_count as usize;
        assert!(count > 1);
        assert_eq!(band(&data, 0), &[0, 2]);
        assert_eq!(band(&data, count - 1), &[0, 1, 2]);
        assert_eq!(uniforms.screen_bounds, [-11.0, -1.0, 11.0, 101.0]);
    }

    #[test]
    fn coarsens_bands_to_fit_capacity() {
        // Three bands would need 13 entries; one band lists each segment once.
        let (uniforms, data) = bin(&[[0.0, 0.0], [10.0, 100.0], [-10.0, 100.0]], 8);
        assert_eq!(uniforms.band_count, 1);
        assert_eq!(data, [2, 3, 0, 1, 2]);
    }

    #[test]
    fn invisible_paths_list_nothing() {
        let transform = DrawableTransform {
            opacity: 0.0,
            ..identity()
        };
        let mut uniforms = PathUniforms::zeroed();
        uniforms.flags = STYLE_FILL_ENABLED;
        uniforms.transform = transform.to_gpu();
        let points = super::super::path::closed_contour(&[[0.0, 0.0], [1.0, 0.0], [0.0, 1.0]]);
        assert!(
            bin_path(
                &mut uniforms,
                &points,
                &transform,
                &DrawableMotion::default(),
                4096
            )
            .is_empty()
        );
        assert_eq!(uniforms.band_count, 0);
    }
}
