//! `transcriptChunks` from the TS CLI's `src/search/chunks.ts` @ 1b8c950,
//! and the bytes Bun stores for each chunk.
//!
//! JS strings are UTF-16, and the TS CLI cuts chunks at UTF-16 offsets, so
//! a cut can fall inside an emoji's surrogate pair. Chunking here works on
//! UTF-16 code units for the same cuts, and [`bun_sqlite_text`] encodes a
//! chunk the way Bun binds such a string to SQLite. Matching those bytes
//! keeps the FTS index, its bm25 ranking and its snippets the TS CLI's.

/// Bump when the text sent to FTS changes (`CHUNK_VERSION`).
pub const CHUNK_VERSION: u32 = 1;
const MAX_CHUNK_CHARS: usize = 800;
const OVERLAP_CHARS: usize = 80;

/// The turn separator `renderTranscript` puts between sections, as
/// `transcriptChunks` splits on it (`/\n---\n\n/`).
const TURN_SEPARATOR: &str = "\n---\n\n";

/// A transcript's chunks, each as UTF-16: every turn and canvas (the header
/// dropped) cut into pieces of at most 800 code units, preferably at a
/// space or line break, overlapping by 80.
pub fn transcript_chunks(markdown: &str) -> Vec<Vec<u16>> {
    let messages: Vec<&str> = markdown.split(TURN_SEPARATOR).skip(1).collect();
    let sections = if messages.is_empty() {
        vec![markdown]
    } else {
        messages
    };
    let mut chunks = Vec::new();
    for section in sections {
        let text: Vec<u16> = chatgpt_core::js::trim(section).encode_utf16().collect();
        let mut start = 0;
        while start < text.len() {
            let mut end = (start + MAX_CHUNK_CHARS).min(text.len());
            if end < text.len() {
                let boundary =
                    last_index_of(&text, b'\n', end).max(last_index_of(&text, b' ', end));
                if let Some(at) = boundary.filter(|&at| at > start + MAX_CHUNK_CHARS / 2) {
                    end = at;
                }
            }
            let chunk = trim_units(&text[start..end]);
            if !chunk.is_empty() {
                chunks.push(chunk.to_vec());
            }
            if end == text.len() {
                break;
            }
            start = (start + 1).max(end.saturating_sub(OVERLAP_CHARS));
        }
    }
    if chunks.is_empty() {
        vec![chatgpt_core::js::trim(markdown).encode_utf16().collect()]
    } else {
        chunks
    }
}

/// `text.lastIndexOf(c, from)`: the last `c` at or before `from`.
fn last_index_of(text: &[u16], c: u8, from: usize) -> Option<usize> {
    let end = from.min(text.len().saturating_sub(1));
    text.get(..=end)?
        .iter()
        .rposition(|&unit| unit == u16::from(c))
}

/// `trim()` on a string that may hold half a surrogate pair at either end.
/// JS whitespace is all in the BMP, so this trims code units.
fn trim_units(units: &[u16]) -> &[u16] {
    let space =
        |unit: &u16| char::from_u32(u32::from(*unit)).is_some_and(chatgpt_core::js::is_space);
    let start = units
        .iter()
        .position(|unit| !space(unit))
        .unwrap_or(units.len());
    let end = units
        .iter()
        .rposition(|unit| !space(unit))
        .map_or(start, |at| at + 1);
    &units[start..end]
}

