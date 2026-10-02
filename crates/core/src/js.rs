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

#[cfg(test)]
mod tests {
    use super::*;

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
