//! `new RegExp(source, flags)` without the `u` flag, on fancy-regex.
//!
//! Such a JS regex matches UTF-16 code units: `.` takes half an emoji,
//! `^..$` matches `👍`, and `[^a]` matches a lone surrogate. fancy-regex
//! matches whole characters, so both the text and the pattern are first
//! spelled in code units ([`to_units`]): each astral character becomes two
//! stand-in characters, one per surrogate, from the start of plane 15.
//! Every real astral character is spelled that way too, so a stand-in
//! never collides with one, and [`from_units`] turns pairs back into
//! characters (a lone half becomes U+FFFD, as writing it out as UTF-8 does).
//!
//! `regress`, an ECMAScript engine with a UTF-16 mode, was the alternative.
//! It backtracks without a budget: `^(a+)+$` against 26 `a`s and a `!`
//! takes 1.4 s and doubles with each `a`, and a canvas pattern runs inside
//! the daemon. fancy-regex hands everything but lookaround and
//! backreferences to a linear-time engine and gives up on the rest after
//! a backtracking budget, so a pathological pattern costs microseconds.

use std::borrow::Cow;

/// JS's `\s`: WhiteSpace and LineTerminator, without U+0085.
const JS_SPACE: &str = r"\t\n\x0B\x0C\r \x{A0}\x{1680}\x{2000}-\x{200A}\x{2028}\x{2029}\x{202F}\x{205F}\x{3000}\x{FEFF}";
/// JS's `\w` without the `u` flag.
const WORD: &str = "0-9A-Za-z_";
const WORD_CLASS: &str = "[0-9A-Za-z_]";
/// The stand-in for surrogate `0xD800 + n` is `STAND_IN + n`.
const STAND_IN: u32 = 0xF0000;
const SURROGATE_FIRST: u32 = 0xD800;
const SURROGATE_LAST: u32 = 0xDFFF;

