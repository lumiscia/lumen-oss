//! Number text for the `format` builtin.

/// The most digits `format` writes after the point.
pub(crate) const MAX_DECIMALS: f64 = 20.0;
/// The widest `format` pads to.
pub(crate) const MAX_WIDTH: f64 = 64.0;

/// `value` with exactly `decimals` digits after the point. Halves round away from zero on the
/// shortest decimal that reads back as `value`, so 1.005 gives 1.01 although its nearest double
/// is just under. A result that rounds to zero has no sign. `value` must be finite.
pub(crate) fn fixed(value: f64, decimals: usize) -> String {
    let shortest = value.abs().to_string();
    let (whole, fraction) = shortest.split_once('.').unwrap_or((&shortest, ""));
    let round_up = fraction
        .as_bytes()
        .get(decimals)
        .is_some_and(|digit| *digit >= b'5');
    let mut digits: Vec<u8> = whole
        .bytes()
        .chain(fraction.bytes().take(decimals))
        .collect();
    digits.resize(whole.len() + decimals, b'0');
    if round_up {
        let carried = digits.iter_mut().rev().all(|digit| {
            let carry = *digit == b'9';
            *digit = if carry { b'0' } else { *digit + 1 };
            carry
        });
        if carried {
            digits.insert(0, b'1');
        }
    }
    let point = digits.len() - decimals;
    let mut text = String::with_capacity(digits.len() + 2);
    if value.is_sign_negative() && digits.iter().any(|digit| *digit != b'0') {
        text.push('-');
    }
    text.extend(digits[..point].iter().map(|digit| char::from(*digit)));
    if decimals > 0 {
        text.push('.');
        text.extend(digits[point..].iter().map(|digit| char::from(*digit)));
    }
    text
}

/// `text` padded on the left with `fill` to `width` characters. Zeros go after a leading minus.
pub(crate) fn pad(text: String, width: usize, fill: char) -> String {
    let missing = width.saturating_sub(text.chars().count());
    if missing == 0 {
        return text;
    }
    let padding: String = std::iter::repeat_n(fill, missing).collect();
    match text.strip_prefix('-') {
        Some(magnitude) if fill == '0' => format!("-{padding}{magnitude}"),
        _ => format!("{padding}{text}"),
    }
}
