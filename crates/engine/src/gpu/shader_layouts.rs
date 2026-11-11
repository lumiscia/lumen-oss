//! Compares the structs and constants that Rust and the shaders each declare, which the compiler
//! can't: a mismatch only shows up as wrong pixels.

use std::mem::{offset_of, size_of};

use lumen_text::{GpuGlyphInstance, GpuTextGlobals};
use naga::{ArraySize, Expression, Literal, Module, TypeInner};

use crate::node::{
    motion::MAX_MOTION_TAPS,
    source::text_record::{GpuTextPaint, TextStyleUniforms},
    vector::{
        drawable::{DRAWABLE_SHADER, GpuDrawableMotion, GpuDrawableTransform},
        paint::GpuPaint,
        path::PATH_SHADER,
        renderer::{PathPoint, PathUniforms},
        repeat::GpuRepeatCopy,
    },
};

/// A WGSL struct's name, its Rust size, and the Rust offsets of the WGSL members it names.
type Layout = (&'static str, usize, Vec<(&'static str, usize)>);

macro_rules! layout {
    ($wgsl:literal, $rust:ty, $($member:ident),+ $(,)?) => {
        (
            $wgsl,
            size_of::<$rust>(),
            vec![$((stringify!($member), offset_of!($rust, $member))),+],
        )
    };
}

fn text_module() -> Module {
    let source = [
        include_str!("shaders/alpha.wgsl"),
        include_str!("shaders/color.wgsl"),
        include_str!("../node/vector/paint/paint.wgsl"),
        DRAWABLE_SHADER,
        lumen_text::TEXT_SHADER,
    ]
    .concat();
    naga::front::wgsl::parse_str(&source).expect("the text shader should parse")
}

fn path_module() -> Module {
    naga::front::wgsl::parse_str(PATH_SHADER).expect("the path shader should parse")
}

fn path_layouts() -> Vec<Layout> {
    vec![
        layout!(
            "PathUniforms",
            PathUniforms,
            fill_paint,
            stroke_paint,
            position,
            bounds_min,
            bounds_size,
            stroke_width,
            flags,
            fill_offset,
            trim_start,
            trim_end,
            fill_feather,
            transform,
            screen_bounds,
            band_origin,
            band_scale,
            band_count,
            reach,
            origin,
            dash_count,
            dash_period,
            dash_start_phase,
            dash,
            motion,
        ),
        layout!(
            "PathPoint",
            PathPoint,
            position,
            contour_start,
            flags,
            offset,
            dash_phase,
        ),
        layout!(
            "RepeatCopy",
            GpuRepeatCopy,
            transform,
            bounds,
            first_tap,
            tap_count,
        ),
    ]
}

fn layouts() -> Vec<Layout> {
    vec![
        layout!(
            "TextGlobals",
            GpuTextGlobals,
            target_size,
            px_range,
            glyph_count
        ),
        layout!(
            "GlyphInstance",
            GpuGlyphInstance,
            rect,
            uv_rect,
            opacity,
            partner,
            mode,
            flags,
            offset,
            linear,
        ),
        layout!("TextStyle", TextStyleUniforms, transform, motion),
        layout!("TextPaint", GpuTextPaint, paint, bounds),
        layout!(
            "Paint",
            GpuPaint,
            colors,
            offsets,
            start,
            end,
            center,
            radius,
            focal_offset,
            angle,
            kind,
            units,
            spread,
            interpolation,
            stop_count,
            paint_supersample,
            stroke,
        ),
        layout!(
            "DrawableTransform",
            GpuDrawableTransform,
            linear,
            inverse_linear,
            offset,
            inverse_offset,
            opacity,
            flags,
        ),
        layout!("DrawableMotion", GpuDrawableMotion, tap_count, taps),
    ]
}

fn is_padding(member: &str) -> bool {
    member.starts_with('_') || member == "padding"
}

#[test]
fn shader_structs_match_their_rust_layouts() {
    check_layouts(&text_module(), layouts());
    check_layouts(&path_module(), path_layouts());
}

