use std::borrow::Cow;

use crate::shape::TextSpan;

/// A case mapping of a span's text.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TextTransform {
    #[default]
    None,
    Uppercase,
    Lowercase,
    /// Uppercases the first letter or digit of each word, where words are separated by
    /// whitespace and continue across spans.
    Capitalize,
}

/// The text of each span after its transform. Untransformed text is borrowed.
pub(crate) fn transformed_texts(spans: &[TextSpan]) -> Vec<Cow<'_, str>> {
    let joined: String = if spans
        .iter()
        .any(|span| span.transform == TextTransform::Lowercase)
    {
        spans.iter().map(|span| span.text.as_str()).collect()
    } else {
        String::new()
    };
    let mut start = 0;
    let mut at_word_start = true;
    spans
        .iter()
        .map(|span| {
            let span_start = start;
            start += span.text.len();
            let mut capitalized = String::new();
            let capitalize = span.transform == TextTransform::Capitalize;
            for char in span.text.chars() {
                let starts_word = at_word_start && char.is_alphanumeric();
                if char.is_whitespace() {
                    at_word_start = true;
                } else if char.is_alphanumeric() {
                    at_word_start = false;
                }
                if capitalize {
                    if starts_word {
                        capitalized.extend(char.to_uppercase());
                    } else {
                        capitalized.push(char);
                    }
                }
            }
            match span.transform {
                TextTransform::None => Cow::Borrowed(span.text.as_str()),
                TextTransform::Uppercase => Cow::Owned(span.text.to_uppercase()),
                TextTransform::Lowercase => {
                    Cow::Owned(lowercase_in_context(&joined, span_start, &span.text))
                }
                TextTransform::Capitalize => Cow::Owned(capitalized),
            }
        })
        .collect()
}

/// Lowercases `text`, which starts at byte `start` of `joined`, like `str::to_lowercase` on
/// `joined`: a capital sigma is final when a cased letter precedes it and none follows it,
/// ignoring case-ignorable characters, even when that context is in a neighbouring span.
fn lowercase_in_context(joined: &str, start: usize, text: &str) -> String {
    let mut lowered = String::with_capacity(text.len());
    for (offset, char) in text.char_indices() {
        if char != '\u{3A3}' {
            lowered.extend(char.to_lowercase());
            continue;
        }
        let at = start + offset;
        let is_final = cased_after_ignorable(joined[..at].chars().rev())
            && !cased_after_ignorable(joined[at + char.len_utf8()..].chars());
        lowered.push(if is_final { '\u{3C2}' } else { '\u{3C3}' });
    }
    lowered
}

fn cased_after_ignorable(mut chars: impl Iterator<Item = char>) -> bool {
    chars
        .find(|&char| !is_case_ignorable(char))
        .is_some_and(is_cased)
}

// `char` has no stable cased or case-ignorable predicates, so ask `str::to_lowercase`, whose
// final-sigma rule is defined by them.
fn is_cased(char: char) -> bool {
    format!("{char}\u{3A3}").to_lowercase().ends_with('\u{3C2}')
}

fn is_case_ignorable(char: char) -> bool {
    !is_cased(char)
        && format!("A{char}\u{3A3}")
            .to_lowercase()
            .ends_with('\u{3C2}')
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::shape::tests::span;
    use crate::{
        TextDecoration, TextDecorationKind, TextLayoutRequest, TextLineDecoration, TextLineStyle,
        TextSystem,
    };

    #[test]
    fn uppercase_keeps_numbering_in_the_transformed_text() {
        let mut system = TextSystem::new();
        let shaped = system.shape(&TextLayoutRequest::from_spans(vec![
            TextSpan {
                transform: TextTransform::Uppercase,
                ..span("Maßﬁ ", 30.0)
            },
            span("end", 30.0),
        ]));
        // "ß" becomes "SS" and "ﬁ" "FI", so the span is "MASSFI " in seven bytes, not eight.
        let layout = shaped.place([0.0; 2]);
        assert_eq!(shaped.span_starts, [0, 7, 10]);
        assert_eq!(shaped.counts.glyph, 10);
        assert_eq!(shaped.counts.word, 2);
        let items: Vec<_> = layout
            .glyphs
            .iter()
            .map(|glyph| (glyph.item.glyph, glyph.item.word, glyph.item.span))
            .collect();
        let expected: Vec<_> = (0..10)
            .map(|glyph| (glyph, u32::from(glyph >= 7), u32::from(glyph >= 7)))
            .collect();
        assert_eq!(items, expected);
        assert_eq!(shaped.segments[0].bytes, 0..7);
        assert_eq!(shaped.segments[1].bytes, 0..3);

        // An underline over the second span starts where its first glyph does, and one over
        // half of the first covers half of its width.
        let underline = |spans, extent| TextDecoration {
            kind: TextDecorationKind::Underline(TextLineDecoration {
                style: TextLineStyle::Solid,
                thickness: None,
                offset: None,
            }),
            spans,
            shifted: false,
            extent,
        };
        let rect = |decoration| {
            let mut instances = Vec::new();
            shaped.decorate(&decoration, [0.0; 2], |_| 0, &mut instances);
            instances[0].rect
        };
        let [second, first] = [
            rect(underline(1..2, [0.0, 1.0])),
            rect(underline(0..1, [0.0, 0.5])),
        ];
        assert!((second[0] - shaped.segments[1].x[0]).abs() < 0.01);
        let width = shaped.segments[0].x[1] - shaped.segments[0].x[0];
        assert!((first[2] - width * 0.5).abs() < 0.01);
    }

    #[test]
    fn capitalize_uppercases_each_word_across_spans() {
        let capitalized = |texts: &[&str], transform| {
            let spans: Vec<_> = texts
                .iter()
                .map(|text| TextSpan {
                    transform,
                    ..TextSpan::new(*text)
                })
                .collect();
            transformed_texts(&spans).concat()
        };
        assert_eq!(
            capitalized(
                &["hello wor", "ld, don't (stop)"],
                TextTransform::Capitalize
            ),
            "Hello World, Don't (Stop)"
        );
        assert_eq!(
            capitalized(&["Hello ", "WORLD"], TextTransform::Lowercase),
            "hello world"
        );
    }

    #[test]
    fn lowercase_decides_final_sigma_across_spans() {
        let lowercased = |texts: &[&str]| {
            let spans: Vec<_> = texts
                .iter()
                .map(|text| TextSpan {
                    transform: TextTransform::Lowercase,
                    ..TextSpan::new(*text)
                })
                .collect();
            transformed_texts(&spans).concat()
        };
        for whole in ["ΟΣ", "ΣΑ", "Ο.Σ.Α", "ΟΣ\u{301}", "ΣΣ", "İΣ", "A\u{345}Σ"] {
            let expected = whole.to_lowercase();
            for (split, _) in whole.char_indices().skip(1) {
                let (first, second) = whole.split_at(split);
                assert_eq!(lowercased(&[first, second]), expected);
            }
        }
        assert_eq!(lowercased(&["Ο", "Σ"]), "ος");
        assert_eq!(lowercased(&["Σ", "Α"]), "σα");
    }
}
