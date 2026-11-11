//! The builtin function table. The parser, evaluator and generated schema all read from it.

/// The type of a builtin argument or result, as published in the schema.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValueKind {
    Number,
    Vec2,
    Vec3,
    Color,
    String,
    /// A string, or a number shown as text.
    Text,
    Easing,
    Key,
    /// Any value but an easing or a key.
    Any,
}

impl ValueKind {
    pub const fn name(self) -> &'static str {
        match self {
            Self::Number => "number",
            Self::Vec2 => "vec2",
            Self::Vec3 => "vec3",
            Self::Color => "color",
            Self::String => "string",
            Self::Text => "text",
            Self::Easing => "easing",
            Self::Key => "key",
            Self::Any => "any",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinArg {
    pub name: &'static str,
    pub kind: ValueKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BuiltinSpec {
    pub name: &'static str,
    pub builtin: BuiltinFn,
    pub min_args: usize,
    /// `None` means the last argument repeats.
    pub max_args: Option<usize>,
    pub args: &'static [BuiltinArg],
    pub returns: ValueKind,
    pub doc: &'static str,
}

impl BuiltinSpec {
    pub fn accepts_arity(&self, count: usize) -> bool {
        count >= self.min_args && self.max_args.is_none_or(|max| count <= max)
    }

    pub fn arity_description(&self) -> String {
        match self.max_args {
            None => format!("at least {}", self.min_args),
            Some(max) if max == self.min_args => max.to_string(),
            Some(max) => format!("{} to {max}", self.min_args),
        }
    }
}

/// Declares `BuiltinFn`, `BUILTINS` and `NamedEasing` together so none can drift from the others.
/// Each easing is a builtin of the same name, and `'linear'` and `'hold'` are easing names with no
/// builtin.
macro_rules! builtins {
    (
        functions {$(
            $variant:ident $name:literal ($min:literal, $max:expr)
            [$($arg:ident: $kind:ident),*] -> $returns:ident, $doc:literal;
        )*}
        easings {$($easing:ident $easing_name:literal, $easing_doc:literal;)*}
    ) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum BuiltinFn {
            $($variant,)*
            $($easing,)*
        }

        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum NamedEasing {
            Linear,
            Hold,
            $($easing,)*
        }

        pub const BUILTINS: &[BuiltinSpec] = &[
            $(BuiltinSpec {
                name: $name,
                builtin: BuiltinFn::$variant,
                min_args: $min,
                max_args: $max,
                args: &[$(BuiltinArg { name: stringify!($arg), kind: ValueKind::$kind }),*],
                returns: ValueKind::$returns,
                doc: $doc,
            },)*
            $(BuiltinSpec {
                name: $easing_name,
                builtin: BuiltinFn::$easing,
                min_args: 1,
                max_args: Some(1),
                args: &[BuiltinArg { name: "t", kind: ValueKind::Number }],
                returns: ValueKind::Number,
                doc: $easing_doc,
            },)*
        ];

        impl NamedEasing {
            pub const ALL: [Self; 2 + [$($easing_name),*].len()] =
                [Self::Linear, Self::Hold, $(Self::$easing),*];

            pub const fn name(self) -> &'static str {
                match self {
                    Self::Linear => "linear",
                    Self::Hold => "hold",
                    $(Self::$easing => $easing_name,)*
                }
            }

            /// The easing a builtin such as `out_expo(t)` applies.
            pub fn for_builtin(builtin: BuiltinFn) -> Option<Self> {
                match builtin {
                    $(BuiltinFn::$easing => Some(Self::$easing),)*
                    _ => None,
                }
            }
        }

        /// A pattern matching every easing builtin.
        macro_rules! easing_builtins {
            () => { $(BuiltinFn::$easing)|* };
        }
        pub(super) use easing_builtins;
    };
}

builtins! {
    functions {
    Min "min" (2, None) [values: Number] -> Number, "Smallest of two or more numbers.";
    Max "max" (2, None) [values: Number] -> Number, "Largest of two or more numbers.";
    Abs "abs" (1, Some(1)) [value: Number] -> Number, "Absolute value.";
    Floor "floor" (1, Some(1)) [value: Number] -> Number, "Rounds down to an integer.";
    Ceil "ceil" (1, Some(1)) [value: Number] -> Number, "Rounds up to an integer.";
    Round "round" (1, Some(1)) [value: Number] -> Number, "Rounds to the nearest integer.";
    Sin "sin" (1, Some(1)) [radians: Number] -> Number, "Sine of an angle in radians.";
    Cos "cos" (1, Some(1)) [radians: Number] -> Number, "Cosine of an angle in radians.";
    Clamp "clamp" (3, Some(3)) [value: Number, min: Number, max: Number] -> Number,
        "Limits a value to `min..=max`.";
    Lerp "lerp" (3, Some(3)) [from: Any, to: Any, t: Number] -> Any,
        "Interpolates two numbers, vec2 or vec3 values, colours or path data strings by `t`. Colours mix premultiplied. Path data strings must have the same commands in the same order, and interpolate point by point.";
    Pow "pow" (2, Some(2)) [base: Number, exponent: Number] -> Number, "Raises `base` to `exponent`.";
    Mod "mod" (2, Some(2)) [value: Number, divisor: Number] -> Number,
        "Remainder of `value / divisor`, keeping the sign of `value`.";
    Fract "fract" (1, Some(1)) [value: Number] -> Number, "Fractional part of a number.";
    Sign "sign" (1, Some(1)) [value: Number] -> Number, "-1, 0 or 1 by the sign of a number.";
    Atan2 "atan2" (2, Some(2)) [y: Number, x: Number] -> Number,
        "Angle in radians of the point `(x, y)` from the +x axis, in `-π..=π`.";
    Hypot "hypot" (2, Some(2)) [x: Number, y: Number] -> Number,
        "Length of the vector `(x, y)`.";
    Remap "remap" (5, Some(5))
        [value: Number, from_min: Number, from_max: Number, to_min: Number, to_max: Number] -> Number,
        "Maps `value` from `from_min..from_max` onto `to_min..to_max` without clamping. `from_min` and `from_max` must differ.";
    Smoothstep "smoothstep" (3, Some(3)) [edge0: Number, edge1: Number, x: Number] -> Number,
        "Hermite smoothing of `x` between two edges.";
    Step "step" (3, Some(3)) [from: Number, to: Number, t: Number] -> Number,
        "`to` once `t >= 1`, otherwise `from`.";
    TextHeight "text_height" (1, Some(4))
        [text: Any, font_size: Number, max_width: Number, font_family: String] -> Number,
        "Laid-out height of a string, or of `node(id)` for a text node.";
    TextWidth "text_width" (1, Some(4))
        [text: Any, font_size: Number, max_width: Number, font_family: String] -> Number,
        "Laid-out width of a string, or of `node(id)` for a text node.";
    Uppercase "uppercase" (1, Some(1)) [text: String] -> String, "Uppercases a string.";
    Lowercase "lowercase" (1, Some(1)) [text: String] -> String, "Lowercases a string.";
    Format "format" (1, Some(4)) [value: Number, decimals: Number, width: Number, fill: String] -> String,
        "A number as text with exactly `decimals` digits after the point (default 0), so `format(4.3, 2)` is `'4.30'`. Halves round away from zero on the number as written: `format(2.5)` is `'3'`, `format(-2.5)` is `'-3'` and `format(1.005, 2)` is `'1.01'`. A result shorter than `width` characters (default 0) is padded on the left with `fill`, one Unicode code point (default a space); with a `'0'` fill the sign stays first, so `format(-5, 0, 4, '0')` is `'-005'`. `value` must be finite, `decimals` a whole number in `0..=20` and `width` a whole number in `0..=64`.";
    Concat "concat" (1, None) [parts: Text] -> String,
        "Joins strings and numbers into one string. A number shows as it does in a text param, so `concat('frame ', frame / 7)` reads like `=frame / 7`; use `format` for a fixed number of decimals or padding, e.g. `concat('frame ', format(frame, 0, 4, '0'), ' / 1200')`. `+` stays numeric. The result can be at most 1 MiB.";
    Vec2 "vec2" (2, Some(2)) [x: Number, y: Number] -> Vec2, "Builds a 2D vector.";
    Vec3 "vec3" (3, Some(3)) [x: Number, y: Number, z: Number] -> Vec3, "Builds a 3D vector.";
    Rgb "rgb" (3, Some(3)) [r: Number, g: Number, b: Number] -> Color,
        "Opaque colour from components in `0..=255`.";
    Rgba "rgba" (4, Some(4)) [r: Number, g: Number, b: Number, a: Number] -> Color,
        "Colour from components in `0..=255`, alpha included.";
    Progress "progress" (2, Some(2)) [start: Number, end: Number] -> Number,
        "0 before frame `start`, 1 after frame `end`, linear in between.";
    Tween "tween" (4, Some(5))
        [from: Any, to: Any, start: Number, duration: Number, ease: Easing] -> Any,
        "Eases from `from` to `to` over `duration` frames from frame `start`. `ease` defaults to `'linear'`.";
    Ease "ease" (2, Some(2)) [easing: Easing, t: Number] -> Number,
        "Applies an easing to progress `t`, e.g. `ease('out_expo', progress(0, 18))`.";
    CubicBezier "cubic_bezier" (2, Some(2)) [out: Vec2, in: Vec2] -> Easing,
        "The easing CSS writes `cubic-bezier(out.x, out.y, in.x, in.y)`. Both x components must be in `0..=1`.";
    Key "key" (2, Some(3)) [at: Number, value: Any, ease: Easing] -> Key,
        "A keyframe for `keyframes`: `value` at `at`, a number, vec2, vec3, colour or path data. `ease` is how the curve leaves this key toward the next.";
    Keyframes "keyframes" (2, None) [t: Number, ease: Easing, keys: Key] -> Any,
        "The value of keys ordered by `at` at time `t`, usually `frame`. `ease` may be left out; it is the easing of keys without one and defaults to `'linear'`. Before the first key and after the last, the value holds. Keys at the same `at` jump.";
    Prop "prop" (1, Some(1)) [name: String] -> Any,
        "The value of a composition template's prop, e.g. `prop('title')`. Only inside a template.";
    Noise "noise" (2, Some(2)) [seed: Number, x: Number] -> Number,
        "Smooth gradient noise in `-1..=1`, continuous in `x` and varying about once per unit. Each `seed` is an unrelated curve. Arguments must be finite.";
    Random "random" (1, Some(2)) [seed: Number, x: Number] -> Number,
        "Uniform number in `0..1`, fixed by its inputs. `random(x)` is `random(seed, x)` with seed 0; e.g. `random(glyph)`. Arguments must be finite.";
    Spring "spring" (1, Some(3)) [t: Number, stiffness: Number, damping: Number] -> Number,
        "Spring from 0 to 1, `t` seconds after release, e.g. `spring((frame - 12) / fps)`. Stiffness defaults to 170 and damping to 26.";
    PathPoint "path_point" (2, Some(2)) [path_data: String, t: Number] -> Vec2,
        "The point `t` of the way along path data by arc length, so that `t` moving steadily moves the point at a steady speed. `t` is clamped to `0..=1`. Moves between contours add no length, and a closed contour's closing segment does.";
    PathAngle "path_angle" (2, Some(2)) [path_data: String, t: Number] -> Number,
        "The direction in degrees that path data heads at `path_point(path_data, t)`, clockwise from +x like `rotation`. It counts whole turns along the path instead of wrapping, so a clockwise circle ends at 360.";
    }
    easings {
    InQuad "in_quad", "Quadratic ease-in.";
    OutQuad "out_quad", "Quadratic ease-out.";
    InOutQuad "in_out_quad", "Quadratic ease-in-out.";
    InCubic "in_cubic", "Cubic ease-in.";
    OutCubic "out_cubic", "Cubic ease-out.";
    InOutCubic "in_out_cubic", "Cubic ease-in-out.";
    InExpo "in_expo", "Exponential ease-in.";
    OutExpo "out_expo", "Exponential ease-out.";
    InOutExpo "in_out_expo", "Exponential ease-in-out.";
    InBack "in_back", "Ease-in that pulls back first.";
    OutBack "out_back", "Ease-out that overshoots.";
    InOutBack "in_out_back", "Ease-in-out that pulls back and overshoots.";
    InSine "in_sine", "Sinusoidal ease-in.";
    OutSine "out_sine", "Sinusoidal ease-out.";
    InOutSine "in_out_sine", "Sinusoidal ease-in-out.";
    InQuart "in_quart", "Quartic ease-in.";
    OutQuart "out_quart", "Quartic ease-out.";
    InOutQuart "in_out_quart", "Quartic ease-in-out.";
    InQuint "in_quint", "Quintic ease-in.";
    OutQuint "out_quint", "Quintic ease-out.";
    InOutQuint "in_out_quint", "Quintic ease-in-out.";
    InCirc "in_circ", "Circular ease-in.";
    OutCirc "out_circ", "Circular ease-out.";
    InOutCirc "in_out_circ", "Circular ease-in-out.";
    InElastic "in_elastic", "Ease-in that oscillates like a spring.";
    OutElastic "out_elastic", "Ease-out that overshoots and oscillates like a spring.";
    InOutElastic "in_out_elastic", "Ease-in-out that oscillates at both ends.";
    InBounce "in_bounce", "Ease-in that bounces.";
    OutBounce "out_bounce", "Ease-out that bounces like a dropped ball.";
    InOutBounce "in_out_bounce", "Ease-in-out that bounces at both ends.";
    }
}

pub fn builtin_for_name(name: &str) -> Option<&'static BuiltinSpec> {
    BUILTINS.iter().find(|spec| spec.name == name)
}

