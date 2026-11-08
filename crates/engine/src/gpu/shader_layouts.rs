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
        include_str!("../node/vector/paint/paint.wgsl"),
        DRAWABLE_SHADER,
        lumen_text::TEXT_SHADER,
    ]
    .concat();
    naga::front::wgsl::parse_str(&source).expect("the text shader should parse")
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
    let module = text_module();
    for (name, size, offsets) in layouts() {
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
