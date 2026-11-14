//! An image plane's source: the region of it a plane shows, and how it is filtered and supersampled.

use lumen_engine::composition::Composition;
use serde_json::{Value, json};

use crate::support::scene::{
    CENTER, SIZE, covered, framed, gradient, image, merge, ortho, plane, plate, render_3d,
    render_3d_with,
};
use crate::support::{
    BLUE, Case, Expect, Frame, GREEN, RED, Scene, TestMedia, check, gpu, render, scene,
};

#[test]
fn an_unlit_plane_filling_the_frame_reproduces_its_source_whatever_the_tone_mapping() {
    let source = || scene(SIZE, json!([plate(1)]));
    let seen = |params: Value| {
        let mut nodes = vec![plate(10)];
        nodes.extend(framed(10));
        render_3d_with(params, nodes)
    };
    // One texel to a pixel, sampled at texel centres.
    let exact = |name: &str, params: Value| {
        Case::new(name, seen(params), Expect::Matches(source().into()))
            .media(gradient())
            .tolerance(1)
    };
    check(vec![
        exact("untouched", json!({ "supersample": 1 })),
        exact(
            "with ACES and an extra stop",
            json!({ "supersample": 1, "tone_mapping": "aces", "exposure": 1 }),
        ),
        exact(
            "with AgX and a stop less",
            json!({ "supersample": 1, "tone_mapping": "agx", "exposure": -1 }),
        ),
        exact(
            "with the default supersampling and ACES",
            json!({ "tone_mapping": "aces", "exposure": 1 }),
        ),
    ]);
}

/// A plane 4 wide, with `params` besides, showing the whole `plate` before an orthographic camera,
/// or the part of it that a `crop` of `crop` leaves, or that the plane's own `region` leaves.
fn plane_showing(part: Option<([i64; 4], bool)>, params: Value) -> Scene {
    let mut params = params;
    params["width"] = json!(4);
    let mut nodes = vec![plate(10)];
    let source = match part {
        Some(([x, y, width, height], true)) => {
            nodes.push(json!({
                "id": 11, "type": "crop", "in": { "source": 10 },
                "params": { "x": x, "y": y, "width": width, "height": height }
            }));
            11
        }
        Some((region, false)) => {
            params["region"] = json!(region);
            10
        }
        None => 10,
    };
    nodes.push(plane(1, source, params));
    nodes.extend([ortho(2, json!({})), merge(3, &[1, 2], json!({}))]);
    render_3d_with(json!({ "supersample": 1 }), nodes)
}

fn crop_of(region: [i64; 4], params: Value) -> Scene {
    plane_showing(Some((region, true)), params)
}

fn region_of(region: [i64; 4], params: Value) -> Scene {
    plane_showing(Some((region, false)), params)
}

#[test]
fn a_plane_with_a_region_looks_as_a_plane_showing_a_crop_of_it() {
    // Moved whole pixels and rolled a quarter turn, so that texels still land on pixels.
    let placed = || json!({ "position": [0.25, -0.125, 0.5], "rotation": [0, 0, 90] });
    check(vec![
        Case::new(
            "inside the source",
            region_of([16, 8, 32, 24], json!({})),
            Expect::Matches(crop_of([16, 8, 32, 24], json!({})).into()),
        )
        .media(gradient())
        .tolerance(1),
        Case::new(
            "moved and rolled about the source's centre",
            region_of([40, 4, 20, 50], placed()),
            Expect::Matches(crop_of([40, 4, 20, 50], placed()).into()),
        )
        .media(gradient())
        .tolerance(2),
    ]);
}

#[test]
fn a_region_is_cut_off_at_the_edges_of_its_source_as_a_crop_is() {
    let nothing = || {
        Expect::Custom(Box::new(|frames: &[Frame]| {
            assert_eq!(covered(&frames[0], CENTER, 0..SIZE.0), 0);
        }))
    };
    check(vec![
        Case::new(
            "reaching past the right and bottom edges",
            region_of([40, 30, 100, 100], json!({})),
            Expect::Matches(crop_of([40, 30, 100, 100], json!({})).into()),
        )
        .media(gradient())
        .tolerance(1),
        Case::new(
            "starting before the origin",
            region_of([-20, -10, 40, 30], json!({})),
            Expect::Matches(crop_of([-20, -10, 40, 30], json!({})).into()),
        )
        .media(gradient())
        .tolerance(1),
        Case::new(
            "wholly outside",
            region_of([64, 0, 10, 10], json!({})),
            nothing(),
        )
        .media(gradient()),
        Case::new(
            "with no width",
            region_of([10, 10, 0, 20], json!({})),
            nothing(),
        )
        .media(gradient()),
    ]);
}

