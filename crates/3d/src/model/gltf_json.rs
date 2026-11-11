//! Recognising a glTF JSON (`.gltf`) file, which is not read, from the start of its bytes.

/// How much of a file's start [`is_gltf_json`] looks at.
pub const GLTF_JSON_SNIFF_BYTES: usize = 64 << 10;

/// Whether `bytes` is a glTF JSON document: a top-level object with an `asset` object that has a
/// `version`. It looks at the first [`GLTF_JSON_SNIFF_BYTES`] bytes only and does not parse the
/// rest, so a document that holds more than that before its `asset` is not recognised, and nor is
/// anything malformed within that start.
pub fn is_gltf_json(bytes: &[u8]) -> bool {
    let bytes = &bytes[..bytes.len().min(GLTF_JSON_SNIFF_BYTES)];
    Scanner { bytes, at: 0 }
        .top_level_asset_has_version()
        .is_some()
}

struct Scanner<'a> {
    bytes: &'a [u8],
    at: usize,
}

impl<'a> Scanner<'a> {
    fn top_level_asset_has_version(&mut self) -> Option<()> {
        self.expect(b'{')?;
        loop {
            let key = self.string()?;
            self.expect(b':')?;
            if key == b"asset" {
                self.expect(b'{')?;
                if self.object_has_version()? {
                    return Some(());
                }
            } else {
                self.skip_value()?;
            }
            if !self.comma()? {
                return None;
            }
        }
    }

    /// Whether the object the scanner is inside has a string `version`, reading to its end.
    fn object_has_version(&mut self) -> Option<bool> {
        let mut found = false;
        loop {
            self.skip_whitespace();
            if self.peek()? == b'}' {
                self.at += 1;
                return Some(found);
            }
            let key = self.string()?;
            self.expect(b':')?;
            self.skip_whitespace();
            if key == b"version" && self.peek()? == b'"' {
                found = true;
            }
            self.skip_value()?;
            self.skip_whitespace();
            match self.peek()? {
                b',' => self.at += 1,
                b'}' => {}
                _ => return None,
            }
        }
    }

    /// Reads a `,` and returns true, or reads nothing before a `}` and returns false.
    fn comma(&mut self) -> Option<bool> {
        self.skip_whitespace();
        match self.peek()? {
            b',' => {
                self.at += 1;
                Some(true)
            }
            b'}' => Some(false),
            _ => None,
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.at).copied()
    }

    fn skip_whitespace(&mut self) {
        while self.peek().is_some_and(|byte| byte.is_ascii_whitespace()) {
            self.at += 1;
        }
    }

    fn expect(&mut self, byte: u8) -> Option<()> {
        self.skip_whitespace();
        (self.peek()? == byte).then(|| self.at += 1)
    }

    /// The raw text of the string at the scanner, without its quotes.
    fn string(&mut self) -> Option<&'a [u8]> {
        self.expect(b'"')?;
        let start = self.at;
        loop {
            match self.peek()? {
                b'"' => {
                    self.at += 1;
                    return Some(&self.bytes[start..self.at - 1]);
                }
                b'\\' => self.at += 2,
                _ => self.at += 1,
            }
        }
    }

    fn skip_value(&mut self) -> Option<()> {
        self.skip_whitespace();
        match self.peek()? {
            b'"' => self.string().map(|_| ()),
            b'{' | b'[' => {
                let mut depth = 0_usize;
                loop {
                    match self.peek()? {
                        b'"' => {
                            self.string()?;
                            continue;
                        }
                        b'{' | b'[' => depth += 1,
                        b'}' | b']' => depth -= 1,
                        _ => {}
                    }
                    self.at += 1;
                    if depth == 0 {
                        return Some(());
                    }
                }
            }
            _ => {
                while !matches!(self.peek()?, b',' | b'}' | b']') {
                    self.at += 1;
                }
                Some(())
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_gltf_json_document_has_an_asset_with_a_version() {
        assert!(is_gltf_json(br#"{"asset":{"version":"2.0"}}"#));
        assert!(is_gltf_json(
            b"  {\n \"scenes\": [{\"nodes\": [0]}], \"buffers\": [{\"uri\": \"a\\\"}\"}],\n \"asset\" : { \"generator\": \"x\", \"version\": \"2.0\" } }"
        ));
        // A document cut off after its asset, as the start of a larger one is.
        assert!(is_gltf_json(
            br#"{"asset":{"version":"2.0"},"scenes":[{"nodes"#
        ));
    }

    #[test]
    fn anything_else_that_starts_with_a_brace_is_not() {
        for bytes in [
            &b"{garbage"[..],
            br#"{"name": "a package", "version": "1.0"}"#,
            br#"{"asset": {"generator": "x"}}"#,
            br#"{"asset": "2.0"}"#,
            br#"{"nested": {"asset": {"version": "2.0"}}}"#,
            b"{",
            b"",
            b"[1, 2]",
        ] {
            assert!(!is_gltf_json(bytes), "{}", String::from_utf8_lossy(bytes));
        }
    }

    #[test]
    fn only_the_start_is_looked_at() {
        let mut bytes = br#"{"buffers":[{"uri":""#.to_vec();
        bytes.resize(GLTF_JSON_SNIFF_BYTES + 100, b'a');
        bytes.extend_from_slice(br#""}],"asset":{"version":"2.0"}}"#);
        assert!(!is_gltf_json(&bytes));
    }
}