/// The bytes Bun (1.3) gives SQLite for a JS string. Valid UTF-16 becomes
/// UTF-8. A surrogate followed by any code unit is combined with it as if
/// they were a pair (`0x10000 + (hi & 0x3FF) << 10 | lo & 0x3FF`); a
/// surrogate at the very end becomes its three-byte form, which isn't valid
/// UTF-8. Observed with `select hex(?)` from `bun:sqlite`.
pub fn bun_sqlite_text(units: &[u16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(units.len() + units.len() / 2);
    let mut index = 0;
    while let Some(&unit) = units.get(index) {
        let surrogate = (0xD800..=0xDFFF).contains(&unit);
        let code_point = match units.get(index + 1) {
            Some(&next) if surrogate => {
                index += 2;
                0x10000 + ((u32::from(unit & 0x3FF) << 10) | u32::from(next & 0x3FF))
            }
            _ => {
                index += 1;
                u32::from(unit)
            }
        };
        push_utf8(&mut out, code_point);
    }
    out
}

/// UTF-8's encoding of `code_point`, surrogates included (as CESU-8 would).
fn push_utf8(out: &mut Vec<u8>, code_point: u32) {
    // Each `as u8` takes six or fewer bits, by the masks and shifts.
    match code_point {
        0..=0x7F => out.push(code_point as u8),
        0x80..=0x7FF => out.extend([
            0xC0 | (code_point >> 6) as u8,
            0x80 | (code_point & 0x3F) as u8,
        ]),
        0x800..=0xFFFF => out.extend([
            0xE0 | (code_point >> 12) as u8,
            0x80 | ((code_point >> 6) & 0x3F) as u8,
            0x80 | (code_point & 0x3F) as u8,
        ]),
        _ => out.extend([
            0xF0 | (code_point >> 18) as u8,
            0x80 | ((code_point >> 12) & 0x3F) as u8,
            0x80 | ((code_point >> 6) & 0x3F) as u8,
            0x80 | (code_point & 0x3F) as u8,
        ]),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(units: &[u16]) -> String {
        String::from_utf16_lossy(units)
    }

    #[test]
    fn turns_are_chunked_without_the_header() {
        let markdown = "# Title\n\nhttps://chatgpt.com/c/x · 2026-01-01 · gpt\n\n---\n\n## Me\n\nhello\n\n---\n\n## ChatGPT\n\n  hi there  \n";
        let chunks: Vec<String> = transcript_chunks(markdown)
            .iter()
            .map(|c| text(c))
            .collect();
        assert_eq!(chunks, ["## Me\n\nhello", "## ChatGPT\n\n  hi there"]);
        assert_eq!(
            text(&transcript_chunks("  just a header ")[0]),
            "just a header"
        );
    }

    #[test]
    fn long_turns_split_at_a_space_and_overlap() {
        let words: String = (0..300).map(|n| format!("w{n:03} ")).collect();
        let markdown = format!("# T\n\n---\n\n{words}");
        let chunks: Vec<String> = transcript_chunks(&markdown)
            .iter()
            .map(|c| text(c))
            .collect();
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.len() <= MAX_CHUNK_CHARS);
            assert!(!chunk.starts_with(' ') && !chunk.ends_with(' '));
        }
        // The first ends at the space at 799; the next starts 80 units
        // earlier, at the space before w144.
        assert!(chunks[0].starts_with("w000 ") && chunks[0].ends_with("w159"));
        assert!(chunks[1].starts_with("w144 "));
    }

    #[test]
    fn a_long_run_without_spaces_is_cut_at_800_units() {
        let markdown = format!("# T\n\n---\n\n{}", "x".repeat(1700));
        let lengths: Vec<usize> = transcript_chunks(&markdown).iter().map(Vec::len).collect();
        assert_eq!(lengths, [800, 800, 260]);
    }

    #[test]
    fn surrogates_encode_as_bun_binds_them() {
        let encode = |s: &[u16]| bun_sqlite_text(s);
        let hex = |bytes: Vec<u8>| bytes.iter().map(|b| format!("{b:02X}")).collect::<String>();
        // Each case observed from bun:sqlite's `select hex(?)`.
        assert_eq!(hex(encode(&[0xDC4D])), "EDB18D");
        assert_eq!(hex(encode(&[0x78, 0xDC4D])), "78EDB18D");
        assert_eq!(hex(encode(&[0xDC4D, 0xD83D, 0xDC4D])), "F0A390BDEDB18D");
        assert_eq!(hex(encode(&[0xD83D, 0xD83D])), "F09F90BD");
        assert_eq!(hex(encode(&[0x61, 0xD83D, 0xE9])), "61F09F93A9");
        assert_eq!(hex(encode(&[0x61, 0xD83D, 0x4E2D])), "61F09F98AD");
        assert_eq!(hex(encode(&[0xE9, 0xD83D, 0x71])), "C3A9F09F91B1");
        let valid: Vec<u16> = "👍 ok é 中".encode_utf16().collect();
        assert_eq!(encode(&valid), "👍 ok é 中".as_bytes());
    }

    #[test]
    fn a_cut_inside_an_emoji_keeps_half_of_it() {
        // 799 x's then an emoji: the 800-unit cut falls between its halves.
        let markdown = format!("# T\n\n---\n\n{}👍{}", "x".repeat(799), "y".repeat(100));
        let chunks = transcript_chunks(&markdown);
        assert_eq!(chunks[0].len(), 800);
        assert_eq!(chunks[0][799], 0xD83D);
        assert!(std::str::from_utf8(&bun_sqlite_text(&chunks[0])).is_err());
    }
}
