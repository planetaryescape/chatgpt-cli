//! JavaScript's text and number semantics the TS CLI's output depends on:
//! `String.prototype.trim`, `\s`, and `Number(text)`.

/// JS whitespace and line terminators: `str::trim` plus U+FEFF, minus
/// U+0085, which Unicode calls whitespace and JS doesn't.
pub fn is_space(c: char) -> bool {
    c == '\u{FEFF}' || (c.is_whitespace() && c != '\u{85}')
}

/// `String.prototype.trim`.
pub fn trim(text: &str) -> &str {
    text.trim_matches(is_space)
}

/// `Number(text)`: NaN when JS would give NaN.
pub fn number(text: &str) -> f64 {
    let text = trim(text);
    if text.is_empty() {
        return 0.0;
    }
    match text {
        "Infinity" | "+Infinity" => return f64::INFINITY,
        "-Infinity" => return f64::NEG_INFINITY,
        _ => {}
    }
    for (prefix, radix) in [
        ("0x", 16),
        ("0X", 16),
        ("0o", 8),
        ("0O", 8),
        ("0b", 2),
        ("0B", 2),
    ] {
        if let Some(digits) = text.strip_prefix(prefix) {
            return u128::from_str_radix(digits, radix)
                .map(|n| n as f64)
                .unwrap_or(f64::NAN);
        }
    }
    // Rust also reads "inf", "nan" and "infinity"; JS reads none of them.
    let decimal = text
        .bytes()
        .all(|byte| byte.is_ascii_digit() || b"+-.eE".contains(&byte));
    if !decimal {
        return f64::NAN;
    }
    text.parse().unwrap_or(f64::NAN)
}

/// `text.replace(/\s+/g, " ")`: every run of JS whitespace as one space.
pub fn collapse_spaces(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut in_space = false;
    for c in text.chars() {
        if is_space(c) {
            if !in_space {
                out.push(' ');
            }
            in_space = true;
        } else {
            out.push(c);
            in_space = false;
        }
    }
    out
}

/// `text.length`: UTF-16 code units, as JS counts them.
pub fn utf16_len(text: &str) -> usize {
    text.encode_utf16().count()
}

/// The longest prefix of `text` within `units` UTF-16 units (`slice(0,
/// units)`) that doesn't split a character.
pub fn utf16_prefix(text: &str, units: usize) -> &str {
    let mut used = 0;
    for (at, c) in text.char_indices() {
        used += c.len_utf16();
        if used > units {
            return &text[..at];
        }
    }
    text
}

/// `review`'s `clip`: `text.replace(/\s+/g, " ").trim()`, cut to `max`
/// UTF-16 units with `…` (never through a character).
pub fn clip(text: &str, max: usize) -> String {
    let collapsed = collapse_spaces(text);
    let one_line = trim(&collapsed);
    if utf16_len(one_line) > max {
        format!("{}…", utf16_prefix(one_line, max))
    } else {
        one_line.to_owned()
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn clip_collapses_whitespace_and_cuts_in_utf16_units() {
        use super::clip;
        assert_eq!(clip("  a\n\n b\tc  ", 10), "a b c");
        assert_eq!(clip("abcdef", 3), "abc…");
        assert_eq!(clip("ab😀cd", 3), "ab…", "never half an emoji");
        assert_eq!(clip("abc", 3), "abc");
    }

    use super::*;

    #[test]
    fn utf16_prefixes_never_split_a_character() {
        let text = format!("{}😀tail", "a".repeat(249));
        assert_eq!(utf16_prefix(&text, 250), "a".repeat(249));
        assert_eq!(utf16_prefix("short", 250), "short");
        assert_eq!(utf16_len("😀a"), 3);
    }

    #[test]
    fn number_reads_what_js_reads() {
        assert_eq!(number("5"), 5.0);
        assert_eq!(number(" 5 "), 5.0);
        assert_eq!(number("1e2"), 100.0);
        assert_eq!(number("0x10"), 16.0);
        assert_eq!(number(""), 0.0);
        assert_eq!(number("2.5"), 2.5);
        assert!(number("abc").is_nan());
        assert!(number("inf").is_nan());
        assert!(number("5d").is_nan());
        assert_eq!(number("Infinity"), f64::INFINITY);
    }

    #[test]
    fn trim_matches_js() {
        assert_eq!(trim("\u{FEFF} x \n"), "x");
        assert_eq!(trim("\u{85}x"), "\u{85}x");
    }

    #[test]
    fn whitespace_runs_collapse_as_js_replace_does() {
        assert_eq!(collapse_spaces("a \n\t b\u{FEFF}\u{85}c"), "a b \u{85}c");
    }
}
