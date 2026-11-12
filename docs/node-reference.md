# Node reference

Every node kind, with its inputs and params, then the types they share. This file is generated from `definitions/composition.schema.json`: run `just generate-types` to update it.

A param or field takes an expression, a string that starts with `=`, unless it is marked. **Literal only** means the value is a plain JSON value. **Literal as a whole** means an object, array or map is never an expression itself, but its fields or elements are marked the same way: in a one-line type such as `{ offset: number }` or `array of (number | expression)`, a field or element takes an expression only where `expression` is among its types. Defaults are the JSON values a node uses when it leaves a param out.

## `background`

Generates a background raster texture.

Category: source. Output: raster.

Params:

- **`height`** (integer, at least 0; default `0`; literal only) Output height in pixels. Use 0 to match the composition height.
- **`paint`** ([paint](#type-paint); default `[0,0,0,255]`) Background paint.
- **`paint_supersample`** (boolean; default `true`) Enables 4×4 supersampling when evaluating paint at each pixel.
- **`width`** (integer, at least 0; default `0`; literal only) Output width in pixels. Use 0 to match the composition width.

## `fractal_noise`

Generates opaque grey fractal noise, the same for a given seed and evolution.

Category: source. Output: raster.

Params:

- **`brightness`** (number, -1 to 1; default `0`) Added to every pixel after contrast.
- **`contrast`** (number, at least 0; default `1`) Contrast multiplier around mid grey.
- **`evolution`** (number; default `0`) Morphs the pattern smoothly, in turns: it repeats every whole turn, so `=time` loops once a second.
- **`height`** (integer, at least 0; default `0`; literal only) Output height in pixels. Use 0 to match the composition height.
- **`mode`** (`"smooth"` | `"turbulent"`; default `"smooth"`) How the layers are summed.
- **`octaves`** (integer, 1 to 8; default `4`) Layers of detail, from 1 to 8. Each adds features half the size, at half the strength.
- **`offset_x`** (number; default `0`) Moves the pattern right by this many pixels.
- **`offset_y`** (number; default `0`) Moves the pattern down by this many pixels.
- **`scale`** (number, at least 1; default `64`) Size of the largest features in pixels.
- **`seed`** (integer; default `0`) Picks an unrelated pattern.
- **`width`** (integer, at least 0; default `0`; literal only) Output width in pixels. Use 0 to match the composition width.

## `media_in`

Binds an external image or video frame as a GPU texture.

Category: source. Output: raster.

Params:

- **`alpha`** (`"straight"` | `"premultiplied"`; default `"straight"`) Whether a video's colour is already multiplied by its alpha. Images ignore it.
- **`kind`** (`"image"` | `"video"`; default `"image"`) Type of external media source.
- **`loop_mode`** (`"clamp"` | `"repeat"` | `"ping_pong"`; default `"clamp"`) Behavior when playback leaves the source range.
- **`range_end`** (integer, at least 0) First source frame past the range; the source's end when unset.
- **`range_start`** (integer, at least 0; default `0`) First source frame to include.
- **`source`** (string; media: image, video, chosen by `kind`; default `""`) External media source: a path or `media` alias, or `asset:<id>` for an asset of the host's.
- **`speed`** (number; default `1`) Playback speed multiplier.

## `text`

Produces a text source.

Category: source. Output: raster.

Params:

- **`alignment_horizontal`** (`"left"` | `"center"` | `"right"` | `"justify"`; default `"left"`; literal only) Horizontal text alignment.
- **`alignment_vertical`** (`"top"` | `"middle"` | `"bottom"`; default `"top"`; literal only) Vertical text alignment.
- **`anchor`** ([number, number]; default `[0,0]`) Pivot for scale and rotation, in pixels relative to `position`.
- **`color`** ([paint](#type-paint); default `[255,255,255,255]`) Text color, and that of spans without their own.
- **`content`** ([textContent](#type-textcontent); default `""`) Text to render: a string, or spans that each override the style these params give. A node draws at most 16384 glyphs unless the host sets another limit, counting a stroked glyph twice and each decoration rect once; whitespace is not drawn, and more is a render error.
- **`fit`** (`"none"` | `"shrink"`; default `"none"`; literal only) Whether the text shrinks to fit `max_width`, `max_lines` and `max_height` without breaking a word, before any lines are dropped. Every font size, letter spacing, `line_height`, baseline shift, paragraph spacing, decoration size and stroke width scales by the largest factor up to 1 that fits.
- **`font_family`** (string; default `"Roboto"`) Font family name, or `asset:<id>` for a font asset of the host's.
- **`font_features`** ([fontFeatures](#type-fontfeatures); default `{}`; literal as a whole) OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0 }`. 0 turns a feature off, 1 turns it on and larger values select an alternate. Values take literals or expressions, but the whole map cannot be an expression.
- **`font_size`** (number, at least 1; default `16`) Font size in pixels.
- **`font_style`** (`"normal"` | `"italic"` | `"oblique"`; default `"normal"`) Font style.
- **`font_weight`** (integer, 1 to 1000; default `400`) Font weight. Variable fonts render any value in their weight range.
- **`glyph_offset`** ([number, number]; default `[0,0]`) Offset of each glyph in pixels. The `glyph_*` params are evaluated for every glyph, and can read its `glyph`, `word`, `line` and `span` index and their counts.
- **`glyph_opacity`** (number, 0 to 1; default `1`) Opacity multiplier of each glyph, applied before `opacity`.
- **`glyph_rotation`** (number; default `0`) Clockwise rotation of each glyph in degrees, about the middle of its advance and line.
- **`glyph_scale`** ([number, number]; default `[1,1]`) Scale multiplier of each glyph per axis, about the middle of its advance and line.
- **`highlight`** ([textHighlight](#type-texthighlight); literal as a whole) Box behind each line of the text, and of spans that leave theirs unset. Off when absent.
- **`letter_spacing`** (number; default `0`) Extra space after every glyph in pixels. Negative values tighten the text.
- **`line_height`** (number, at least 0; default `0`) Distance between lines in pixels. Use 0 for 1.2 × the font size.
- **`max_height`** (number, at least 0) Most height in pixels of the lines to show. Unlimited when absent.
- **`max_lines`** (integer, at least 0) Most lines to show. Unlimited when absent.
- **`max_width`** (number, at least 0; default `0`) Maximum line width in pixels. Use 0 for automatic width.
- **`min_font_size`** (number, at least 1; default `8`) Smallest `font_size` that `fit` shrinks the text to. Lines that still do not fit are dropped by `overflow`.
- **`motion_blur`** (number, 0 to 360; default `0`) Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the usual film look and 360 smears over the whole frame. Blurs the text's own movement, not that of the `glyph_*` params.
- **`opacity`** (number, 0 to 1; default `1`) Opacity multiplier.
- **`overflow`** (`"clip"` | `"ellipsis"`; default `"clip"`; literal only) How the last line shown ends when lines past `max_lines` or `max_height` are dropped: as it is, or in an ellipsis. Clusters at that line's end are dropped until the ellipsis fits `max_width`. The ellipsis takes the style and paint of the text it follows, and is one glyph, numbered after the last one shown.
- **`paint_space`** (`"span"` | `"text"`; default `"span"`; literal only) Box a span's own gradient `color` spans: the span's lines, or the whole text. The node's `color` always spans the whole text.
- **`paint_supersample`** (boolean; default `true`) Enables 4×4 supersampling when evaluating paint at each glyph pixel.
- **`paragraph_spacing`** (number, at least 0; default `0`) Extra space in pixels after each paragraph, but the last. A line break starts a new paragraph.
- **`position`** ([number, number]; default `[0,0]`) Text origin in pixels.
- **`render_mode`** (`"raster"` | `"msdf"` | `"auto"`; default `"raster"`; literal only) Glyph rendering mode. MSDF reuses outline distance fields across sizes; Auto is MSDF from 512 px.
- **`rotation`** (number; default `0`) Clockwise rotation in degrees, about the anchor.
- **`scale`** ([number, number]; default `[1,1]`) Scale multiplier per axis, about the anchor. Raster glyphs are resampled, so use MSDF rendering to keep scaled text sharp.
- **`strikethrough`** ([textLine](#type-textline); literal as a whole) Line through the text, and through spans that leave theirs unset. Off when absent.
- **`stroke`** ([textStroke](#type-textstroke); literal as a whole) Outline along the glyphs, and along those of spans that leave theirs unset. Off when absent. It does not change the layout or the measured size. `glyph_opacity` fades a glyph's stroke and fill as one.
- **`text_transform`** (`"none"` | `"uppercase"` | `"lowercase"` | `"capitalize"`; default `"none"`) Case applied to the text, and to spans that leave theirs unset.
- **`underline`** ([textLine](#type-textline); literal as a whole) Line under the text, and under spans that leave theirs unset. Off when absent. Its fields take literals or expressions, but the whole line cannot be an expression.

## `path`

Produces a rasterized vector path source.

Category: vector. Output: raster.

Params:

- **`anchor`** ([number, number]; default `[0,0]`) Pivot for scale and rotation, in pixels relative to `position`.
- **`data`** (string; default `"M 0 0 L 100 0 L 100 100 L 0 100 Z"`) SVG-style path data.
- **`edge_antialias`** (boolean; default `true`) Enables analytic distance-based edge antialiasing.
- **`feather`** (number, at least 0; default `0`) Width in untransformed pixels over which the fill's edge fades out, centred on the edge. 0 keeps a sharp edge.
- **`fill_enabled`** (boolean; default `true`) Enables fill rendering.
- **`fill_paint`** ([paint](#type-paint); default `[255,255,255,255]`) Fill paint. Accepts a solid color or gradient.
- **`motion_blur`** (number, 0 to 360; default `0`) Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the usual film look and 360 smears over the whole frame.
- **`offset`** (number; default `0`) Distance in untransformed pixels that the fill grows outward by, or shrinks inward by when negative, rounding the corners it grows around. A shape shrunk past its middle vanishes. The stroke stays on the outline in `data`.
- **`opacity`** (number, 0 to 1; default `1`) Opacity multiplier.
- **`position`** ([number, number]; default `[0,0]`) Path origin in pixels.
- **`repeat`** ([repeat](#type-repeat); literal as a whole) Copies of the path drawn in one pass. Absent draws the path once.
- **`rotation`** (number; default `0`) Clockwise rotation in degrees, about the anchor.
- **`scale`** ([number, number]; default `[1,1]`) Scale multiplier per axis, about the anchor.
- **`stroke_dash`** ([dashPattern](#type-dashpattern); default `[]`; literal as a whole) Lengths in untransformed pixels that the stroke alternately draws and skips along each contour, starting with a dash. An odd number of lengths is repeated to make the pattern, as in SVG. Each dash has round caps, so a dash of length 0 draws a dot. No lengths, or only zeros, draw a solid stroke. Dashes are measured along the whole contour, and `trim_start` and `trim_end` then clip them.
- **`stroke_dash_offset`** (number; default `0`) Distance in untransformed pixels into the dash pattern at which each contour starts. Increasing it moves the dashes toward the start of the contour.
- **`stroke_enabled`** (boolean; default `false`) Enables stroke rendering.
- **`stroke_paint`** ([paint](#type-paint); default `[0,0,0,255]`) Stroke paint. Accepts a solid color or gradient.
- **`stroke_width`** (number, at least 0; default `1`) Stroke width in untransformed pixels. Under non-uniform scale the stroke stretches with the path, so it is wider across the more scaled axis.
- **`trim_end`** (number, 0 to 1; default `1`) Normalized end of the visible stroke range for each contour.
- **`trim_start`** (number, 0 to 1; default `0`) Normalized start of the visible stroke range for each contour.

## `shape`

Produces a vector shape layer for GPU rasterization.

Category: vector. Output: raster.

Params:

- **`anchor`** ([number, number]; default `[0,0]`) Pivot for scale and rotation, in pixels relative to `position`.
- **`border_radius`** (number, at least 0; default `0`) Corner radius for rectangle geometry.
- **`edge_antialias`** (boolean; default `true`) Enables analytic distance-based edge antialiasing.
- **`fill_enabled`** (boolean; default `true`) Enables fill rendering.
- **`fill_paint`** ([paint](#type-paint); default `[255,255,255,255]`) Fill paint. Accepts a solid color or gradient.
- **`geometry_kind`** (`"rectangle"` | `"ellipse"`; default `"rectangle"`; literal only) Geometry primitive to rasterize.
- **`height`** (number, at least 0; default `1`) Shape height in pixels; 0 draws nothing and anything thinner than a pixel draws a pixel tall.
- **`motion_blur`** (number, 0 to 360; default `0`) Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the usual film look and 360 smears over the whole frame.
- **`opacity`** (number, 0 to 1; default `1`) Opacity multiplier.
- **`position`** ([number, number]; default `[0,0]`) Shape origin in pixels.
- **`repeat`** ([repeat](#type-repeat); literal as a whole) Copies of the shape drawn in one pass. Absent draws the shape once.
- **`rotation`** (number; default `0`) Clockwise rotation in degrees, about the anchor.
- **`scale`** ([number, number]; default `[1,1]`) Scale multiplier per axis, about the anchor.
- **`stroke_enabled`** (boolean; default `false`) Enables stroke rendering.
- **`stroke_paint`** ([paint](#type-paint); default `[0,0,0,255]`) Stroke paint. Accepts a solid color or gradient.
- **`stroke_width`** (number, at least 0; default `1`) Stroke width in untransformed pixels. Under non-uniform scale the stroke stretches with the shape, so it is wider across the more scaled axis.
- **`width`** (number, at least 0; default `1`) Shape width in pixels; 0 draws nothing and anything thinner than a pixel draws a pixel wide.

## `blur`

Applies a separable Gaussian blur to a raster.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`radius`** (number, at least 0; default `4`) Blur radius in pixels. Fractional radii blend smoothly. Radii above about 588 look the same.

## `color_grade`

Applies a LUT-driven color transform to a raster, looking up its sRGB-encoded colour.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`interpolation`** (`"nearest"` | `"linear"`; default `"linear"`) Sampling filter used when reading LUT data.
- **`lut`** (string; default `"identity"`) An inline 1D LUT: `rgb1d:` and RGB stops separated by `;`, each three numbers from 0 to 1, such as `rgb1d:0,0,0;0.2,0.1,0.3;1,1,1`; or `identity`. A value outside 0..1 is an error: divide 0..255 values by 255. Set either this or `lut_source`.
- **`lut_source`** (string; media: lut; default `""`) A 3D `.cube` LUT file (Adobe/Resolve) to grade with: a path or `media` alias, or `asset:<id>` for an asset of the host's. Tables up to 129 points per axis and 64 MiB are read, sampled with trilinear interpolation (`interpolation` `nearest` takes the nearest point), and `DOMAIN_MIN`/`DOMAIN_MAX` set the input range they cover. 1D `.cube` tables are not supported. Empty uses `lut`; setting both is an error.
- **`strength`** (number, 0 to 1; default `1`) Blend amount for the LUT transform.

## `crop`

Keeps a raster region in place and clears everything outside it.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`height`** (integer, at least 0; default `1`) Height of the crop region in pixels.
- **`width`** (integer, at least 0; default `1`) Width of the crop region in pixels.
- **`x`** (integer; default `0`) Left edge of the crop region in pixels.
- **`y`** (integer; default `0`) Top edge of the crop region in pixels.

## `directional_blur`

Blurs a raster along a line, as a straight motion blur.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`angle`** (number; default `0`) Direction of the blur in degrees, clockwise from horizontal.
- **`distance`** (number, at least 0; default `20`) Length in pixels of the line each pixel is spread along, centred on it.
- **`edge`** (`"clamp"` | `"transparent"`; default `"clamp"`) What the source reads as beyond the canvas.

## `displacement_map`

Moves each pixel of `source` by the `map` pixel over it, in canvas pixels. A channel byte is the sRGB code of the map's colour, and its move is scaled by the map's alpha, so transparent areas of the map, and everything outside it, leave the source in place.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required
- `map`: raster, required

Params:

- **`amount_x`** (number; default `10`) Pixels the source moves horizontally at full strength: a channel byte of 0 moves it this far left, 255 nearly as far right, and 128 leaves it in place. Negative amounts reverse it.
- **`amount_y`** (number; default `10`) Pixels the source moves vertically at full strength: a channel byte of 0 moves it this far up, 255 nearly as far down, and 128 leaves it in place. Negative amounts reverse it.
- **`channel_x`** (`"red"` | `"green"` | `"blue"` | `"luma"`; default `"red"`) Map channel that drives horizontal displacement.
- **`channel_y`** (`"red"` | `"green"` | `"blue"` | `"luma"`; default `"green"`) Map channel that drives vertical displacement.
- **`edge`** (`"clamp"` | `"transparent"`; default `"clamp"`) What the source reads as beyond its edges.
- **`sampling`** (`"nearest"` | `"linear"`; default `"linear"`) Sampling filter used to read the displaced source.

## `exposure`

Adjusts raster exposure, contrast, and offset.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`contrast`** (number, at least 0; default `1`) Contrast multiplier around sRGB-encoded mid grey, after the exposure.
- **`exposure`** (number; default `0`) Exposure in stops, multiplying linear light by 2 to the power of this.
- **`offset`** (number; default `0`) Offset added to the sRGB-encoded colour, after the exposure and before the contrast.

## `glow`

Adds a blurred copy of a raster's brightest pixels to it, as light: the glow's alpha is its brightest channel, so dark pixels add nothing.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`edge`** (`"clamp"` | `"transparent"`; default `"clamp"`) What the source reads as beyond the canvas.
- **`glow_only`** (boolean; default `false`) Outputs only the glow instead of adding it to the source.
- **`intensity`** (number, 0 to 1000; default `1`) Multiplies the glow's brightness, up to 1000.
- **`radius`** (number, at least 0; default `12`) Glow blur radius in pixels, as on `blur`. Radii above about 588 look the same.
- **`threshold`** (number, 0 to 1; default `0.6`) Luminance from 0 to 1 where the glow starts, on the sRGB-encoded scale, so 0.5 is half way up from black to white as they look. Brighter pixels glow more, white at full strength, and 1 lets nothing glow. Past 254/255 white dims too, so the glow fades out continuously as the threshold reaches 1.
- **`tint`** ([paint](#type-paint); default `[255,255,255,255]`) Glow colour when `tint_enabled` is set, at the brightness of the light that casts it. Must be a solid color; a gradient is an error.
- **`tint_enabled`** (boolean; default `false`) Colours the glow with `tint` instead of the colours that cast it.

## `hue_saturation`

Adjusts raster hue, saturation, and lightness of the sRGB-encoded colour.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`hue_degrees`** (number; default `0`) Hue rotation in degrees.
- **`lightness`** (number; default `0`) Lightness offset.
- **`saturation`** (number; default `1`) Saturation multiplier.

## `levels`

Remaps raster black, white, gamma, and output range of the sRGB-encoded colour.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`black_point`** (number, 0 to 1; default `0`) Input black point.
- **`gamma`** (number, at least 0.01; default `1`) Midtone gamma adjustment.
- **`output_black`** (number, 0 to 1; default `0`) Output black level.
- **`output_white`** (number, 0 to 1; default `1`) Output white level.
- **`white_point`** (number, 0 to 1; default `1`) Input white point.

## `motion_blur`

Blurs everything that moves in its source, including what analytic motion blur can't see: animated sizes, path data, motion upstream of a transform and motion inside video. Analytic motion blur in the source is off while it samples, and so is a `motion_blur` inside it. Content that already has its own motion blur, like a video recorded with it, is blurred twice.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`samples`** (integer, 1 to 64; default `8`) Sub-frames rendered across the shutter, from 1 to 64. Each renders the whole source again, so the cost grows with it. 1 renders the source once, at the frame.
- **`shutter`** (number, 0 to 360; default `180`) Shutter angle in degrees, centred on the frame, from 0 to 360. 0 is off, 180 is the usual film look and 360 smears over the whole frame.

## `opacity`

Uniformly adjusts raster opacity.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`opacity`** (number, 0 to 1; default `1`) Opacity multiplier applied to the raster.

## `radial_blur`

Blurs a raster along circles about a centre, as a spin.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`amount`** (number, 0 to 360; default `10`) Arc in degrees each pixel is spread along about `center`, centred on it, up to 360.
- **`center`** ([number, number]) Centre of the spin in canvas pixels. The canvas centre, `[width / 2, height / 2]`, when absent.
- **`edge`** (`"clamp"` | `"transparent"`; default `"clamp"`) What the source reads as beyond the canvas.

## `resize`

Resamples a raster into a rectangle placed on the composition canvas.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`height`** (integer, at least 1; default `1`) Output height in pixels.
- **`mode`** (`"stretch"` | `"fit"` | `"fill"`; default `"stretch"`; literal only) How the source raster should fit the output bounds.
- **`sampling`** (`"nearest"` | `"linear"`; default `"linear"`) Sampling filter used when resizing.
- **`width`** (integer, at least 1; default `1`) Output width in pixels.
- **`x`** (integer; default `0`) Left edge of the output rectangle in pixels.
- **`y`** (integer; default `0`) Top edge of the output rectangle in pixels.

## `shadow`

Composites a blurred alpha shadow behind a raster.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`color`** ([paint](#type-paint); default `[0,0,0,255]`) Shadow color. Must be a solid color; a gradient is an error.
- **`offset_x`** (number; default `8`) Horizontal shadow offset in pixels.
- **`offset_y`** (number; default `8`) Vertical shadow offset in pixels.
- **`opacity`** (number, 0 to 1; default `0.5`) Shadow opacity.
- **`radius`** (number, at least 0; default `8`) Shadow blur radius in pixels. Fractional radii blend smoothly. Radii above about 588 look the same.

## `time_remap`

Evaluates a raster input at another frame.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`frame`** (number, at least 0; default `0`) Source frame to sample.
- **`loop_enabled`** (boolean; default `false`) Enables looping between the configured loop bounds.
- **`loop_end`** (integer, at least 0; default `0`) Last frame in the loop range.
- **`loop_start`** (integer, at least 0; default `0`) First frame in the loop range.

## `transform`

Transforms a raster into the composition canvas bounds.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`custom_pivot`** (boolean; default `false`) Uses the pivot as given; when off, the pivot is the source centre.
- **`motion_blur`** (number, 0 to 360; default `0`) Motion blur shutter angle in degrees, centred on the frame. 0 is off, 180 is the usual film look and 360 smears over the whole frame.
- **`pivot_x`** (number; default `0`) Horizontal pivot point in pixels.
- **`pivot_y`** (number; default `0`) Vertical pivot point in pixels.
- **`rotate`** (number; default `0`) Rotation in degrees.
- **`sampling`** (`"nearest"` | `"linear"`; default `"linear"`) Sampling filter used when transforming.
- **`scale_x`** (number; default `1`) Horizontal scale multiplier.
- **`scale_y`** (number; default `1`) Vertical scale multiplier.
- **`translate_x`** (number; default `0`) Horizontal translation in pixels.
- **`translate_y`** (number; default `0`) Vertical translation in pixels.

## `wgsl_shader`

Runs a custom WGSL compute shader over one or more rasters.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required, takes several connections

Params:

- **`shader`** (string; default `""`) WGSL compute shader; the prelude provides inputs, `input_sampler`, `output` and `lumen`.
- **`uniforms`** ([shaderUniforms](#type-shaderuniforms); default `{}`; literal as a whole) Uniform struct field values by name; unset fields are zero.

## `zoom_blur`

Blurs a raster along rays from a centre, as a zoom.

Category: processing. Output: raster.

Inputs:

- `source`: raster, required

Params:

- **`amount`** (number, at least 0; default `0.2`) Length of the streak through each pixel, as a fraction of its distance from `center`. The streak runs both ways along the ray, sampled in equal ratios of scale, so it reaches farther away from `center` than towards it.
- **`center`** ([number, number]) Centre of the zoom in canvas pixels. The canvas centre, `[width / 2, height / 2]`, when absent.
- **`edge`** (`"clamp"` | `"transparent"`; default `"clamp"`) What the source reads as beyond the canvas.

## `boolean`

Combines two raster alpha masks with boolean operations.

Category: compositing. Output: raster.

Inputs:

- `a`: raster, required
- `b`: raster, required

Params:

- **`operation`** (`"union"` | `"intersect"` | `"subtract"` | `"xor"`; default `"union"`) Boolean operation used to combine the two input masks.
- **`threshold`** (number, 0 to 1; default `0`) Alpha above this cutoff counts as opaque; 0 applies no cutoff and keeps antialiased edges.

## `clip`

Plays a composition template, or its source, over a window of time.

Category: compositing. Output: raster.

Inputs:

- `source`: raster, optional

Params:

- **`composition`** (string not an expression; default `""`; literal only) Template from `compositions` to play; empty plays the `source` input instead.
- **`duration`** (number, at least -1; default `-1`) Frames the clip plays for; -1 plays the template to its end, or a source to the end of this composition.
- **`end`** (`"hide"` | `"hold"` | `"loop"`; default `"hide"`; literal only) What the clip shows after its window: nothing, its last frame, or the window again.
- **`offset`** (number; default `0`) Frames of the content skipped at the start, in the content's frames.
- **`props`** ([clipProps](#type-clipprops); default `{}`; literal as a whole) Values for the template's props by name; expressions run in this composition's time.
- **`speed`** (number; default `1`) Playback rate of the content; must be greater than 0.
- **`start`** (number; default `0`) Frame the clip starts at, in this composition's frames.

## `merge`

Composites an overlay raster over a base raster.

Category: compositing. Output: raster.

Inputs:

- `base`: raster, required
- `overlay`: raster, required
- `mask`: raster, optional

Params:

- **`blend_mode`** (`"normal"` | `"multiply"` | `"screen"` | `"overlay"` | `"darken"` | `"lighten"` | `"add"` | `"difference"` | `"exclusion"` | `"color_dodge"` | `"color_burn"` | `"hard_light"` | `"soft_light"` | `"hue"` | `"saturation"` | `"color"` | `"luminosity"`; default `"normal"`) Blend mode used when combining the overlay with the base raster. `normal` composites in linear light; every other mode applies its formula to the sRGB-encoded colours and composites the result in linear light.
- **`feather`** (number, at least 0; default `0`) Softens the mask edge with a Gaussian blur of this radius in pixels, applied before `invert`. Radii above about 588 look the same.
- **`invert`** (boolean; default `false`) Swaps the masked and unmasked areas.
- **`mask_mode`** (`"alpha"` | `"luma"`; default `"alpha"`; literal only) Channel of the `mask` input that controls overlay visibility.
- **`opacity`** (number, 0 to 1; default `1`) Overlay opacity applied before compositing.

## `raster_multimerge`

Composites a variadic stack of raster layers in order.

Category: compositing. Output: raster.

Inputs:

- `layers`: raster, optional, takes several connections

Params:

- **`blend_mode`** (`"normal"` | `"multiply"` | `"screen"` | `"overlay"` | `"darken"` | `"lighten"` | `"add"` | `"difference"` | `"exclusion"` | `"color_dodge"` | `"color_burn"` | `"hard_light"` | `"soft_light"` | `"hue"` | `"saturation"` | `"color"` | `"luminosity"`; default `"normal"`) Blend mode used for each layer in the stack. `normal` composites in linear light; every other mode applies its formula to the sRGB-encoded colours and composites the result in linear light.
- **`opacity`** (number, 0 to 1; default `1`) Opacity of each layer above the first, which is the base and is used as is.

## `switch`

Selects one raster input according to a controlled layer index.

Category: compositing. Output: raster.

Inputs:

- `layers`: raster, optional, takes several connections

Params:

- **`selected_layer`** (integer, at least 0; default `0`) Zero-based input index to route to the output.

## `camera_3d`

The viewpoint `render_3d` draws from, looking down its -Z. The first camera in a scene wins; without one, `render_3d` views from 5 units along +Z with a 40° field of view.

Category: scene. Output: scene.

Params:

- **`far`** (number, at least 0; default `1000`) Farthest distance drawn, in world units.
- **`fov`** (number, 1 to 179; default `40`) Vertical field of view in degrees, with a perspective projection.
- **`look_at`** (boolean; default `false`) Aims the camera at `target`, ignoring its rotation.
- **`near`** (number, at least 0; default `0.05`) Nearest distance drawn, in world units.
- **`ortho_height`** (number, at least 0; default `4`) Height of the view in world units, with an orthographic projection.
- **`position`** ([number, number, number]; default `[0,0,5]`) Position in world units along X, Y and Z, in the parent's space.
- **`projection`** (`"perspective"` | `"orthographic"`; default `"perspective"`; literal only) How the scene is projected onto the image.
- **`rotation`** ([number, number, number]; default `[0,0,0]`) Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z.
- **`target`** ([number, number, number]; default `[0,0,0]`) Point along X, Y and Z the camera aims at while `look_at` is on, in its parent's space.

## `image_plane_3d`

Shows a raster, such as text or video, on a flat rectangle facing +Z. Unlit and double-sided by default, so it looks as it does in 2D while it faces the camera. A plane is drawn blended, ordered by its centre among blended objects, so its transparent parts show what is behind it. Planes that intersect, or overlap while crossing in depth, can show in the wrong order; layers separated in depth, and opaque shapes in front of or behind planes, are ordered correctly. `opaque` draws a plane that has no transparent pixels with the opaque objects instead. A source keeps its detail when it is shown at its own size or larger; shown smaller, it is filtered with mipmaps, trilinearly and anisotropically, before `render_3d`'s `supersample` averages it.

Category: scene. Output: scene.

Inputs:

- `source`: raster, required

Params:

- **`cast_shadows`** (boolean; default `true`) Blocks light from lights that cast shadows. A blended surface blocks it where its alpha is at least half. Unlit and glowing surfaces block it too, so a shape standing in for a light, such as a bulb around a point light, needs this off.
- **`color`** ([color](#type-color); default `[255,255,255,255]`) Base colour. An image plane multiplies its raster by it. Must be a solid color; a gradient fails `check`.
- **`double_sided`** (boolean; default `true`) Draws the back of each face too.
- **`emissive`** (number, at least 0; default `0`) Light the surface gives off on its own, as a multiple of its base colour.
- **`metallic`** (number, 0 to 1; default `0`) 0 is a dielectric such as plastic, 1 a bare metal.
- **`opacity`** (number, 0 to 1; default `1`) Opacity multiplier. Anything below 1 is drawn blended, after opaque objects.
- **`opaque`** (boolean; default `false`) Draws the plane with the opaque objects, writing depth, rather than blended. For a source with no transparent pixels, this orders the plane correctly against any geometry it intersects or crosses in depth. Alpha is then ignored: the colour's alpha, `opacity` and the source's alpha. A source pixel that is transparent shows its colour at full opacity, which is black for a fully transparent one, and a half-transparent one shows its colour undimmed. It casts a shadow over its whole rectangle. A shadow catcher is always drawn as one, so this has no effect on it.
- **`position`** ([number, number, number]; default `[0,0,0]`) Position in world units along X, Y and Z, in the parent's space.
- **`receive_shadows`** (boolean; default `true`) Darkens where shadows fall on it. An unlit surface shows none.
- **`region`** ([region](#type-region); literal as a whole) Shows only this part of the source, `[x, y, width, height]` in the source's pixels from its top left, on a plane that covers just that part, so a plane showing a region is where it would be and as big as it would be with the whole source on it: it looks as a plane showing a `crop` of that region does, where `width` is still the width of the whole source, and `position`, `rotation` and `scale` pivot about the whole source's centre. Cut off at the source's edges as `crop` cuts its region, and a region with no pixel left shows nothing. Each of the four numbers takes an integer or an expression, which is truncated toward zero as `crop`'s are; the whole region cannot be an expression. Unset shows the whole source. Planes showing parts of one source share its texture and mip levels and draw together, so a region costs nothing beyond the plane, where a `crop` costs a raster the size of the source and a draw of its own. Sampling is kept inside the region, so what is outside it isn't meant to show. To keep it so under strong minification, the band along each edge of the region that is inside the source, and all of a narrow region, is sampled from a sharper mip level than its footprint, since a texel of a shared level can hold pixels from both sides of the edge: it can alias or shimmer there where a `crop` would be smooth, across the whole of a piece drawn much smaller than its pixels.
- **`rotation`** ([number, number, number]; default `[0,0,0]`) Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z.
- **`roughness`** (number, 0 to 1; default `0.5`) 0 is a mirror finish, 1 fully diffuse.
- **`scale`** ([number, number, number]; default `[1,1,1]`) Scale multiplier along X, Y and Z, applied before the rotation.
- **`shadow_catcher`** (boolean; default `false`) Shows nothing but the shadows falling on it, as transparent black that darkens what the render is composited over by as much light as the shadows take away, times the colour's alpha, the opacity and, on an image plane, its raster's alpha, whatever `unlit` says. It shows none of its colours, hides what is behind it wherever that alpha isn't 0, and casts no shadows, so a ground plane catching a model's shadow can stand in for the floor of 2D footage behind it.
- **`unlit`** (boolean; default `true`) Shows the base colour as is, ignoring lights.
- **`width`** (number, at least 0; default `1`) Width in world units, 1 being as wide as a default `shape_3d` box; the height follows the aspect ratio of the source's full display size.

## `instance_3d`

Draws its input scene `count` times, each copy placed by `position`, `rotation` and `scale` evaluated for that copy, where `instance` is the copy's index from 0 and `instance_count` is `count`: `position = vec3((instance % 10) * 1.5, 0, floor(instance / 10) * 1.5)` lays out a grid, and a ring, a spiral or a path follow from `sin`, `cos` and `path_point` of `instance / instance_count`. A copy is placed in the input scene's own space, so it turns and scales about the origin of the scene's own coordinates.

Copies share the input's meshes, textures, materials and joint matrices on the GPU, so every copy of a skinned model plays the same animation at the same time. Only the objects are copied: the input's lights and camera appear once, placed as they would be without the node, so copies don't multiply a scene's lighting. Shadows, fog and the environment treat a copy as any other object. An `instance_3d` may hold another, which multiplies their counts, up to 8 deep. A scene draws at most 100000 objects in all, counting each copy's objects, and a count that would exceed this fails the render. The node is static when its params don't read the frame and its scene is static.

Category: scene. Output: scene.

Inputs:

- `scene`: scene, required

Params:

- **`count`** (integer, 0 to 100000; default `1`) Copies to place, from 0 to 100000. 0 leaves the node's scene out entirely, lights and cameras too.
- **`position`** ([number, number, number]; default `[0,0,0]`) Position in world units along X, Y and Z of each copy, in the scene's own space. Evaluated for every copy, and can read `instance` and `instance_count`.
- **`rotation`** ([number, number, number]; default `[0,0,0]`) Rotation in degrees about X, Y and Z of each copy, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z. Evaluated for every copy, and can read `instance` and `instance_count`.
- **`scale`** ([number, number, number]; default `[1,1,1]`) Scale multiplier along X, Y and Z of each copy, applied before the rotation. Evaluated for every copy, and can read `instance` and `instance_count`.

## `light_3d`

A light. A scene without any is lit by a default key light and ambient fill.

Category: scene. Output: scene.

Params:

- **`cast_shadows`** (boolean; default `false`) Casts shadows from objects that block it onto those behind them. A scene draws shadows for its first 8 lights that cast them, in the order its scene inputs are wired, and warns about the rest. A spot light's shadows reach 170° across at most.
- **`color`** ([color](#type-color); default `[255,255,255,255]`) Light colour. Must be a solid color; a gradient fails `check`.
- **`intensity`** (number, at least 0; default `3`) Unitless brightness multiplier; point and spot lights fall off with the square of distance.
- **`light`** (`"directional"` | `"point"` | `"spot"` | `"ambient"`; default `"directional"`; literal only) Kind of light.
- **`position`** ([number, number, number]; default `[0,0,0]`) Position in world units along X, Y and Z, in the parent's space.
- **`range`** (number, at least 0; default `0`) Distance past which a point or spot light has no effect; 0 is unlimited.
- **`rotation`** ([number, number, number]; default `[-45,30,0]`) Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z.
- **`shadow_distance`** (number, at least 0.01; default `50`) How far from the camera, along its view, a directional light's shadows reach, fading out over the last tenth. Its shadow map spreads over what the camera sees of the scene up to here, so a larger distance trades sharpness for reach; a scene the camera sees nearer keeps its full sharpness.
- **`shadow_softness`** (number, at least 0; default `0.02`) Width in world units over which shadow edges blur, up to 16 texels of the light's shadow map, which is up to 4096 texels across and shared among the shadow-casting lights.
- **`spot_angle`** (number, 0 to 179; default `45`) Full angle of a spot light's cone in degrees.
- **`spot_softness`** (number, 0 to 1; default `0.2`) Fraction of a spot light's cone, from its edge, that fades out.

## `merge_3d`

Groups scenes into one, placed together by its transform.

Category: scene. Output: scene.

Inputs:

- `objects`: scene, optional, takes several connections

Params:

- **`position`** ([number, number, number]; default `[0,0,0]`) Position in world units along X, Y and Z, in the parent's space.
- **`rotation`** ([number, number, number]; default `[0,0,0]`) Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z.
- **`scale`** ([number, number, number]; default `[1,1,1]`) Scale multiplier along X, Y and Z, applied before the rotation.

## `model_3d`

A glTF model: the default scene of a `.glb`, with its metallic-roughness materials, vertex colours, two sets of texture coordinates with `KHR_texture_transform`, skins, morph targets and animation clips. A raster wired into `texture` shows on the surfaces of the material `texture_material` names, such as a screen. The model's own lights and a camera are added with `lights` and `camera`. Points, lines, and triangle strips and fans aren't drawn, with a `scene.model_primitives_skipped` warning.

Category: scene. Output: scene.

Inputs:

- `texture`: raster, optional

Params:

- **`animation`** (string; default `""`) The animation clip to play, by its name or else its index; empty plays none. A model's clips are listed in the `animations` of its upload's `metadata` on a host that takes uploads, and by `modelInfo` in the browser bindings.
- **`animation_loop`** (boolean; default `true`) Plays the clip again from its start once it ends, rather than holding its last pose.
- **`animation_offset`** (number; default `0`) Seconds into the clip at the clip's first frame.
- **`animation_speed`** (number; default `1`) Playback speed multiplier.
- **`camera`** (string; default `""`) The model's camera to view the scene from, by its name or else its index, placed by the first node that places it, without that node's scale; empty uses none. It counts as a `camera_3d` at this node, so the first camera in the scene still wins. Its aspect ratio is the output's. A model's cameras are listed in the `cameras` of its upload's `metadata` on a host that takes uploads, and by `modelInfo` in the browser bindings.
- **`cast_shadows`** (boolean; default `true`) Blocks light from lights that cast shadows, in the posed shape and where masked or blended materials' alpha reaches their cutoff, or half for blended ones. Unlit and glowing materials block it too, so a model of a lamp around a light needs this off.
- **`lights`** (boolean; default `false`) Adds the model's own `KHR_lights_punctual` lights to the scene, where its nodes place them, as `light_3d` nodes there would light it: a directional light's intensity, in lux, and a point or spot light's, in candela, are `light_3d` intensities as they are, so a point light of intensity `I` lights a surface `d` metres away as a directional one of `I / d²` does. A spot light's cone fades from its inner angle out to its outer one. They cast no shadows, and a scene they light gets no default lights.
- **`position`** ([number, number, number]; default `[0,0,0]`) Position in world units along X, Y and Z, in the parent's space.
- **`receive_shadows`** (boolean; default `true`) Darkens where shadows fall on its lit surfaces.
- **`rotation`** ([number, number, number]; default `[0,0,0]`) Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z.
- **`scale`** ([number, number, number]; default `[1,1,1]`) Scale multiplier along X, Y and Z, applied before the rotation.
- **`source`** (string; media: model; default `""`) A binary glTF (`.glb`) file with its buffers and images inside it: a path or `media` alias, or `asset:<id>` for a model asset of the host's.
- **`texture_material`** (string; default `""`) The material, by name, whose surfaces show the `texture` input instead, unlit. A model's materials are listed in the `materials` of its upload's `metadata` on a host that takes uploads, and by `modelInfo` in the browser bindings.

## `render_3d`

Draws a 3D scene into a raster, with a transparent background unless it shows its environment map; or, by `output`, its depth or normals, for 2D effects such as fog.

Category: scene. Output: raster.

Inputs:

- `scene`: scene, required

Params:

- **`depth_far`** (number, at least 0; default `0`) Distance from the camera, in world units, that `depth` output maps to 1, and past which it stays 1. 0 uses the camera's `far`, which spreads the 0 to 1 range thin over a scene much nearer than it.
- **`depth_near`** (number, at least 0; default `0`) Distance from the camera, in world units, that `depth` output maps to 0. 0 uses the camera's `near`.
- **`environment`** (number, at least 0; default `1`) Brightness of the light from all around that surfaces reflect and are lit by: the `environment_map` when one is set, and otherwise a built-in studio of softboxes. 0 turns it off.
- **`environment_background`** (boolean; default `false`) Shows the environment map behind the scene, through the exposure and tone mapping but not the fog, which makes the output opaque. Shadow catchers darken it.
- **`environment_background_blur`** (number, 0 to 1; default `0`) How blurred the background is, as a surface of this roughness reflects it: 0 is sharp.
- **`environment_map`** (string; media: environment; default `""`) An equirectangular Radiance `.hdr` or OpenEXR `.exr` file that lights the scene from all around in place of the studio: a path or `media` alias, or `asset:<id>` for an environment map asset of the host's. Its centre is the view along -Z, with +X to its right and +Y at its top, and it is stretched over the whole sphere whatever its aspect. Non-finite and negative pixels are read as black. While a map lights a scene without lights, it gets no default ones. Empty uses the studio.
- **`environment_rotation`** (number; default `0`) Turn of the environment map about +Y in degrees, as a yaw turns an object.
- **`exposure`** (number; default `0`) Exposure adjustment in stops before tone mapping.
- **`fog_color`** ([color](#type-color); default `[255,255,255,255]`) Colour surfaces fade to with distance. Must be a solid color; a gradient fails `check`.
- **`fog_density`** (number, at least 0; default `0`) How quickly surfaces fade into the fog: the fraction of their colour left after `d` world units is e^(-density * d). 0 turns fog off.
- **`height`** (integer, at least 0; default `0`; literal only) Output height in pixels. Use 0 to match the composition height.
- **`output`** (`"color"` | `"depth"` | `"normal"`; default `"color"`; literal only) What the raster holds: the lit scene, its depth or its normals. A composition that needs more than one draws the same scene with a `render_3d` for each.
- **`supersample`** (integer, 1 to 4; default `2`) Renders this many times wider and taller, then averages down, to antialias edges.
- **`tone_mapping`** (`"none"` | `"aces"` | `"agx"`; default `"none"`; literal only) How light brighter than white is brought into range.
- **`width`** (integer, at least 0; default `0`; literal only) Output width in pixels. Use 0 to match the composition width.

## `shape_3d`

A primitive solid: box, sphere, plane, cylinder, torus or cone, centred on its position.

Category: scene. Output: scene.

Params:

- **`cast_shadows`** (boolean; default `true`) Blocks light from lights that cast shadows. A blended surface blocks it where its alpha is at least half. Unlit and glowing surfaces block it too, so a shape standing in for a light, such as a bulb around a point light, needs this off.
- **`color`** ([color](#type-color); default `[255,255,255,255]`) Base colour. An image plane multiplies its raster by it. Must be a solid color; a gradient fails `check`.
- **`double_sided`** (boolean; default `false`) Draws the back of each face too.
- **`emissive`** (number, at least 0; default `0`) Light the surface gives off on its own, as a multiple of its base colour.
- **`metallic`** (number, 0 to 1; default `0`) 0 is a dielectric such as plastic, 1 a bare metal.
- **`opacity`** (number, 0 to 1; default `1`) Opacity multiplier. Anything below 1 is drawn blended, after opaque objects.
- **`position`** ([number, number, number]; default `[0,0,0]`) Position in world units along X, Y and Z, in the parent's space.
- **`receive_shadows`** (boolean; default `true`) Darkens where shadows fall on it. An unlit surface shows none.
- **`rotation`** ([number, number, number]; default `[0,0,0]`) Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z.
- **`roughness`** (number, 0 to 1; default `0.5`) 0 is a mirror finish, 1 fully diffuse.
- **`scale`** ([number, number, number]; default `[1,1,1]`) Scale multiplier along X, Y and Z, applied before the rotation.
- **`shadow_catcher`** (boolean; default `false`) Shows nothing but the shadows falling on it, as transparent black that darkens what the render is composited over by as much light as the shadows take away, times the colour's alpha, the opacity and, on an image plane, its raster's alpha, whatever `unlit` says. It shows none of its colours, hides what is behind it wherever that alpha isn't 0, and casts no shadows, so a ground plane catching a model's shadow can stand in for the floor of 2D footage behind it.
- **`shape`** (`"box"` | `"sphere"` | `"plane"` | `"cylinder"` | `"torus"` | `"cone"`; default `"box"`; literal only) Primitive to build.
- **`size`** ([number, number, number]; default `[1,1,1]`) Width, height and depth in world units, along X, Y and Z before the rotation.
- **`unlit`** (boolean; default `false`) Shows the base colour as is, ignoring lights.

## `text_3d`

Text from its font's outlines, extruded into a solid with the material params of a `shape_3d`. The text is shaped as a `text` node with the same font, size and layout shapes it, with its kerning, ligatures, font fallback and wrapping, and lies in the XY plane facing +Z, reading along +X, with `alignment_horizontal` and `alignment_vertical` choosing the point of its box at `position`. Its geometry is built again only when the text, font, layout or bevel change, or its size or depth do with a bevel, so animating its transform or material costs no more than for a `shape_3d`. One style covers the whole text: rich text spans, decorations, strokes and the `glyph_*` params of a `text` node have no counterpart.

Category: scene. Output: scene.

Params:

- **`alignment_horizontal`** (`"left"` | `"center"` | `"right"` | `"justify"`; default `"center"`; literal only) How lines align with each other, and which point of the text's box sits at `position`: its left edge, centre or right edge. Justified lines anchor at the left edge.
- **`alignment_vertical`** (`"top"` | `"middle"` | `"bottom"`; default `"middle"`; literal only) Which point of the text's box sits at `position` vertically: the top of its first line, its middle, or the bottom of its last line.
- **`bevel_segments`** (integer, 0 to 16; default `4`) Steps of the bevel's quarter-round profile, up to 16: 1 is a flat 45° chamfer, and 0 is no bevel.
- **`bevel_size`** (number, at least 0; default `0`) How far the bevel rounds off each glyph's edges in world units, in from the outline and in from the front and back, up to half of `depth`; 0 is none. The outline keeps its size where the sides meet the bevel. A glyph whose strokes are too thin for the bevel gets the widest one that fits it, so at regular weights bevels past about 0.04 of the font size stop growing on most glyphs.
- **`cast_shadows`** (boolean; default `true`) Blocks light from lights that cast shadows. A blended surface blocks it where its alpha is at least half. Unlit and glowing surfaces block it too, so a shape standing in for a light, such as a bulb around a point light, needs this off.
- **`color`** ([color](#type-color); default `[255,255,255,255]`) Base colour. An image plane multiplies its raster by it. Must be a solid color; a gradient fails `check`.
- **`content`** (string; default `"Text"`) Text to extrude, in one style. Line breaks start new lines. Whitespace is not drawn, and glyphs that their font draws from colour or bitmap tables, such as emoji, have no outline and are left out with a `font.no_outlines` warning. A node extrudes at most as many glyphs as the host's text glyph limit lets a `text` node draw, 16384 unless the host sets another, and at most 4000000 vertices and 12000000 indices; more is a render error. One glyph builds from an outline of at most 4096 commands, 64 contours and 8192 flattened points, and takes at most 262144 vertices and 786432 indices, and 3072 points where it is bevelled; a font with a glyph past any of these fails the render rather than the glyph being left out.
- **`depth`** (number, at least 0; default `0.25`) Thickness along Z in world units, centred on `position`, so the front faces +Z at half of it. 0 is flat.
- **`double_sided`** (boolean; default `false`) Draws the back of each face too.
- **`emissive`** (number, at least 0; default `0`) Light the surface gives off on its own, as a multiple of its base colour.
- **`font_family`** (string; default `"Roboto"`) Font family name, or `asset:<id>` for a font asset of the host's. A family no font answers to falls back as a `text` node's does.
- **`font_features`** ([fontFeatures](#type-fontfeatures); default `{}`; literal as a whole) OpenType features by four-character tag, such as `{ "tnum": 1, "liga": 0 }`. 0 turns a feature off, 1 turns it on and larger values select an alternate. Values take literals or expressions, but the whole map cannot be an expression.
- **`font_size`** (number, at least 0; default `1`) Font size in world units: the height of the font's em square, so capitals are about 0.7 of it tall. At 1, the default, a word is about as tall as a default `shape_3d` box is wide. 0 draws nothing, and so does anything under 0.000001. At most 1000000; `depth`, `bevel_size`, `max_width`, `letter_spacing` and `line_height` may be at most 100000 times it, and at most 1000000 themselves.
- **`font_style`** (`"normal"` | `"italic"` | `"oblique"`; default `"normal"`) Font style.
- **`font_weight`** (integer, 1 to 1000; default `400`) Font weight. Variable fonts render any value in their weight range.
- **`letter_spacing`** (number; default `0`) Extra space after every glyph in world units. Negative values tighten the text.
- **`line_height`** (number, at least 0; default `0`) Distance between lines in world units. Use 0 for 1.2 × the font size.
- **`max_width`** (number, at least 0; default `0`) Maximum line width in world units, at which lines wrap. Use 0 for automatic width.
- **`metallic`** (number, 0 to 1; default `0`) 0 is a dielectric such as plastic, 1 a bare metal.
- **`opacity`** (number, 0 to 1; default `1`) Opacity multiplier. Anything below 1 is drawn blended, after opaque objects.
- **`position`** ([number, number, number]; default `[0,0,0]`) Position in world units along X, Y and Z, in the parent's space.
- **`receive_shadows`** (boolean; default `true`) Darkens where shadows fall on it. An unlit surface shows none.
- **`rotation`** ([number, number, number]; default `[0,0,0]`) Rotation in degrees about X, Y and Z, applied as yaw about Y, then pitch about the turned X, then roll about the turned Z.
- **`roughness`** (number, 0 to 1; default `0.5`) 0 is a mirror finish, 1 fully diffuse.
- **`scale`** ([number, number, number]; default `[1,1,1]`) Scale multiplier along X, Y and Z, applied before the rotation.
- **`shadow_catcher`** (boolean; default `false`) Shows nothing but the shadows falling on it, as transparent black that darkens what the render is composited over by as much light as the shadows take away, times the colour's alpha, the opacity and, on an image plane, its raster's alpha, whatever `unlit` says. It shows none of its colours, hides what is behind it wherever that alpha isn't 0, and casts no shadows, so a ground plane catching a model's shadow can stand in for the floor of 2D footage behind it.
- **`text_transform`** (`"none"` | `"uppercase"` | `"lowercase"` | `"capitalize"`; default `"none"`) Case applied to the text.
- **`unlit`** (boolean; default `false`) Shows the base colour as is, ignoring lights.

## `media_output`

Converts its source into the final composition output: sRGB-encoded and quantised once to the output's format.

Category: output. Output: raster.

Inputs:

- `source`: raster, required

## Types

### Type `clipProps`

Prop values a `clip` passes to its template, by prop name.

Value: map from names to [clipPropValue](#type-clippropvalue).

### Type `clipPropValue`

A value for a template prop: a literal of the prop's type, or an expression in the clip's time.

Value: number | string | [number, number] | [number, number, number] | [color](#type-color).

### Type `color`

Value: array of integer, 0 to 255, at least 3, at most 4 items | string hex colour, `#rrggbb` or `#rrggbbaa`.

### Type `dashPattern`

Lengths that a stroke alternately draws and skips, starting with a dash. Each takes a literal or an expression.

Value: array of (number, 0 to 1e+37 | expression), at most 16 items.

### Type `fontFeatures`

OpenType features by four-character tag. 0 turns a feature off, 1 turns it on and larger values select an alternate.

Value: map from names matching `^[\x20-\x7E]+$`, 4 characters to integer, 0 to 4294967295 | expression.

### Type `gradientPaint`

Fields:

- **`angle`** (number; default `0`; literal only) Start angle of a conic gradient in degrees.
- **`center`** ([number, number]; default `[0.5,0.5]`; literal as a whole) Center of a radial or conic gradient.
- **`end`** ([number, number]; default `[1,0]`; literal as a whole) End point of a linear gradient.
- **`focal`** ([number, number]; literal as a whole) Focal point of a radial gradient, where the first stop's color sits. The gradient widens from it to the circle at `center` and `radius`, which gives an off-centre highlight. Defaults to `center`. A point on or outside that circle is moved to 99% of the way to it from `center`, along the same direction.
- **`interpolation`** (`"srgb"` | `"linear_srgb"` | `"linear"`; default `"srgb"`; literal only) Color space the stops blend in: `srgb` blends the sRGB-encoded stops, `linear_srgb` blends them in linear light.
- **`radius`** (number | [number, number]; default `[0.5,0.5]`; literal as a whole) Horizontal and vertical radius of a radial gradient. A number sets both.
- **`spread`** (`"pad"` | `"repeat"` | `"reflect"`; default `"pad"`; literal only) How the gradient continues past its first and last stops.
- **`start`** ([number, number]; default `[0,0]`; literal as a whole) Start point of a linear gradient.
- **`stops`** (required; array of [gradientStop](#type-gradientstop), at least 1, at most 8 items; literal as a whole) Colors along the gradient. Required, with one to eight stops.
- **`type`** (required; `"linear_gradient"` | `"radial_gradient"` | `"conic_gradient"` | `"linear"` | `"radial"` | `"conic"`; literal only) Gradient shape. Required.
- **`units`** (`"object_bounding_box"` | `"user_space"` | `"userSpaceOnUse"`; default `"object_bounding_box"`; literal only) Whether points and radii are fractions of the shape's bounding box or pixels.

No other fields.

### Type `gradientStop`

A color at a point along a gradient: `{ offset, color }` or `[offset, color]`.

Value: { color: [color](#type-color); offset: number } | [number, [color](#type-color)].

### Type `paint`

A solid color or a gradient.

Value: [color](#type-color) | [gradientPaint](#type-gradientpaint).

### Type `region`

A rectangle of pixels, `[x, y, width, height]`. Each number takes an integer or an expression.

Value: [integer | expression, integer | expression, integer | expression, integer | expression].

### Type `repeat`

Fields:

- **`columns`** (integer, at least 0; default `0`) Copies per row, after which the next row starts. 0 puts every copy in one row.
- **`count`** (required; integer, 0 to 1024) Copies to draw, from 0 to 1024. 0 draws nothing, and 1 draws the node alone.
- **`opacity_offset`** (number, -1 to 1; default `0`) Opacity added for each copy. Each copy's opacity is clamped to 0–1.
- **`position_offset`** ([number, number]; default `[0,0]`) Position in pixels added for each column.
- **`rotation_offset`** (number; default `0`) Clockwise rotation in degrees added for each copy.
- **`row_offset`** ([number, number]; default `[0,0]`) Position in pixels added for each row.
- **`scale_offset`** ([number, number]; default `[0,0]`) Scale added for each copy, per axis. A negative offset shrinks later copies through 0, where they draw nothing, into mirrored copies.

No other fields.

### Type `shaderUniformComponent`

Value: number | expression.

### Type `shaderUniforms`

Shader uniform values by field name, checked against the shader's uniform struct when the node compiles.

Value: map from names to [shaderUniformValue](#type-shaderuniformvalue).

### Type `shaderUniformValue`

A shader uniform field value: a number, a 2-4 component vector, an expression, or a hex colour for vec3<f32> and vec4<f32> fields.

Value: number | array of [shaderUniformComponent](#type-shaderuniformcomponent), at least 2, at most 4 items | string hex colour, `#rrggbb` or `#rrggbbaa` | expression.

### Type `textContent`

Text to render: a string, or spans that each override the node's text style.

Value: string | array of [textSpan](#type-textspan).

### Type `textHighlight`

Fields:

- **`extent`** ([number, number]; default `[0,1]`) Start and end of the box along the text, as fractions of its width. An expression gives both as one `vec2`, not one per element.
- **`padding`** ([number, number]; default `[0,0]`) Horizontal and vertical padding around the text in pixels. An expression gives both as one `vec2`, not one per element.
- **`paint`** ([paint](#type-paint); default `[0,0,0,255]`) Box color.
- **`radius`** (number, at least 0; default `0`) Corner radius in pixels.

No other fields.

### Type `textLine`

Fields:

- **`extent`** ([number, number]; default `[0,1]`) Start and end of the line along the text, as fractions of its width. An expression gives both as one `vec2`, not one per element.
- **`offset`** (number) Line position in pixels below the baseline; negative values are above it. Absent uses the font's underline or strikethrough position.
- **`paint`** ([paint](#type-paint)) Line color. Absent uses the text color.
- **`style`** (`"solid"` | `"double"` | `"dotted"` | `"dashed"` | `"wavy"`; default `"solid"`) Line style.
- **`thickness`** (number, at least 0) Line thickness in pixels. Absent uses the font's.

No other fields.

### Type `textSpan`

Fields:

- **`baseline_shift`** (number) Pixels the span's glyphs and its own decorations sit above the baseline; negative values sit below it. The line keeps its height, so shifted text can overlap the lines around it.
- **`color`** ([paint](#type-paint)) Text color.
- **`font_family`** (string) Font family name.
- **`font_features`** ([fontFeatures](#type-fontfeatures); literal as a whole) OpenType features, merged over the node's: a feature set here wins and the others are kept. Within a word, the first span's features apply to all of it.
- **`font_size`** (number, at least 1) Font size in pixels. Where spans of one family, weight and style meet, a ligature or other cluster across the boundary takes the size of the span it starts in.
- **`font_style`** (`"normal"` | `"italic"` | `"oblique"`) Font style.
- **`font_weight`** (integer, 1 to 1000) Font weight. Variable fonts render any value in their weight range.
- **`highlight`** ([textHighlight](#type-texthighlight) | `null`; literal as a whole) Box behind the text. `null` turns off the node's highlight. Its fields take literals or expressions, but the whole highlight cannot be an expression.
- **`letter_spacing`** (number) Extra space after every glyph in pixels. Negative values tighten the text.
- **`strikethrough`** ([textLine](#type-textline) | `null`; literal as a whole) Line through the text. `null` turns off the node's strikethrough. Its fields take literals or expressions, but the whole line cannot be an expression.
- **`stroke`** ([textStroke](#type-textstroke) | `null`; literal as a whole) Outline along the glyphs. `null` turns off the node's stroke. Its fields take literals or expressions, but the whole stroke cannot be an expression.
- **`text`** (required; string) Text of the span. Required.
- **`text_transform`** (`"none"` | `"uppercase"` | `"lowercase"` | `"capitalize"`) Case applied to the span's text.
- **`underline`** ([textLine](#type-textline) | `null`; literal as a whole) Line under the text. `null` turns off the node's underline. Its fields take literals or expressions, but the whole line cannot be an expression.

No other fields.

### Type `textStroke`

Fields:

- **`align`** (`"outside"` | `"center"`; default `"outside"`) Where the outline lies: behind the glyphs and outside their edge, or over them and centred on it.
- **`paint`** ([paint](#type-paint); default `[0,0,0,255]`) Outline color.
- **`width`** (number, at least 0; default `2`) Outline width in pixels.

No other fields.
