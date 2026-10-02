//! `transcriptChunks` from the TS CLI's `src/search/chunks.ts` @ 1b8c950:
//! chunk lengths and overlaps are counted in UTF-16 code units, as there.
//! Unlike there, a cut never falls inside an emoji's surrogate pair, so
//! every chunk is valid text.

/// Bump when the text sent to FTS changes. Chunks of another version are
/// stale: the indexer rebuilds them from the cached transcripts, without
/// fetching, and the embedder embeds the new ones.
///
/// 2: cuts no longer split a surrogate pair (version 1 kept half an emoji,
/// stored as the bytes Bun gave SQLite for it, for parity with the TS CLI).
pub const CHUNK_VERSION: u32 = 2;
const MAX_CHUNK_CHARS: usize = 800;
const OVERLAP_CHARS: usize = 80;

/// The turn separator `renderTranscript` puts between sections, as
/// `transcriptChunks` splits on it (`/\n---\n\n/`).
const TURN_SEPARATOR: &str = "\n---\n\n";

/// A transcript's chunks: every turn and canvas (the header dropped) cut
/// into pieces of at most 800 UTF-16 code units, preferably at a space or
/// line break, overlapping by 80, never inside a surrogate pair.
pub fn transcript_chunks(markdown: &str) -> Vec<String> {
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
                } else if is_high_surrogate(text[end - 1]) {
                    // Keep the pair whole: in the next chunk, or (when it
                    // would be all this one holds) in this one.
                    end = if end - 1 > start { end - 1 } else { end + 1 };
                }
            }
            let chunk = trim_units(&text[start..end]);
            if !chunk.is_empty() {
                chunks.push(String::from_utf16_lossy(chunk));
            }
            if end == text.len() {
                break;
            }
            start = (start + 1).max(end.saturating_sub(OVERLAP_CHARS));
            if is_low_surrogate(text[start]) {
                start += 1;
            }
        }
    }
    if chunks.is_empty() {
        vec![chatgpt_core::js::trim(markdown).to_owned()]
    } else {
        chunks
    }
}

fn is_high_surrogate(unit: u16) -> bool {
    (0xD800..0xDC00).contains(&unit)
}

fn is_low_surrogate(unit: u16) -> bool {
    (0xDC00..0xE000).contains(&unit)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn turns_are_chunked_without_the_header() {
        let markdown = "# Title\n\nhttps://chatgpt.com/c/x · 2026-01-01 · gpt\n\n---\n\n## Me\n\nhello\n\n---\n\n## ChatGPT\n\n  hi there  \n";
        let chunks = transcript_chunks(markdown);
        assert_eq!(chunks, ["## Me\n\nhello", "## ChatGPT\n\n  hi there"]);
        assert_eq!(transcript_chunks("  just a header ")[0], "just a header");
    }

    #[test]
    fn long_turns_split_at_a_space_and_overlap() {
        let words: String = (0..300).map(|n| format!("w{n:03} ")).collect();
        let markdown = format!("# T\n\n---\n\n{words}");
        let chunks = transcript_chunks(&markdown);
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
        let lengths: Vec<usize> = transcript_chunks(&markdown)
            .iter()
            .map(String::len)
            .collect();
        assert_eq!(lengths, [800, 800, 260]);
    }

    #[test]
    fn a_cut_never_splits_an_emoji() {
        let units = |text: &str| text.encode_utf16().count();
        // 799 x's then an emoji: the 800-unit cut would fall between its
        // halves, so the emoji goes whole into the next chunk.
        let markdown = format!("# T\n\n---\n\n{}👍{}", "x".repeat(799), "y".repeat(100));
        let chunks = transcript_chunks(&markdown);
        assert_eq!(chunks[0], "x".repeat(799));
        assert!(chunks[1].ends_with(&format!("👍{}", "y".repeat(100))));
        // An overlap that would start on an emoji's second half starts
        // after it.
        let markdown = format!(
            "# T\n\n---\n\n{}{}{}",
            "x".repeat(719),
            "👍".repeat(2),
            "y".repeat(200)
        );
        for chunk in transcript_chunks(&markdown) {
            assert!(units(&chunk) <= MAX_CHUNK_CHARS, "{}", units(&chunk));
        }
        // Emoji all the way: every cut still lands between pairs.
        let markdown = format!("# T\n\n---\n\n{}", "👍".repeat(900));
        let chunks = transcript_chunks(&markdown);
        assert!(chunks.iter().all(|chunk| chunk.chars().all(|c| c == '👍')));
        assert!(chunks.iter().all(|chunk| units(chunk) <= MAX_CHUNK_CHARS));
    }
}