fn check_layouts(module: &Module, layouts: Vec<Layout>) {
    for (name, size, offsets) in layouts {
        let (members, span) = module
            .types
            .iter()
            .find_map(|(_, ty)| match &ty.inner {
                TypeInner::Struct { members, span } if ty.name.as_deref() == Some(name) => {
                    Some((members, *span))
                }
                _ => None,
            })
            .unwrap_or_else(|| panic!("the shader has no struct `{name}`"));
        assert_eq!(span as usize, size, "size of `{name}`");
        for member in members {
            let member_name = member.name.as_deref().expect("struct members are named");
            if is_padding(member_name) {
                continue;
            }
            let (_, offset) = offsets
                .iter()
                .find(|(field, _)| *field == member_name)
                .unwrap_or_else(|| panic!("`{name}.{member_name}` has no Rust offset to check"));
            assert_eq!(member.offset as usize, *offset, "`{name}.{member_name}`");
        }
    }
}

#[test]
fn drawable_motion_holds_the_rust_tap_count() {
    let module = text_module();
    let taps = module
        .types
        .iter()
        .find_map(|(_, ty)| match &ty.inner {
            TypeInner::Struct { members, .. } if ty.name.as_deref() == Some("DrawableMotion") => {
                members
                    .iter()
                    .find(|member| member.name.as_deref() == Some("taps"))
            }
            _ => None,
        })
        .expect("`DrawableMotion.taps`");
    let TypeInner::Array {
        size: ArraySize::Constant(count),
        ..
    } = module.types[taps.ty].inner
    else {
        panic!("`DrawableMotion.taps` should be a sized array");
    };
    assert_eq!(count.get() as usize, MAX_MOTION_TAPS);
}

fn literal(module: &Module, name: &str) -> Literal {
    let (_, constant) = module
        .constants
        .iter()
        .find(|(_, constant)| constant.name.as_deref() == Some(name))
        .unwrap_or_else(|| panic!("the shader has no constant `{name}`"));
    match module.global_expressions[constant.init] {
        Expression::Literal(literal) => literal,
        ref other => panic!("`{name}` should be a literal, not {other:?}"),
    }
}

#[test]
fn shader_constants_match_rust() {
    let module = text_module();
    let constants = [
        ("GLYPH_TRANSFORMED", lumen_text::GLYPH_TRANSFORMED),
        ("GLYPH_CLUSTER_FOLLOWER", lumen_text::GLYPH_CLUSTER_FOLLOWER),
        ("GLYPH_CLUSTER_SHIFT", lumen_text::GLYPH_CLUSTER_SHIFT),
        ("GLYPH_RANGE_SHIFT", lumen_text::GLYPH_RANGE_SHIFT),
        ("GLYPH_RANGE_MASK", lumen_text::GLYPH_RANGE_MASK),
        ("GLYPH_PARTNER", lumen_text::GLYPH_PARTNER),
        ("GLYPH_MODE_MASK", lumen_text::GLYPH_MODE_MASK),
        ("GLYPH_PAINT_SHIFT", lumen_text::GLYPH_PAINT_SHIFT),
        ("GLYPH_MODE_RECT", lumen_text::GLYPH_MODE_RECT),
        ("RECT_SHAPE_SHIFT", lumen_text::RECT_SHAPE_SHIFT),
        ("RECT_BOX", lumen_text::RECT_BOX),
        ("RECT_DOUBLE", lumen_text::RECT_DOUBLE),
        ("RECT_DOTTED", lumen_text::RECT_DOTTED),
        ("RECT_DASHED", lumen_text::RECT_DASHED),
        ("RECT_WAVY", lumen_text::RECT_WAVY),
        ("RECT_JOINED_LEFT", lumen_text::RECT_JOINED_LEFT),
        ("RECT_JOINED_RIGHT", lumen_text::RECT_JOINED_RIGHT),
    ];
    for (name, value) in constants {
        assert_eq!(literal(&module, name), Literal::U32(value), "`{name}`");
    }
    assert_eq!(
        literal(&module, "RECT_WAVY_AMPLITUDE"),
        Literal::F32(lumen_text::RECT_WAVY_AMPLITUDE),
    );
}