#[test]
fn a_region_covering_the_whole_source_is_the_plane_without_one() {
    check(vec![
        Case::new(
            "the whole source",
            region_of([0, 0, 64, 64], json!({})),
            Expect::Matches(plane_showing(None, json!({})).into()),
        )
        .media(gradient())
        .tolerance(1),
    ]);
}

/// A 64 x 64 image, red on its left half and green on its right.
fn red_and_green() -> TestMedia {
    image(|x, _| if x < CENTER { RED } else { GREEN })
}

#[test]
fn a_magnified_region_shows_nothing_from_beside_it() {
    // Four pixels to a texel, with the edge between the halves at the middle of the frame.
    let half = |x: i64| {
        render_3d_with(
            json!({ "supersample": 1 }),
            vec![
                plate(10),
                plane(1, 10, json!({ "width": 16, "region": [x, 0, 32, 64] })),
                ortho(2, json!({})),
                merge(3, &[1, 2], json!({})),
            ],
        )
    };
    let only = |color: [u8; 4], xs: std::ops::Range<u32>| {
        Expect::Custom(Box::new(move |frames: &[Frame]| {
            for y in 0..SIZE.1 {
                for x in xs.clone() {
                    assert_eq!(frames[0].pixel(x, y), color, "({x}, {y})");
                }
            }
        }))
    };
    check(vec![
        Case::new("the right half", half(32), only(GREEN, CENTER..SIZE.0)).media(red_and_green()),
        Case::new("the left half", half(0), only(RED, 0..CENTER)).media(red_and_green()),
    ]);
}