impl BuiltinFn {
    pub fn spec(self) -> &'static BuiltinSpec {
        &BUILTINS[self as usize]
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GlobalSpec {
    pub name: &'static str,
    pub doc: &'static str,
}

/// Declares `GlobalVar` and `GLOBALS` together so neither can drift from the other.
macro_rules! globals {
    ($($variant:ident $name:literal, $doc:literal;)*) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum GlobalVar {
            $($variant),*
        }

        pub const GLOBALS: &[GlobalSpec] = &[$(GlobalSpec { name: $name, doc: $doc }),*];

        impl GlobalVar {
            pub fn from_name(name: &str) -> Option<Self> {
                match name {
                    $($name => Some(Self::$variant),)*
                    _ => None,
                }
            }

            pub const fn name(self) -> &'static str {
                match self {
                    $(Self::$variant => $name),*
                }
            }
        }
    };
}

globals! {
    Frame "frame", "Current frame number.";
    Time "time", "Current time in seconds.";
    Fps "fps", "Frames per second.";
    Width "width", "Render width in pixels.";
    Height "height", "Render height in pixels.";
    Glyph "glyph", "Index of the glyph being drawn, counting spaces. Only in a text node's `glyph_*` params.";
    Word "word", "Index of the word being drawn. Only in a text node's `glyph_*` params.";
    Line "line", "Index of the line being drawn. Only in a text node's `glyph_*` params.";
    Span "span", "Index of the span being drawn, or 0 in plain text. Only in a text node's `glyph_*` params.";
    GlyphCount "glyph_count", "Number of glyphs in the text. Only in a text node's `glyph_*` params.";
    WordCount "word_count", "Number of words in the text. Only in a text node's `glyph_*` params.";
    LineCount "line_count", "Number of lines in the text. Only in a text node's `glyph_*` params.";
    SpanCount "span_count", "Number of spans in the text, or 1 in plain text. Only in a text node's `glyph_*` params.";
}