/// A JS regex source the translation can't express, with V8's message.
#[derive(Debug, thiserror::Error)]
pub enum RegexError {
    #[error("{0}")]
    Syntax(&'static str),
    #[error(transparent)]
    Engine(#[from] fancy_regex::Error),
}

/// A compiled JS regex. A match that runs out of backtracking budget
/// counts as no match.
#[derive(Debug)]
pub struct JsRegex(fancy_regex::Regex);

/// `new RegExp(source, flags)` for flags `""` or `"i"`: every JS regex the
/// TS CLI built goes through here, so none skips the translation.
pub fn regex(source: &str, case_insensitive: bool) -> Result<JsRegex, RegexError> {
    let translated = regex_source(&to_units(source))?;
    Ok(JsRegex(
        fancy_regex::RegexBuilder::new(&translated)
            .case_insensitive(case_insensitive)
            .build()?,
    ))
}

impl JsRegex {
    /// `regex.test(text)`.
    pub fn is_match(&self, text: &str) -> bool {
        self.0.is_match(&to_units(text)).unwrap_or(false)
    }

    /// The first match's text (`text.match(regex)?.[0]`).
    pub fn find(&self, text: &str) -> Option<String> {
        let units = to_units(text);
        let found = self.0.find(&units).ok()??.as_str();
        Some(match &units {
            Cow::Borrowed(_) => found.to_owned(),
            Cow::Owned(_) => from_units(found),
        })
    }

    /// `text = text.replace(regex, () => replacement)`, at most `limit`
    /// times (0 for every match). `None`, leaving `text` as it was, when
    /// the budget runs out.
    pub fn replacen(&self, text: &mut JsString, limit: usize, replacement: &str) -> Option<()> {
        let replacement = to_units(replacement);
        let replaced = self
            .0
            .try_replacen(&text.0, limit, fancy_regex::NoExpand(&replacement))
            .ok()?
            .into_owned();
        text.0 = replaced;
        Some(())
    }
}

/// A string as JS holds it, in UTF-16 code units: half an emoji left by
/// one edit is still there for the next, and becomes U+FFFD only when the
/// text is written out ([`JsString::into_string`]), as JS's UTF-8 output
/// does.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct JsString(String);

impl JsString {
    pub fn new(text: &str) -> Self {
        Self(to_units(text).into_owned())
    }

    pub fn into_string(self) -> String {
        from_units(&self.0)
    }
}

fn stand_in(surrogate: u16) -> char {
    char::from_u32(STAND_IN + u32::from(surrogate) - SURROGATE_FIRST)
        .unwrap_or(char::REPLACEMENT_CHARACTER)
}

/// The surrogate a stand-in spells, if `c` is one.
fn surrogate(c: char) -> Option<u16> {
    let offset = u32::from(c).checked_sub(STAND_IN)?;
    if offset > SURROGATE_LAST - SURROGATE_FIRST {
        return None;
    }
    u16::try_from(SURROGATE_FIRST + offset).ok()
}

/// `text` in code units: each astral character as its two surrogates'
/// stand-ins. Borrowed when there are none.
fn to_units(text: &str) -> Cow<'_, str> {
    // Astral characters are exactly UTF-8's four-byte sequences.
    if !text.bytes().any(|byte| byte >= 0xF0) {
        return Cow::Borrowed(text);
    }
    let mut out = String::with_capacity(text.len() * 2);
    let mut buffer = [0u16; 2];
    for c in text.chars() {
        match c.encode_utf16(&mut buffer) {
            [high, low] => {
                out.push(stand_in(*high));
                out.push(stand_in(*low));
            }
            _ => out.push(c),
        }
    }
    Cow::Owned(out)
}

/// [`to_units`] undone: stand-in pairs become their character again, and a
/// lone stand-in U+FFFD.
fn from_units(text: &str) -> String {
    let mut units = Vec::with_capacity(text.len());
    let mut buffer = [0u16; 2];
    for c in text.chars() {
        match surrogate(c) {
            Some(unit) => units.push(unit),
            None => units.extend_from_slice(c.encode_utf16(&mut buffer)),
        }
    }
    String::from_utf16_lossy(&units)
}

/// Rust regex syntax for one code unit: a stand-in for a surrogate.
fn unit_syntax(unit: u32) -> String {
    match u16::try_from(unit) {
        Ok(surrogate) if (SURROGATE_FIRST..=SURROGATE_LAST).contains(&unit) => {
            format!(r"\x{{{:X}}}", u32::from(stand_in(surrogate)))
        }
        _ => format!(r"\x{{{unit:X}}}"),
    }
}

/// One member of a JS character class.
enum ClassAtom {
    Unit(u32),
    /// A class escape (`\w`, `\D`, …) as Rust class syntax.
    Set(String),
}

/// A code-unit spelled JS regex source ([`to_units`]) rewritten for
/// fancy-regex, so it matches what `new RegExp(source)` would: `\w`, `\d`
/// and `\b` are ASCII-only, `\s` is JS's whitespace set, `.` stops at
/// every JS line terminator and takes one code unit, and a class takes one
/// code unit, with JS's (Annex B) reading of its escapes. Everything else
/// passes through.
fn regex_source(source: &str) -> Result<String, RegexError> {
    let mut out = String::with_capacity(source.len() + 16);
    let mut chars = source.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\\' => {
                let Some(next) = chars.next() else {
                    out.push('\\');
                    break;
                };
                let word = WORD_CLASS;
                match next {
                    'w' => out.push_str(word),
                    'W' => out.push_str(&format!("[^{WORD}]")),
                    'd' => out.push_str("[0-9]"),
                    'D' => out.push_str("[^0-9]"),
                    // fancy-regex has no `(?-u:\b)`: spell the ASCII
                    // boundary out with lookarounds.
                    'b' => {
                        out.push_str(&format!("(?:(?<={word})(?!{word})|(?<!{word})(?={word}))"))
                    }
                    'B' => {
                        out.push_str(&format!("(?:(?<={word})(?={word})|(?<!{word})(?!{word}))"))
                    }
                    's' => out.push_str(&format!("[{JS_SPACE}]")),
                    'S' => out.push_str(&format!("[^{JS_SPACE}]")),
                    'u' => match hex(&mut chars, 4) {
                        Some(unit) => out.push_str(&unit_syntax(unit)),
                        None => out.push('u'),
                    },
                    // An identity escape of a non-ASCII character (a
                    // stand-in included) is that character.
                    other if !other.is_ascii() => out.push_str(&unit_syntax(u32::from(other))),
                    other => {
                        out.push('\\');
                        out.push(other);
                    }
                }
            }
            '[' => class(&mut chars, &mut out)?,
            '.' => out.push_str(r"[^\n\r\x{2028}\x{2029}]"),
            _ => out.push(c),
        }
    }
    Ok(out)
}

