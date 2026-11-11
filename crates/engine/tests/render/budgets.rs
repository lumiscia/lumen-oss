//! Cache budgets set in one place: each cache keeps what the frame uses whatever its budget,
//! frees the rest past it, and reports what it holds.

use lumen_engine::composition::Composition;
use lumen_engine::gpu::{CacheBudgets, CacheStats};
use serde_json::json;

use crate::support::{TestMedia, gpu, render, scene};

/// The bytes `held` reads after `second` renders following `first` on one renderer, whose budgets
/// `budgets` sets, and after `second` alone on a new one.
fn held_after(
    first: &Composition,
    second: &Composition,
    media: &TestMedia,
    budgets: impl Fn(CacheBudgets) -> CacheBudgets,
    held: impl Fn(&CacheStats) -> u64,
) -> (u64, u64) {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    renderer.set_cache_budgets(budgets(renderer.cache_budgets()));
    render(&mut renderer, first, 0.0, media);
    render(&mut renderer, second, 0.0, media);
    let both = held(&renderer.cache_stats());
    let mut alone = gpu.renderer();
    render(&mut alone, second, 0.0, media);
    (both, held(&alone.cache_stats()))
}

/// A cache keeps what it held from `first` within its budget, and with none keeps only what
/// `second` uses.
fn check_budget(
    first: Composition,
    second: Composition,
    media: TestMedia,
    zero: impl Fn(CacheBudgets) -> CacheBudgets,
    held: impl Fn(&CacheStats) -> u64 + Copy,
) {
    let (kept, used) = held_after(&first, &second, &media, |budgets| budgets, held);
    assert!(used > 0, "the second composition fills the cache");
    assert!(kept > used, "within the budget what the first used stays");
    let (kept, used) = held_after(&first, &second, &media, zero, held);
    assert_eq!(kept, used, "past the budget only what the frame uses stays");
}

fn graded(lut: &str) -> Composition {
    scene(
        (2, 1),
        json!([
            { "id": 1, "type": "media_in", "params": { "kind": "image", "source": "image" } },
            { "id": 2, "type": "color_grade", "in": { "source": 1 },
              "params": { "lut_source": lut } },
        ]),
    )
    .into()
}

fn identity_cube() -> String {
    let mut text = String::from("LUT_3D_SIZE 4\n");
    for b in 0..4 {
        for g in 0..4 {
            for r in 0..4 {
                text += &format!("{} {} {}\n", r as f64 / 3.0, g as f64 / 3.0, b as f64 / 3.0);
            }
        }
    }
    text
}

#[test]
fn luts_past_their_budget_keep_only_what_the_frame_uses() {
    let media = TestMedia::default()
        .image("image", 2, 1, vec![255; 8])
        .lut("a.cube", &identity_cube())
        .lut("b.cube", &identity_cube());
    check_budget(
        graded("a.cube"),
        graded("b.cube"),
        media,
        |budgets| CacheBudgets { luts: 0, ..budgets },
        |stats| stats.luts,
    );
}

#[test]
fn images_past_their_budget_keep_only_what_the_frame_binds() {
    let shown = |source: &str| -> Composition {
        scene(
            (4, 4),
            json!([{ "id": 1, "type": "media_in",
                     "params": { "kind": "image", "source": source } }]),
        )
        .into()
    };
    let media =
        TestMedia::default()
            .image("a", 4, 4, vec![255; 64])
            .image("b", 4, 4, vec![128; 64]);
    check_budget(
        shown("a"),
        shown("b"),
        media,
        |budgets| CacheBudgets {
            image_textures: 0,
            ..budgets
        },
        |stats| stats.image_textures,
    );
}

#[test]
fn stats_report_what_each_cache_holds() {
    let gpu = gpu();
    let mut renderer = gpu.renderer();
    assert_eq!(renderer.cache_stats(), CacheStats::default());
    let media = TestMedia::default()
        .image("image", 2, 1, vec![255; 8])
        .lut("a.cube", &identity_cube());
    render(&mut renderer, &graded("a.cube"), 0.0, &media);
    let stats = renderer.cache_stats();
    assert_eq!(stats.image_textures, 2 * 4);
    assert!(stats.luts > 0 && stats.retained_outputs > 0, "{stats:?}");
    assert!(stats.pooled_textures >= stats.retained_outputs, "{stats:?}");
    renderer.release_resources();
    let released = renderer.cache_stats();
    assert_eq!(
        (
            released.image_textures,
            released.luts,
            released.retained_outputs
        ),
        (0, 0, 0)
    );
}

#[cfg(feature = "3d")]
mod scene_caches {
    use super::*;
    use crate::scene::{environment_file, model, render_3d_with};
    use lumen_3d::glam::Vec3;
    use serde_json::Value;

    fn sphere() -> Value {
        json!({ "id": 1, "type": "shape_3d", "params": { "shape": "sphere" } })
    }

    #[test]
    fn models_past_their_budget_keep_only_what_the_frame_draws() {
        let fox = include_bytes!("../fixtures/Fox.glb").to_vec();
        let media = TestMedia::default()
            .model("a.glb", fox.clone())
            .model("b.glb", fox);
        let drawn = |source: &str| -> Composition {
            render_3d_with(json!({}), vec![model(1, json!({ "source": source }))]).into()
        };
        check_budget(
            drawn("a.glb"),
            drawn("b.glb"),
            media,
            |budgets| CacheBudgets {
                models: 0,
                ..budgets
            },
            |stats| stats.models,
        );
    }

    #[test]
    fn environments_past_their_budget_keep_only_what_the_frame_lights_with() {
        let map = environment_file(32, 16, |_| Vec3::splat(0.5));
        let media = TestMedia::default()
            .environment("a.hdr", map.clone())
            .environment("b.hdr", map);
        let lit = |source: &str| -> Composition {
            render_3d_with(json!({ "environment_map": source }), vec![sphere()]).into()
        };
        check_budget(
            lit("a.hdr"),
            lit("b.hdr"),
            media,
            |budgets| CacheBudgets {
                environments: 0,
                ..budgets
            },
            |stats| stats.environments,
        );
    }

    #[test]
    fn text_meshes_past_their_budget_keep_only_what_the_frame_draws() {
        let text = |content: &str| -> Composition {
            render_3d_with(
                json!({}),
                vec![json!({ "id": 1, "type": "text_3d", "params": { "content": content } })],
            )
            .into()
        };
        check_budget(
            text("l"),
            text("I"),
            TestMedia::default(),
            |budgets| CacheBudgets {
                text_meshes: 0,
                ..budgets
            },
            |stats| stats.text_meshes,
        );
    }
}
