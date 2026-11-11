//! Radiance `.hdr` files of RGBE pixels, flat or run-length encoded, with rows from the top and
//! columns from the left (`-Y height +X width`).

use super::{EnvironmentError, Equirect, Shrink, invalid, unsupported};

pub(super) fn is_hdr(bytes: &[u8]) -> bool {
    bytes.starts_with(b"#?RADIANCE") || bytes.starts_with(b"#?RGBE")
}

fn truncated() -> EnvironmentError {
    invalid("the environment map ends early")
}

/// The next `count` bytes of `data`, which it moves past.
fn take<'a>(data: &mut &'a [u8], count: usize) -> Result<&'a [u8], EnvironmentError> {
    if data.len() < count {
        return Err(truncated());
    }
    let (taken, rest) = data.split_at(count);
    *data = rest;
    Ok(taken)
}

fn take_byte(data: &mut &[u8]) -> Result<u8, EnvironmentError> {
    Ok(take(data, 1)?[0])
}

fn take_pixel(data: &mut &[u8]) -> Result<[u8; 4], EnvironmentError> {
    let bytes = take(data, 4)?;
    Ok([bytes[0], bytes[1], bytes[2], bytes[3]])
}

/// The longest header line read, newline included.
const MAX_LINE_BYTES: usize = 1024;
/// The most header lines read before the blank line that ends them.
const MAX_HEADER_LINES: usize = 128;

/// The next line of `data` without its newline, which it moves past, looking no further than
/// [`MAX_LINE_BYTES`] for its end.
fn line<'a>(data: &mut &'a [u8]) -> Result<&'a [u8], EnvironmentError> {
    let end = data[..data.len().min(MAX_LINE_BYTES)]
        .iter()
        .position(|&byte| byte == b'\n')
        .ok_or_else(|| {
            if data.len() < MAX_LINE_BYTES {
                truncated()
            } else {
                invalid("a header line of the environment map is too long")
            }
        })?;
    let line = &data[..end];
    *data = &data[end + 1..];
    Ok(line.strip_suffix(b"\r").unwrap_or(line))
}

pub(super) fn decode(bytes: &[u8]) -> Result<Equirect, EnvironmentError> {
    let mut data = bytes;
    line(&mut data)?;
    let mut lines = 0;
    loop {
        let header = line(&mut data)?;
        if header.is_empty() {
            break;
        }
        lines += 1;
        if lines > MAX_HEADER_LINES {
            return Err(invalid("the environment map's header has too many lines"));
        }
        if let Some(format) = header.strip_prefix(b"FORMAT=")
            && format.trim_ascii() != b"32-bit_rle_rgbe"
        {
            return Err(unsupported(
                "the environment map's pixels aren't RGBE, the only Radiance format read",
            ));
        }
    }
    let resolution = std::str::from_utf8(line(&mut data)?)
        .map_err(|_| invalid("the environment map's size isn't text"))?;
    let mut fields = resolution.split_ascii_whitespace();
    let first = std::array::from_fn::<_, 4, _>(|_| fields.next());
    let (width, height) = match (first, fields.next()) {
        ([Some("-Y"), Some(height), Some("+X"), Some(width)], None) => {
            (width.parse::<u32>(), height.parse::<u32>())
        }
        _ => {
            return Err(unsupported(
                "the environment map's rows aren't stored from the top and left (-Y and +X)",
            ));
        }
    };
    let (Ok(width), Ok(height)) = (width, height) else {
        return Err(invalid("the environment map's size isn't a number"));
    };
    let mut shrink = Shrink::new(width, height)?;
    let mut row = vec![[0; 4]; width as usize];
    for y in 0..height {
        read_row(&mut data, &mut row)?;
        for (x, &pixel) in (0..).zip(&row) {
            shrink.add(x, y, rgbe(pixel));
        }
    }
    Ok(shrink.finish())
}

/// The radiance an RGBE pixel holds: its mantissas scaled by two to its exponent.
fn rgbe([r, g, b, e]: [u8; 4]) -> [f32; 3] {
    if e == 0 {
        return [0.0; 3];
    }
    let scale = (f32::from(e) - 136.0).exp2();
    [r, g, b].map(|c| f32::from(c) * scale)
}

/// Reads one row of pixels into `row`, in whichever encoding it has.
fn read_row(data: &mut &[u8], row: &mut [[u8; 4]]) -> Result<(), EnvironmentError> {
    let width = row.len();
    let first = take_pixel(data)?;
    if (8..=0x7fff).contains(&width) && first[0] == 2 && first[1] == 2 && first[2] < 128 {
        if (usize::from(first[2]) << 8 | usize::from(first[3])) != width {
            return Err(invalid(
                "a row of the environment map is a different width from the map",
            ));
        }
        // Each channel in turn, as runs of one value or spans of literal values.
        for channel in 0..4 {
            let mut x = 0;
            while x < width {
                let count = take_byte(data)?;
                let (run, length) = if count > 128 {
                    (true, usize::from(count - 128))
                } else {
                    (false, usize::from(count))
                };
                if length == 0 || x + length > width {
                    return Err(invalid("a row of the environment map overruns its width"));
                }
                if run {
                    let value = take_byte(data)?;
                    row[x..x + length]
                        .iter_mut()
                        .for_each(|pixel| pixel[channel] = value);
                } else {
                    let values = take(data, length)?;
                    for (pixel, &value) in row[x..x + length].iter_mut().zip(values) {
                        pixel[channel] = value;
                    }
                }
                x += length;
            }
        }
        return Ok(());
    }
    // Flat pixels, where (1, 1, 1, n) repeats the previous one n times, and consecutive repeats
    // count in higher bytes.
    let mut x = 0;
    let mut shift = 0;
    let mut pixel = first;
    loop {
        if pixel[..3] == [1, 1, 1] {
            if x == 0 || shift > 16 {
                return Err(invalid("a row of the environment map repeats no pixel"));
            }
            let count = usize::from(pixel[3]) << shift;
            if x + count > width {
                return Err(invalid("a row of the environment map overruns its width"));
            }
            let previous = row[x - 1];
            row[x..x + count].fill(previous);
            x += count;
            shift += 8;
        } else {
            row[x] = pixel;
            x += 1;
            shift = 0;
        }
        if x == width {
            return Ok(());
        }
        pixel = take_pixel(data)?;
    }
}