type Chars<'a> = std::iter::Peekable<std::str::Chars<'a>>;

/// Exactly `digits` hex digits, consumed only when all are there.
fn hex(chars: &mut Chars<'_>, digits: usize) -> Option<u32> {
    let ahead: String = chars.clone().take(digits).collect();
    if ahead.len() != digits || !ahead.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    chars.nth(digits - 1);
    u32::from_str_radix(&ahead, 16).ok()
}

/// A class after its `[`, through its `]`, as Rust class syntax. Members
/// are written as `\x{…}` escapes, so none reads as Rust class syntax
/// (`[`, `&&`, `--`, `~~`).
fn class(chars: &mut Chars<'_>, out: &mut String) -> Result<(), RegexError> {
    let negated = chars.next_if_eq(&'^').is_some();
    if chars.next_if_eq(&']').is_some() {
        // `[]` matches nothing and `[^]` any code unit.
        out.push_str(if negated {
            r"[\x{0}-\x{10FFFF}]"
        } else {
            r"[^\x{0}-\x{10FFFF}]"
        });
        return Ok(());
    }
    out.push('[');
    if negated {
        out.push('^');
    }
    loop {
        let first = match chars.peek() {
            None => return Err(RegexError::Syntax("Unterminated character class")),
            Some(']') => {
                chars.next();
                break;
            }
            Some(_) => class_atom(chars),
        };
        let range = chars.peek() == Some(&'-') && {
            let mut ahead = chars.clone();
            ahead.next();
            ahead.peek().is_some_and(|&c| c != ']')
        };
        if !range {
            push_atom(out, &first);
            continue;
        }
        chars.next();
        let last = class_atom(chars);
        match (first, last) {
            (ClassAtom::Unit(low), ClassAtom::Unit(high)) => {
                if low > high {
                    return Err(RegexError::Syntax("Range out of order in character class"));
                }
                push_range(out, low, high);
            }
            // Annex B: a range with a class escape at either end is the
            // two ends and a `-`.
            (first, last) => {
                push_atom(out, &first);
                push_atom(out, &ClassAtom::Unit(u32::from('-')));
                push_atom(out, &last);
            }
        }
    }
    out.push(']');
    Ok(())
}

fn push_atom(out: &mut String, atom: &ClassAtom) {
    match atom {
        ClassAtom::Unit(unit) => out.push_str(&unit_syntax(*unit)),
        ClassAtom::Set(set) => out.push_str(set),
    }
}

/// `low-high` in code units: the surrogates in it as their stand-ins.
fn push_range(out: &mut String, low: u32, high: u32) {
    let pieces = [
        (low, high.min(SURROGATE_FIRST - 1)),
        (low.max(SURROGATE_FIRST), high.min(SURROGATE_LAST)),
        (low.max(SURROGATE_LAST + 1), high),
    ];
    for (from, to) in pieces {
        if from <= to {
            out.push_str(&format!("{}-{}", unit_syntax(from), unit_syntax(to)));
        }
    }
}