#[test]
fn a_minified_region_shows_little_from_beside_it() {
    // A source 1280 pixels across, red up to x = 640 and green after, shown at ten texels to a
    // pixel: the red region from x = 320 ends at the middle of the frame.
    let (width, height) = (1280, 64);
    let pixels = (0..height)
        .flat_map(|_| (0..width).flat_map(|x| if x < 640 { RED } else { GREEN }))
        .collect();
    let media = TestMedia::default().image("plate", width, height, pixels);
    let nodes = vec![
        plate(10),
        plane(1, 10, json!({ "width": 8, "region": [320, 0, 320, 64] })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ];
    let frame = render(
        &mut gpu().renderer(),
        &render_3d_with(json!({ "supersample": 1 }), nodes).into(),
        0.0,
        &media,
    );
    for y in 30..34 {
        for x in 0..CENTER {
            let [red, green, ..] = frame.pixel(x, y);
            assert!(
                red > 200 && green <= 4,
                "({x}, {y}) is {red} red, {green} green"
            );
        }
    }
}

/// The most red a pixel of a region that holds no red may show: what filtering and the working
/// space's rounding leave of exact green.
const BLEED_TOLERANCE: u8 = 4;

/// Renders a source `size` pixels across, red left of `edge` and green from it on, on a plane
/// `plane_width` wide showing `region`, which lies in the green, and returns a description of what
/// is wrong where it is not drawn in green with no red in it.
fn bleed_of_green_region(
    size: (u32, u32),
    edge: u32,
    plane_width: f64,
    region: [i64; 4],
) -> Option<String> {
    let (width, height) = size;
    let pixels = (0..height)
        .flat_map(|_| (0..width).flat_map(|x| if x < edge { RED } else { GREEN }))
        .collect();
    let media = TestMedia::default().image("plate", width, height, pixels);
    let nodes = vec![
        plate(10),
        plane(1, 10, json!({ "width": plane_width, "region": region })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ];
    let frame = render(
        &mut gpu().renderer(),
        &render_3d_with(json!({ "supersample": 1 }), nodes).into(),
        0.0,
        &media,
    );
    let drawn: Vec<[u8; 4]> = (0..SIZE.0 * SIZE.1)
        .map(|at| frame.pixel(at % SIZE.0, at / SIZE.0))
        .filter(|pixel| pixel[3] == 255)
        .collect();
    let worst_red = drawn.iter().map(|pixel| pixel[0]).max().unwrap_or(0);
    let least_green = drawn.iter().map(|pixel| pixel[1]).min().unwrap_or(0);
    eprintln!("edge {edge}, {region:?}: red up to {worst_red}, green down to {least_green}");
    (drawn.is_empty() || worst_red > BLEED_TOLERANCE || least_green < 255 - BLEED_TOLERANCE).then(
        || format!("edge {edge} of {width}: red up to {worst_red}, green down to {least_green}"),
    )
}

#[test]
fn a_region_a_texel_wide_shows_only_its_texel_when_minified_along_it() {
    // A column of green at x = 632 of 1280 pixels, one pixel across when shown and 160 texels to
    // a pixel down, which a shared mip level of 8 or more texels would average into red.
    let (width, height) = (1280, 1280);
    let pixels = (0..height)
        .flat_map(|_| (0..width).flat_map(|x| if x == 632 { GREEN } else { RED }))
        .collect();
    let media = TestMedia::default().image("plate", width, height, pixels);
    let nodes = vec![
        plate(10),
        plane(
            1,
            10,
            json!({ "width": 8, "scale": [10, 0.0625, 1], "region": [632, 0, 1, 1280] }),
        ),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ];
    let frame = render(
        &mut gpu().renderer(),
        &render_3d_with(json!({ "supersample": 1 }), nodes).into(),
        0.0,
        &media,
    );
    let [red, green, _, alpha] = frame.pixel(24, CENTER);
    assert_eq!(alpha, 255);
    assert!(
        red <= BLEED_TOLERANCE && green >= 251,
        "the column is [{red}, {green}], not green"
    );
}

#[test]
fn a_minified_region_never_shows_what_lies_beyond_its_edge() {
    // Ten texels to a pixel, with green from the region's left edge and red before it, the edge
    // on and off the grid of the mip levels' texels.
    let mut wrong = Vec::new();
    for edge in [633, 640, 650, 652] {
        wrong.extend(bleed_of_green_region(
            (1280, 64),
            edge,
            8.0,
            [i64::from(edge), 0, 320, 64],
        ));
    }
    wrong.extend(bleed_of_green_region(
        (1920, 64),
        970,
        12.0,
        [970, 0, 320, 64],
    ));
    assert!(wrong.is_empty(), "{wrong:#?}");
}

#[test]
fn a_region_with_moving_bounds_renders_each_frame_as_a_fresh_renderer_does() {
    let gpu = gpu();
    let media = fine_detail();
    for width in [8.0, 1.0] {
        let composition = render_3d_with(
            json!({ "supersample": 1 }),
            vec![
                plate(10),
                plane(
                    1,
                    10,
                    json!({ "width": width, "region": ["=8 + frame * 4", "=frame * 2", 40, "=48 - frame * 2"] }),
                ),
                ortho(2, json!({})),
                merge(3, &[1, 2], json!({})),
            ],
        )
        .frames(5)
        .into();
        let mut kept = gpu.renderer();
        let mut previous = None;
        for frame in 0..5 {
            let at = f64::from(frame);
            let shown = render(&mut kept, &composition, at, &media).rgba;
            let fresh = render(&mut gpu.renderer(), &composition, at, &media).rgba;
            assert!(shown == fresh, "frame {frame} at width {width}");
            assert!(previous.as_ref() != Some(&shown), "the region moves");
            previous = Some(shown);
        }
    }
}

#[test]
fn a_region_inside_an_instance_is_drawn_as_the_plane_itself() {
    // Minified, so the plane's source needs its mip levels either way.
    let shown = |instanced: bool, region: [i64; 4], width: f64| {
        let mut nodes = vec![
            plate(10),
            plane(1, 10, json!({ "width": width, "region": region })),
        ];
        let object = if instanced {
            nodes.push(
                json!({ "id": 4, "type": "instance_3d", "params": { "count": 1 },
                "in": { "scene": 1 } }),
            );
            4
        } else {
            1
        };
        nodes.extend([ortho(2, json!({})), merge(3, &[object, 2], json!({}))]);
        render_3d_with(json!({ "supersample": 1 }), nodes)
    };
    check(
        [("magnified", 8.0), ("minified", 1.0)]
            .map(|(name, width)| {
                Case::new(
                    name,
                    shown(true, [16, 8, 40, 40], width),
                    Expect::Matches(shown(false, [16, 8, 40, 40], width).into()),
                )
                .media(fine_detail())
            })
            .into(),
    );
}

#[test]
fn planes_showing_regions_of_one_source_draw_together() {
    let gpu = gpu();
    let media = gradient();
    let passes = |pieces: i64| {
        let mut nodes = vec![plate(10)];
        let mut objects = vec![2];
        for piece in 0..pieces {
            let id = 20 + piece as u32;
            let region = [piece % 4 * 16, piece / 4 * 16, 16, 16];
            let depth = -0.1 * piece as f64;
            nodes.push(plane(
                id,
                10,
                json!({ "width": 4, "region": region, "position": [0, 0, depth] }),
            ));
            objects.push(id);
        }
        nodes.extend([ortho(2, json!({})), merge(3, &objects, json!({}))]);
        let mut renderer = gpu.renderer();
        render(
            &mut renderer,
            &render_3d_with(json!({}), nodes).into(),
            0.0,
            &media,
        );
        renderer.submitted_pass_count()
    };
    assert_eq!(passes(16), passes(1));
}

/// A checkerboard of one-pixel squares on the left, and on the right one-pixel white strokes
/// across black.
fn fine_detail() -> TestMedia {
    image(|x, y| {
        let white = if x < CENTER {
            (x + y) % 2 == 0
        } else {
            x == 48 || y == 20
        };
        if white { [255; 4] } else { [0, 0, 0, 255] }
    })
}

#[test]
fn a_plane_drawn_pixel_for_pixel_keeps_one_pixel_detail_at_any_supersample() {
    let seen = |supersample: u32| {
        let mut nodes = vec![plate(10)];
        nodes.extend(framed(10));
        render_3d_with(json!({ "supersample": supersample }), nodes)
    };
    check(
        [1, 2, 3, 4]
            .map(|supersample| {
                Case::new(
                    format!("supersample {supersample}"),
                    seen(supersample),
                    Expect::Matches(scene(SIZE, json!([plate(1)])).into()),
                )
                .media(fine_detail())
            })
            .into(),
    );
}

#[test]
fn a_plane_with_a_texel_to_each_supersample_is_averaged_by_them_and_not_filtered_again() {
    // 32 x 32 output pixels show 64 x 64 texels, so each of the four supersamples of the default
    // reads one texel. They should average to what a render twice as large, with one sample to
    // each pixel, gives over each 2 x 2 pixels, in linear light.
    let seen = |canvas: u32, supersample: u32| {
        let nodes = json!([
            plate(10),
            plane(1, 10, json!({ "width": 2 })),
            ortho(2, json!({})),
            merge(3, &[1, 2], json!({})),
            { "id": 100, "type": "render_3d", "params": { "supersample": supersample },
              "in": { "scene": 3 } },
        ]);
        Composition::from(scene((canvas, canvas), nodes))
    };
    let to_linear = |byte: u8| {
        let c = f64::from(byte) / 255.0;
        if c <= 0.04045 {
            c / 12.92
        } else {
            ((c + 0.055) / 1.055).powf(2.4)
        }
    };
    let to_srgb = |c: f64| {
        let c = if c <= 0.0031308 {
            c * 12.92
        } else {
            1.055 * c.powf(1.0 / 2.4) - 0.055
        };
        (c * 255.0).round() as u8
    };
    let gpu = gpu();
    let media = fine_detail();
    let supersampled = render(&mut gpu.renderer(), &seen(64, 2), 0.0, &media);
    let large = render(&mut gpu.renderer(), &seen(128, 1), 0.0, &media);
    // The plane covers pixels 16 to 48, and its edges are left out.
    for y in 18..46 {
        for x in 18..46 {
            for channel in 0..3 {
                let sum: f64 = [(0, 0), (1, 0), (0, 1), (1, 1)]
                    .iter()
                    .map(|(dx, dy)| to_linear(large.pixel(2 * x + dx, 2 * y + dy)[channel]))
                    .sum();
                let expected = to_srgb(sum / 4.0);
                let actual = supersampled.pixel(x, y)[channel];
                assert!(
                    actual.abs_diff(expected) <= 1,
                    "({x}, {y}) channel {channel} is {actual}, expected {expected}"
                );
            }
        }
    }
}

#[test]
fn a_planes_silhouette_is_antialiased_without_a_seam_at_the_default_supersample() {
    // A blue square turned 20° and not filling the frame has partly covered edge pixels, each
    // blue scaled by its coverage like any other blended edge.
    let mut nodes = vec![plate(10)];
    nodes.extend([
        plane(1, 10, json!({ "width": 2, "rotation": [0, 0, 20] })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ]);
    check(vec![
        Case::new(
            "a rotated plane",
            render_3d(nodes),
            Expect::Custom(Box::new(|frames| {
                let edges: Vec<[u8; 4]> = frames[0]
                    .rgba
                    .chunks(4)
                    .map(|pixel| [pixel[0], pixel[1], pixel[2], pixel[3]])
                    .filter(|pixel| (1..255).contains(&pixel[3]))
                    .collect();
                assert!(edges.len() > 40, "{} edge pixels", edges.len());
                for edge in edges {
                    assert!(
                        edge[0] == 0 && edge[1] == 0 && edge[2].abs_diff(edge[3]) <= 2,
                        "an edge pixel is blue scaled by coverage, got {edge:?}"
                    );
                }
                assert_eq!(frames[0].pixel(CENTER, CENTER), BLUE);
            })),
        )
        .media(image(|_, _| BLUE)),
    ]);
}

#[test]
fn opacity_scales_the_alpha_of_a_source() {
    let mut nodes = vec![plate(10)];
    nodes.extend([
        plane(1, 10, json!({ "width": 4, "opacity": 0.5 })),
        ortho(2, json!({})),
        merge(3, &[1, 2], json!({})),
    ]);
    check(vec![
        Case::new(
            "half-transparent blue at half opacity",
            render_3d(nodes),
            Expect::Probes(vec![(CENTER, CENTER, [0, 0, 64, 64])]),
        )
        .media(image(|_, _| [0, 0, 255, 128])),
    ]);
}
