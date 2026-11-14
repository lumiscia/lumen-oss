//! Compares the structs and constants that Rust and the shaders each declare, which the compiler
//! can't: a mismatch only shows up as wrong pixels. Structs deriving `ShaderLayout` carry their own
//! test, which calls `assert_layout`.

use std::mem::{offset_of, size_of};

use lumen_text::{GpuGlyphInstance, GpuTextGlobals};
use naga::{ArraySize, Expression, Literal, Module, TypeInner};

use crate::node::{motion::MAX_MOTION_TAPS, source::text_record::shared_text_shader};

fn module(source: &str) -> Module {
    naga::front::wgsl::parse_str(source).expect("the shader should parse")
}

/// Whether the shader's `member` is padding, which Rust names `_…` and may leave out of its fields.
fn is_padding(member: &str) -> bool {
    member.starts_with('_') || member == "padding"
}

/// Asserts that the struct `wgsl` in `source` is `size` bytes, that each of the Rust `fields`,
/// given as a name and an offset, is a member at that offset, and that every member that isn't
/// padding is one of them.
pub(crate) fn assert_layout<S: AsRef<str> + ?Sized>(
    source: &S,
    wgsl: &str,
    size: usize,
    fields: &[(&str, usize)],
) {
    let module = module(source.as_ref());
    let (members, span) = module
        .types
        .iter()
        .find_map(|(_, ty)| match &ty.inner {
            TypeInner::Struct { members, span } if ty.name.as_deref() == Some(wgsl) => {
                Some((members, *span))
            }
            _ => None,
        })
        .unwrap_or_else(|| panic!("the shader has no struct `{wgsl}`"));
    assert_eq!(span as usize, size, "size of `{wgsl}`");
    for member in members {
        let name = member.name.as_deref().expect("struct members are named");
        if is_padding(name) {
            continue;
        }
        let (_, offset) = fields
            .iter()
            .find(|(field, _)| *field == name)
            .unwrap_or_else(|| panic!("`{wgsl}.{name}` has no Rust field"));
        assert_eq!(member.offset as usize, *offset, "offset of `{wgsl}.{name}`");
    }
    for (field, _) in fields {
        assert!(
            members
                .iter()
                .any(|member| member.name.as_deref() == Some(field)),
            "`{wgsl}` has no member `{field}`"
        );
    }
}

#[test]
fn the_text_crates_structs_match_the_shader() {
    assert_layout(
        shared_text_shader(),
        "TextGlobals",
        size_of::<GpuTextGlobals>(),
        &[
            ("target_size", offset_of!(GpuTextGlobals, target_size)),
            ("px_range", offset_of!(GpuTextGlobals, px_range)),
            ("glyph_count", offset_of!(GpuTextGlobals, glyph_count)),
        ],
    );
    assert_layout(
        shared_text_shader(),
        "GlyphInstance",
        size_of::<GpuGlyphInstance>(),
        &[
            ("rect", offset_of!(GpuGlyphInstance, rect)),
            ("uv_rect", offset_of!(GpuGlyphInstance, uv_rect)),
            ("opacity", offset_of!(GpuGlyphInstance, opacity)),
            ("partner", offset_of!(GpuGlyphInstance, partner)),
            ("mode", offset_of!(GpuGlyphInstance, mode)),
            ("flags", offset_of!(GpuGlyphInstance, flags)),
            ("offset", offset_of!(GpuGlyphInstance, offset)),
            ("linear", offset_of!(GpuGlyphInstance, linear)),
        ],
    );
}

#[test]
fn drawable_motion_holds_the_rust_tap_count() {
    let module = module(shared_text_shader());
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
    let module = module(shared_text_shader());
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
fn scene_shader_matches_its_rust_constants_and_the_3d_crates_layout() {
    use crate::node::scene::render::{
        FLAG_BASE_COLOR_MAP, FLAG_BLEND, FLAG_EMISSIVE_MAP, FLAG_FLAT_NORMALS, FLAG_MAP_UVS,
        FLAG_MASK, FLAG_METALLIC_ROUGHNESS_MAP, FLAG_MIRRORED, FLAG_NORMAL_MAP, FLAG_OCCLUSION_MAP,
        FLAG_PART, FLAG_RASTER, FLAG_RECEIVE_SHADOWS, FLAG_SHADOW_CATCHER, FLAG_SKINNED,
        FLAG_UNLIT, FLAG_VERTEX_COLORS, LIGHT_DIRECTIONAL, LIGHT_POINT, LIGHT_SPOT,
        MORPH_DELTA_WORDS, SHADER,
    };

    assert_layout(
        SHADER,
        "Vertex",
        size_of::<lumen_3d::Vertex>(),
        &[
            ("position", offset_of!(lumen_3d::Vertex, position)),
            ("normal", offset_of!(lumen_3d::Vertex, normal)),
            ("uv", offset_of!(lumen_3d::Vertex, uv)),
        ],
    );
    let module = module(SHADER);
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
        ("FLAG_PART", FLAG_PART),
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
