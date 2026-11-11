//! Adobe/Resolve `.cube` 3D colour lookup tables, parsed from untrusted files.

use crate::media::MAX_LUT_BYTES;

/// The largest `LUT_3D_SIZE` read: 129 points per axis is 2.1 million entries, more than any
/// grading export needs.
pub(crate) const MAX_LUT_3D_SIZE: u32 = 129;

/// The largest magnitude a table value may have, which a half float holds.
const MAX_VALUE: f32 = 65504.0;

/// A 3D table. `texels` holds `size`³ colours with red varying fastest, then green, then blue.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct CubeLut {
    pub(crate) size: u32,
    pub(crate) domain_min: [f32; 3],
    pub(crate) domain_max: [f32; 3],
    pub(crate) texels: Vec<[f32; 3]>,
}

/// Why a file is not a table that can be used.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum CubeError {
    Invalid(String),
    Unsupported(String),
    TooLarge {
        details: String,
        limit: u64,
        actual: u64,
    },
}

fn invalid(line: usize, details: impl std::fmt::Display) -> CubeError {
    CubeError::Invalid(format!("line {line}: {details}"))
}

fn number(line: usize, token: &str) -> Result<f32, CubeError> {
    let value = token
        .parse::<f32>()
        .ok()
        .filter(|value| value.is_finite())
        .ok_or_else(|| invalid(line, format!("`{token}` is not a finite number")))?;
    if value.abs() > MAX_VALUE {
        return Err(invalid(
            line,
            format!("`{token}` is outside ±{MAX_VALUE}, which a table can hold"),
        ));
    }
    Ok(value)
}

fn triple(line: usize, keyword: &str, tokens: &[&str]) -> Result<[f32; 3], CubeError> {
    if tokens.len() != 3 {
        return Err(invalid(line, format!("{keyword} takes three numbers")));
    }
    Ok([
        number(line, tokens[0])?,
        number(line, tokens[1])?,
        number(line, tokens[2])?,
    ])
}