#[cfg(feature = "3d")]
#[test]
fn scene_shader_matches_its_rust_layouts_and_constants() {
    use crate::node::scene::render::{
        FLAG_BASE_COLOR_MAP, FLAG_BLEND, FLAG_EMISSIVE_MAP, FLAG_FLAT_NORMALS, FLAG_MAP_UVS,
        FLAG_MASK, FLAG_METALLIC_ROUGHNESS_MAP, FLAG_MIRRORED, FLAG_NORMAL_MAP, FLAG_OCCLUSION_MAP,
        FLAG_RASTER, FLAG_RECEIVE_SHADOWS, FLAG_SHADOW_CATCHER, FLAG_SKINNED, FLAG_UNLIT,
        FLAG_VERTEX_COLORS, GpuEnvironment, GpuGlobals, GpuLight, GpuMapUv, GpuObject, GpuRun,
        GpuShadowView, LIGHT_DIRECTIONAL, LIGHT_POINT, LIGHT_SPOT, MORPH_DELTA_WORDS, SHADER,
    };

    let module = naga::front::wgsl::parse_str(SHADER).expect("the scene shader should parse");
    check_layouts(
        &module,
        vec![
            layout!(
                "Globals",
                GpuGlobals,
                view_proj,
                camera_position,
                light_count,
                camera_back,
                orthographic,
                ambient,
                environment,
                fog_color,
                fog_density,
                tone_mapping,
                exposure,
                supersample,
                camera_right,
                depth_near,
                camera_up,
                depth_far,
            ),
            layout!(
                "Object",
                GpuObject,
                model,
                normal,
                base_color,
                emissive,
                metallic,
                roughness,
                flags,
                first_vertex,
                first_index,
                first_joint,
                alpha_cutoff,
                normal_scale,
                occlusion_strength,
                extras,
                first_delta,
                morph_vertex,
                morph_targets,
                first_weight,
                first_map_uv,
            ),
            layout!("Vertex", lumen_3d::Vertex, position, normal, uv),
            layout!("MapUv", GpuMapUv, first, second),
            layout!(
                "Light",
                GpuLight,
                position,
                kind,
                direction,
                range,
                radiance,
                cos_outer,
                cos_inner,
                shadow,
                shadow_softness,
                shadow_distance,
            ),
            layout!(
                "ShadowView",
                GpuShadowView,
                view_proj,
                depth,
                tile,
                texel,
                perspective,
            ),
            layout!("Run", GpuRun, first, count, base),
            layout!(
                "Environment",
                GpuEnvironment,
                irradiance,
                clip_to_world,
                turn,
                roughest,
                background_blur,
                map,
            ),
        ],
    );
    for (name, value) in [
        ("FLAG_UNLIT", FLAG_UNLIT),
        ("FLAG_BLEND", FLAG_BLEND),
        ("FLAG_MIRRORED", FLAG_MIRRORED),
        ("FLAG_RASTER", FLAG_RASTER),
        ("FLAG_MASK", FLAG_MASK),
        ("FLAG_SKINNED", FLAG_SKINNED),
        ("FLAG_FLAT_NORMALS", FLAG_FLAT_NORMALS),
        ("FLAG_BASE_COLOR_MAP", FLAG_BASE_COLOR_MAP),
        ("FLAG_METALLIC_ROUGHNESS_MAP", FLAG_METALLIC_ROUGHNESS_MAP),
        ("FLAG_NORMAL_MAP", FLAG_NORMAL_MAP),
        ("FLAG_EMISSIVE_MAP", FLAG_EMISSIVE_MAP),
        ("FLAG_OCCLUSION_MAP", FLAG_OCCLUSION_MAP),
        ("FLAG_RECEIVE_SHADOWS", FLAG_RECEIVE_SHADOWS),
        ("FLAG_SHADOW_CATCHER", FLAG_SHADOW_CATCHER),
        ("FLAG_VERTEX_COLORS", FLAG_VERTEX_COLORS),
        ("FLAG_MAP_UVS", FLAG_MAP_UVS),
        ("NO_EXTRAS", u32::MAX),
        ("MORPH_DELTA_WORDS", MORPH_DELTA_WORDS),
        ("LIGHT_DIRECTIONAL", LIGHT_DIRECTIONAL),
        ("LIGHT_POINT", LIGHT_POINT),
        ("LIGHT_SPOT", LIGHT_SPOT),
    ] {
        assert_eq!(literal(&module, name), Literal::U32(value), "`{name}`");
    }
    assert_eq!(
        literal(&module, "MAX_SHADOW_RADIUS"),
        Literal::F32(lumen_3d::MAX_FILTER_TEXELS),
    );
    assert_eq!(
        literal(&module, "BRDF_TABLE_SIZE"),
        Literal::U32(lumen_3d::environment::BRDF_TABLE_SIZE as u32),
    );
    // The shader reads a skinned vertex's joints and weights, and a vertex's extras, as 4 words.
    assert_eq!(size_of::<lumen_3d::SkinVertex>(), 16);
    assert_eq!(size_of::<lumen_3d::VertexExtra>(), 16);
}