/// One class member, which the caller has seen is there.
fn class_atom(chars: &mut Chars<'_>) -> ClassAtom {
    let unit = |c: char| ClassAtom::Unit(surrogate(c).map_or(u32::from(c), u32::from));
    let Some(c) = chars.next() else {
        return ClassAtom::Unit(0);
    };
    if c != '\\' {
        return unit(c);
    }
    // Annex B: `\c` without a letter, digit or `_` after it is a backslash,
    // and the `c` is read next.
    let mut ahead = chars.clone();
    if ahead.next() == Some('c')
        && !ahead
            .peek()
            .is_some_and(|&c| c.is_ascii_alphanumeric() || c == '_')
    {
        return ClassAtom::Unit(u32::from('\\'));
    }
    let Some(next) = chars.next() else {
        return ClassAtom::Unit(u32::from('\\'));
    };
    match next {
        'w' => ClassAtom::Set(WORD.to_owned()),
        'W' => ClassAtom::Set(format!("[^{WORD}]")),
        'd' => ClassAtom::Set("0-9".to_owned()),
        'D' => ClassAtom::Set("[^0-9]".to_owned()),
        's' => ClassAtom::Set(JS_SPACE.to_owned()),
        'S' => ClassAtom::Set(format!("[^{JS_SPACE}]")),
        // In a class, `\b` is a backspace and `\B` an identity escape.
        'b' => ClassAtom::Unit(0x08),
        'n' => ClassAtom::Unit(0x0A),
        'r' => ClassAtom::Unit(0x0D),
        't' => ClassAtom::Unit(0x09),
        'f' => ClassAtom::Unit(0x0C),
        'v' => ClassAtom::Unit(0x0B),
        'x' => ClassAtom::Unit(hex(chars, 2).unwrap_or(u32::from('x'))),
        'u' => ClassAtom::Unit(hex(chars, 4).unwrap_or(u32::from('u'))),
        // `\cX` is a control character (the letter was checked above).
        'c' => ClassAtom::Unit(
            chars
                .next()
                .map_or(u32::from('c'), |letter| u32::from(letter) % 32),
        ),
        // A legacy octal escape: at most three digits, at most 0o377.
        '0'..='7' => {
            let mut value = u32::from(next) - u32::from('0');
            let mut digits = 1;
            while let Some(digit) = chars.peek().and_then(|c| c.to_digit(8)) {
                if digits == 3 || value * 8 + digit > 0o377 {
                    break;
                }
                digits += 1;
                value = value * 8 + digit;
                chars.next();
            }
            ClassAtom::Unit(value)
        }
        other => unit(other),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn js(source: &str) -> JsRegex {
        regex(source, false).expect("valid")
    }

    #[test]
    fn regexes_match_as_js_regexes_without_the_u_flag() {
        assert_eq!(js(r"\w+").find("café").as_deref(), Some("caf"));
        assert!(js(r"\bé").is_match("café"), "é isn't a JS word character");
        assert!(!js(r"\d").is_match("٣"), "only ASCII digits");
        assert!(js(r"a.b").is_match("a\u{85}b"));
        assert!(!js(r"a.b").is_match("a\u{2028}b"));
        assert!(js(r"\s").is_match("\u{FEFF}"));
        assert!(!js(r"\s").is_match("\u{85}"));
        assert!(js(r"[\w-]+$").is_match("a-b"));
        assert!(js(r"[^.]x").is_match("ax"), "a class keeps its own dot");
        assert!(js(r"\.").is_match("."));
    }

    // docs/issues/js-regex-negated-classes.md
    #[test]
    fn negated_shorthands_in_a_class_are_ascii_as_in_js() {
        assert!(js(r"[\W]").is_match("é"), "é is a JS non-word character");
        assert!(!js(r"[^\W]").is_match("é"));
        assert!(js(r"^[\D]$").is_match("٣"), "only ASCII digits are digits");
        assert!(js(r"^[\S]$").is_match("\u{85}"), "U+0085 isn't JS space");
        assert!(!js(r"[\S]").is_match("\u{FEFF}"));
        assert!(js(r"^[\W\d]+$").is_match("é1"));
        // In a class `\b` is a backspace and `\B` an identity escape.
        assert!(js(r"[\b]").is_match("\u{8}"));
        assert!(!js(r"[\b]").is_match("b"));
        assert!(js(r"^[\B]$").is_match("B"));
    }

    // docs/issues/cubic-pr3-followups.md (`js.rs:125`)
    #[test]
    fn dot_and_classes_take_one_utf16_code_unit() {
        assert!(js("^..$").is_match("👍"), "an emoji is two units");
        assert!(!js("^.$").is_match("👍"));
        assert!(js("^.{3}$").is_match("a👍"));
        assert!(js("^[^x][^x]$").is_match("👍"));
        assert!(js(r"^\W\W$").is_match("👍"), "surrogates aren't word units");
        assert!(js(r"^[\uD800-\uDBFF][\uDC00-\uDFFF]$").is_match("👍"));
        assert!(js(r"^👍$").is_match("👍"));
        assert!(js(r"^[\u0000-￿]+$").is_match("a👍b"));
        assert!(js("^👍+$").is_match("👍"));
        // A quantifier after an astral character repeats its second half.
        assert!(!js("^👍{2}$").is_match("👍👍"));
        assert!(js("^(?:👍){2}$").is_match("👍👍"));
        // A class holding an emoji holds its two halves.
        assert!(js("^[👍]{2}$").is_match("👍"));
        assert_eq!(
            regex("[😂-😀]", false).expect_err("halves").to_string(),
            "Range out of order in character class"
        );
    }

    #[test]
    fn replacing_half_an_emoji_leaves_a_replacement_character_when_written() {
        let replaced = |pattern: &str, text: &str, limit: usize, with: &str| {
            let mut text = JsString::new(text);
            js(pattern)
                .replacen(&mut text, limit, with)
                .expect("in budget");
            text.into_string()
        };
        assert_eq!(replaced("^.", "👍!", 1, "x"), "x\u{FFFD}!");
        assert_eq!(replaced("b", "a😀b", 0, "😂"), "a😀😂");
        // Text in the stand-ins' own range comes through untouched.
        let private = "\u{F0001}\u{F0400}";
        assert_eq!(replaced("x", private, 0, "y"), private);
        assert_eq!(js("..").find(private).as_deref(), Some("\u{F0001}"));
    }

    // The coordinator's repro, as node runs it: `d = "👍"; d =
    // d.replace(/^./, () => ""); d = d.replace(/^\uDC4D$/, () => "OK")` is
    // "OK". The half emoji the first edit leaves is still there for the
    // second.
    #[test]
    fn half_an_emoji_survives_from_one_edit_to_the_next() {
        let mut doc = JsString::new("👍");
        js("^.").replacen(&mut doc, 1, "").expect("in budget");
        js(r"^\uDC4D$")
            .replacen(&mut doc, 1, "OK")
            .expect("in budget");
        assert_eq!(doc.into_string(), "OK");
    }

    // Annex B reads at most three octal digits, up to 0o377. Expected
    // values from node: `/^[\0000]+$/.test("\0" + "0")` and so on.
    #[test]
    fn legacy_octal_escapes_read_at_most_three_digits() {
        assert!(js(r"^[\0000]+$").is_match("\u{0}0"));
        assert!(js(r"^[\0000]$").is_match("\u{0}"));
        assert!(js(r"^[\0100]+$").is_match("\u{8}0"));
        assert!(!js(r"^[\0100]$").is_match("@"));
        assert!(js(r"^[\400]+$").is_match(" 0"));
        assert!(js(r"^[\377]$").is_match("\u{FF}"));
        assert!(js(r"^[\08]+$").is_match("\u{0}8"));
    }

    #[test]
    fn class_edge_cases_read_as_js_reads_them() {
        assert!(!js("a[]").is_match("a"), "[] matches nothing");
        assert!(js("a[^]b").is_match("a\nb"), "[^] matches anything");
        assert!(js(r"^[[]$").is_match("["), "[ is literal in a JS class");
        assert!(js(r"^[a&&b]+$").is_match("a&&b"), "no Rust set operators");
        assert!(js(r"^[\x41B\103\cJ]+$").is_match("ABC\n"));
        assert!(js(r"^[\c]+$").is_match("\\c"), "a lone \\c is a backslash");
        assert!(js(r"^[a-]+$").is_match("a-"));
        assert!(js(r"^[\w-.]+$").is_match("a-."), "Annex B: no range");
        assert!(regex("[a", false).is_err());
        assert!(regex("[z-a]", false).is_err());
    }

    #[test]
    fn a_pathological_pattern_gives_up_instead_of_hanging() {
        let text = format!("{}!", "a".repeat(64));
        let started = std::time::Instant::now();
        assert!(!js("^(a+)+$").is_match(&text));
        assert!(!js(r"^(a+)+\1$").is_match(&text));
        assert!(started.elapsed() < std::time::Duration::from_secs(5));
    }
}