impl CubeLut {
    pub(crate) fn parse(bytes: &[u8]) -> Result<Self, CubeError> {
        if bytes.len() > MAX_LUT_BYTES {
            return Err(CubeError::TooLarge {
                details: format!(
                    "the file is larger than the {} MiB a .cube file may be",
                    MAX_LUT_BYTES >> 20
                ),
                limit: MAX_LUT_BYTES as u64,
                actual: bytes.len() as u64,
            });
        }
        let text = std::str::from_utf8(bytes)
            .map_err(|_| CubeError::Invalid("the file is not UTF-8 text".to_string()))?;

        let mut size: Option<u32> = None;
        let mut domain_min = [0.0; 3];
        let mut domain_max = [1.0; 3];
        let mut texels: Vec<[f32; 3]> = Vec::new();
        let mut expected = 0_usize;
        for (index, raw) in text.lines().enumerate() {
            let line = index + 1;
            let content = raw.split('#').next().unwrap_or_default();
            let tokens: Vec<&str> = content.split_whitespace().take(8).collect();
            let Some(&first) = tokens.first() else {
                continue;
            };
            if first.starts_with(|c: char| c.is_ascii_digit() || matches!(c, '-' | '+' | '.')) {
                if size.is_none() {
                    return Err(invalid(line, "table data comes before LUT_3D_SIZE"));
                }
                if texels.len() == expected {
                    return Err(invalid(
                        line,
                        format!("more than the {expected} entries LUT_3D_SIZE declares"),
                    ));
                }
                texels.push(triple(line, "a table entry", &tokens)?);
                continue;
            }
            let args = &tokens[1..];
            match first {
                "TITLE" => {}
                "LUT_3D_SIZE" => {
                    if size.is_some() {
                        return Err(invalid(line, "LUT_3D_SIZE is declared twice"));
                    }
                    if !texels.is_empty() {
                        return Err(invalid(line, "LUT_3D_SIZE comes after table data"));
                    }
                    let [value] = args else {
                        return Err(invalid(line, "LUT_3D_SIZE takes one integer"));
                    };
                    let value = value.parse::<u32>().map_err(|_| {
                        invalid(line, format!("LUT_3D_SIZE `{value}` is not a whole number"))
                    })?;
                    if value < 2 {
                        return Err(invalid(line, "LUT_3D_SIZE must be at least 2"));
                    }
                    if value > MAX_LUT_3D_SIZE {
                        return Err(CubeError::TooLarge {
                            details: format!(
                                "LUT_3D_SIZE {value} is larger than the {MAX_LUT_3D_SIZE} a table may have"
                            ),
                            limit: u64::from(MAX_LUT_3D_SIZE),
                            actual: u64::from(value),
                        });
                    }
                    size = Some(value);
                    expected = (value as usize).pow(3);
                    texels.reserve_exact(expected);
                }
                "LUT_1D_SIZE" => {
                    return Err(CubeError::Unsupported(
                        "1D .cube tables are not supported; export a 3D table (LUT_3D_SIZE)"
                            .to_string(),
                    ));
                }
                "DOMAIN_MIN" => domain_min = triple(line, "DOMAIN_MIN", args)?,
                "DOMAIN_MAX" => domain_max = triple(line, "DOMAIN_MAX", args)?,
                other => {
                    return Err(invalid(
                        line,
                        format!("unknown keyword `{other}`, which can change what the table means"),
                    ));
                }
            }
        }

        let Some(size) = size else {
            return Err(CubeError::Invalid(
                "the file has no LUT_3D_SIZE line".to_string(),
            ));
        };
        if texels.len() != expected {
            return Err(CubeError::Invalid(format!(
                "LUT_3D_SIZE {size} needs {expected} entries, the file has {}",
                texels.len()
            )));
        }
        if (0..3).any(|axis| domain_min[axis] >= domain_max[axis]) {
            return Err(CubeError::Invalid(
                "DOMAIN_MIN must be below DOMAIN_MAX on every channel".to_string(),
            ));
        }
        Ok(Self {
            size,
            domain_min,
            domain_max,
            texels,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cube(size: u32) -> String {
        let mut text = format!("LUT_3D_SIZE {size}\n");
        let last = (size - 1) as f32;
        for b in 0..size {
            for g in 0..size {
                for r in 0..size {
                    text += &format!(
                        "{} {} {}\n",
                        r as f32 / last,
                        g as f32 / last,
                        b as f32 / last
                    );
                }
            }
        }
        text
    }

    #[test]
    fn cube_files_parse_and_reject_what_cannot_be_used() {
        let parsed = CubeLut::parse(
            format!(
                "# made by hand\nTITLE \"two\"\n\nDOMAIN_MIN 0 0 -1\nDOMAIN_MAX 1 2 1 # wide\n{}",
                cube(2)
            )
            .as_bytes(),
        )
        .unwrap();
        assert_eq!(parsed.size, 2);
        assert_eq!(parsed.domain_min, [0.0, 0.0, -1.0]);
        assert_eq!(parsed.domain_max, [1.0, 2.0, 1.0]);
        assert_eq!(parsed.texels.len(), 8);
        assert_eq!(parsed.texels[1], [1.0, 0.0, 0.0], "red varies fastest");
        assert_eq!(parsed.texels[2], [0.0, 1.0, 0.0]);
        assert_eq!(parsed.texels[4], [0.0, 0.0, 1.0]);

        let three_points = CubeLut::parse(cube(3).as_bytes()).unwrap();
        assert_eq!((three_points.size, three_points.texels.len()), (3, 27));
        assert_eq!(three_points.texels[1], [0.5, 0.0, 0.0]);

        let eight = "0 0 0\n".repeat(8);
        let short = "0 0 0\n".repeat(7);
        let long = "0 0 0\n".repeat(9);
        let huge = format!("LUT_3D_SIZE {}\n", MAX_LUT_3D_SIZE + 1);
        let table: [(String, &str); 14] = [
            (
                format!("LUT_3D_SIZE 2\n{short}"),
                "needs 8 entries, the file has 7",
            ),
            (format!("LUT_3D_SIZE 2\n{long}"), "more than the 8 entries"),
            (eight.clone(), "comes before LUT_3D_SIZE"),
            ("TITLE \"none\"\n".to_string(), "no LUT_3D_SIZE line"),
            (
                format!("LUT_3D_SIZE 2\nLUT_3D_SIZE 2\n{eight}"),
                "declared twice",
            ),
            ("LUT_3D_SIZE 1\n".to_string(), "at least 2"),
            ("LUT_3D_SIZE two\n".to_string(), "not a whole number"),
            (huge, "larger than the 129"),
            (
                format!("LUT_3D_SIZE 2\n{}0 0 nan\n", "0 0 0\n".repeat(7)),
                "not a finite number",
            ),
            (
                format!("LUT_3D_SIZE 2\n{}0 0 1e30\n", "0 0 0\n".repeat(7)),
                "outside",
            ),
            (
                format!("LUT_3D_SIZE 2\n{}0 0\n", "0 0 0\n".repeat(7)),
                "takes three numbers",
            ),
            (
                "LUT_1D_SIZE 4\n".to_string(),
                "1D .cube tables are not supported",
            ),
            (
                format!("LUT_3D_SIZE 2\nLUT_3D_INPUT_RANGE 0 1\n{eight}"),
                "unknown keyword `LUT_3D_INPUT_RANGE`",
            ),
            (
                format!("LUT_3D_SIZE 2\nDOMAIN_MIN 1 0 0\nDOMAIN_MAX 0 1 1\n{eight}"),
                "DOMAIN_MIN must be below",
            ),
        ];
        for (text, message) in table {
            let error = CubeLut::parse(text.as_bytes()).expect_err(message);
            let shown = match error {
                CubeError::Invalid(m) | CubeError::Unsupported(m) => m,
                CubeError::TooLarge { details, .. } => details,
            };
            assert!(shown.contains(message), "`{message}` not in `{shown}`");
        }
        assert!(matches!(
            CubeLut::parse(&vec![b' '; MAX_LUT_BYTES + 1]),
            Err(CubeError::TooLarge { .. })
        ));
        assert!(CubeLut::parse(&[0xff, 0xfe]).is_err());
    }
}