/// A call the parser handles itself instead of through `BUILTINS`, listed for documentation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FormSpec {
    pub name: &'static str,
    pub signature: &'static str,
    pub doc: &'static str,
}

pub const FORMS: &[FormSpec] = &[
    FormSpec {
        name: "if",
        signature: "if(condition, then, else)",
        doc: "`then` when `condition` is truthy, otherwise `else`. Only the chosen branch is evaluated.",
    },
    FormSpec {
        name: "node",
        signature: "node(id, 'property')",
        doc: "The value of another node's param. `node(id)` alone names a text node for `text_width` and `text_height`.",
    },
];

/// The names composition functions and their parameters may not take.
pub fn reserved_names() -> impl Iterator<Item = &'static str> {
    ["true", "false"]
        .into_iter()
        .chain(FORMS.iter().map(|form| form.name))
        .chain(BUILTINS.iter().map(|spec| spec.name))
        .chain(GLOBALS.iter().map(|global| global.name))
}

#[cfg(test)]
mod tests {
    use super::{BUILTINS, builtin_for_name};

    #[test]
    fn every_builtin_appears_exactly_once() {
        for spec in BUILTINS {
            let same_name = BUILTINS.iter().filter(|other| other.name == spec.name);
            assert_eq!(same_name.count(), 1, "{}", spec.name);
            assert_eq!(builtin_for_name(spec.name), Some(spec));
            assert_eq!(spec.builtin.spec(), spec);
            match spec.max_args {
                Some(max) => assert!(max == spec.args.len() && spec.min_args <= max),
                None => assert!(!spec.args.is_empty()),
            }
        }
    }
}
