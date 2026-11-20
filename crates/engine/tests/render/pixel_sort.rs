//! `pixel_sort` on grey rows and columns, where each run and segment is known from the greys.

use serde_json::{Value, json};

use crate::support::{Frame, TestMedia, gpu, render, scene};

/// `pixel_sort` with `params` over an opaque image of `width` by `height` whose pixel at `(x, y)`
/// is `pixel`.
fn sorted(
    (width, height): (u32, u32),
    pixel: impl Fn(u32, u32) -> [u8; 4],
    params: Value,
) -> Frame {
    let rgba = (0..height)
        .flat_map(|y| (0..width).map(move |x| (x, y)))
        .flat_map(|(x, y)| pixel(x, y))
        .collect();
    let media = TestMedia::default().image("picture", width, height, rgba);
    let composition = scene(
        (width, height),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "picture" } },
            { "id": 2, "type": "pixel_sort", "in": { "source": 1 }, "params": params },
        ]),
    )
    .into();
    render(&mut gpu().renderer(), &composition, 0.0, &media)
}

fn grey(value: u8) -> [u8; 4] {
    [value, value, value, 255]
}

/// The greys of row `y`.
fn row(frame: &Frame, y: u32) -> Vec<u8> {
    (0..frame.width).map(|x| frame.pixel(x, y)[0]).collect()
}

/// `greys` as the only row of a picture, sorted with `params`.
fn sort_row(greys: &[u8], params: Value) -> Vec<u8> {
    let frame = sorted(
        (greys.len() as u32, 1),
        |x, _| grey(greys[x as usize]),
        params,
    );
    row(&frame, 0)
}

// Greys 64 to 204 are in the default band, 0.25 to 0.8 of luma.
#[test]
fn a_run_inside_the_band_sorts_ascending_and_descending_with_reverse() {
    let greys = [180, 90, 150, 120, 200, 100];
    assert_eq!(sort_row(&greys, json!({})), [90, 100, 120, 150, 180, 200]);
    assert_eq!(
        sort_row(&greys, json!({ "reverse": true })),
        [200, 180, 150, 120, 100, 90]
    );
}

#[test]
fn a_pixel_outside_the_band_stays_put_and_splits_the_run() {
    let greys = [180, 90, 255, 150, 120, 30, 110, 100];
    assert_eq!(
        sort_row(&greys, json!({})),
        [90, 180, 255, 120, 150, 30, 100, 110]
    );
}

#[test]
fn max_length_sorts_each_piece_of_a_long_run_alone() {
    let greys = [200, 190, 180, 170, 160, 150, 140, 130];
    assert_eq!(
        sort_row(&greys, json!({ "max_length": 4 })),
        [170, 180, 190, 200, 130, 140, 150, 160]
    );
}

#[test]
fn segments_of_a_thousand_pixels_sort_whole() {
    let greys: Vec<u8> = (0..1100u32)
        .map(|index| 100 + (index.wrapping_mul(2_654_435_761) >> 24) as u8 % 100)
        .collect();
    let mut expected = greys.clone();
    expected[..1024].sort_unstable();
    expected[1024..].sort_unstable();
    assert_eq!(sort_row(&greys, json!({ "max_length": 5000 })), expected);
}

#[test]
fn vertical_sorts_columns() {
    let columns = [[180u8, 90, 255, 150, 120, 100], [200, 190, 180, 30, 80, 90]];
    let frame = sorted(
        (2, 6),
        |x, y| grey(columns[x as usize][y as usize]),
        json!({ "direction": "vertical" }),
    );
    let column = |x| (0..6).map(|y| frame.pixel(x, y)[0]).collect::<Vec<_>>();
    assert_eq!(column(0), [90, 180, 255, 100, 120, 150]);
    assert_eq!(column(1), [180, 190, 200, 30, 80, 90]);
}

#[test]
fn each_row_keeps_exactly_its_pixels() {
    let colour = |x: u32, y: u32| {
        let seed = (x * 7919 + y * 104_729).wrapping_mul(2_654_435_761);
        [
            (seed >> 8) as u8,
            (seed >> 16) as u8,
            (seed >> 24) as u8,
            255,
        ]
    };
    let pixels = |frame: &Frame, y: u32| (0..48).map(|x| frame.pixel(x, y)).collect::<Vec<_>>();
    let unsorted = sorted((48, 6), colour, json!({ "low": 1, "high": 0 }));
    for key in ["luma", "hue", "saturation"] {
        let frame = sorted(
            (48, 6),
            colour,
            json!({ "key": key, "low": 0, "high": 1, "max_length": 20 }),
        );
        assert!(
            (0..6).any(|y| pixels(&frame, y) != pixels(&unsorted, y)),
            "{key} moved nothing"
        );
        for y in 0..6 {
            let mut after = pixels(&frame, y);
            let mut before = pixels(&unsorted, y);
            after.sort_unstable();
            before.sort_unstable();
            assert_eq!(after, before, "row {y} by {key}");
        }
    }
}

#[test]
fn a_source_off_the_canvas_origin_keeps_its_pixels() {
    let greys = [180u8, 120, 90];
    let media = TestMedia::default().image(
        "picture",
        3,
        1,
        greys.iter().flat_map(|&g| grey(g)).collect(),
    );
    let composition = scene(
        (5, 1),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "picture" } },
            { "id": 2, "type": "transform", "in": { "source": 1 },
              "params": { "translate_x": 1, "sampling": "nearest" } },
            { "id": 3, "type": "pixel_sort", "in": { "source": 2 }, "params": { "max_length": 4 } },
        ]),
    )
    .into();
    let frame = render(&mut gpu().renderer(), &composition, 0.0, &media);
    assert_eq!(&row(&frame, 0)[1..4], [90, 120, 180]);
}
