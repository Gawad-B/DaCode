//! Searching within a buffer.
//!
//! Separate from `vex-project`'s search, which walks the filesystem on a thread. This one runs on
//! the UI thread against the rope, so it must stay bounded: a search is either limited to the
//! visible range (for highlighting) or capped by a match count (for `n`/`N`).
//!
//! Literal, not regex. `/` in an editor is used overwhelmingly for literal text, and a regex engine
//! here would mean either a second dependency or surprising metacharacter behavior in the common
//! case. Project search already offers regex where it earns its keep.

use std::ops::Range;
use vex_text::{CharOffset, Text};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchOptions {
    /// `None` means smart case: insensitive unless the query has an uppercase letter — the same
    /// rule as project search, so the two behave alike.
    pub case_sensitive: Option<bool>,
    /// Cap on matches collected, so a search for `e` cannot allocate a vector per character in a
    /// large file.
    pub limit: usize,
}

impl Default for SearchOptions {
    fn default() -> Self {
        Self {
            case_sensitive: None,
            limit: 10_000,
        }
    }
}

impl SearchOptions {
    fn is_sensitive(&self, query: &str) -> bool {
        self.case_sensitive
            .unwrap_or_else(|| query.chars().any(char::is_uppercase))
    }
}

/// Every match in the document, in order.
pub fn find_all(text: &Text, query: &str, options: &SearchOptions) -> Vec<Range<CharOffset>> {
    if query.is_empty() {
        return Vec::new();
    }

    let sensitive = options.is_sensitive(query);
    // One allocation of the document for the scan. At the sizes an interactive search runs on this
    // is far cheaper than walking the rope character by character, and it happens per query rather
    // than per frame — matches are cached by the caller.
    let haystack = if sensitive {
        text.to_string()
    } else {
        text.to_string().to_lowercase()
    };
    let needle = if sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };

    // Lowercasing can change byte lengths (e.g. `İ`), which would make byte offsets from the
    // lowercased haystack point at the wrong place in the original. Fall back to a character scan
    // when that happens rather than reporting confident nonsense.
    if !sensitive && haystack.len() != text.len_bytes().get() {
        return find_all_by_chars(text, &needle, options);
    }

    let mut matches = Vec::new();
    let mut byte = 0usize;

    while let Some(found) = haystack[byte..].find(&needle) {
        let start = byte + found;
        let end = start + needle.len();

        matches.push(
            text.byte_to_char(vex_text::ByteOffset(start))
                ..text.byte_to_char(vex_text::ByteOffset(end)),
        );
        if matches.len() >= options.limit {
            break;
        }
        // Advance by one byte boundary past the start, so overlapping matches are still found.
        byte = start
            + haystack[start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        if byte >= haystack.len() {
            break;
        }
    }

    matches
}

/// Character-by-character fallback for text where case folding changes lengths.
fn find_all_by_chars(
    text: &Text,
    needle_lower: &str,
    options: &SearchOptions,
) -> Vec<Range<CharOffset>> {
    let chars: Vec<char> = text.to_string().chars().collect();
    let needle: Vec<char> = needle_lower.chars().collect();
    let mut matches = Vec::new();

    if needle.is_empty() || chars.len() < needle.len() {
        return matches;
    }

    for start in 0..=chars.len() - needle.len() {
        let candidate = chars[start..start + needle.len()]
            .iter()
            .flat_map(|c| c.to_lowercase())
            .collect::<String>();
        if candidate == needle_lower {
            matches.push(CharOffset(start)..CharOffset(start + needle.len()));
            if matches.len() >= options.limit {
                break;
            }
        }
    }

    matches
}

/// The next match at or after `from`, wrapping to the start of the document.
///
/// Wrapping is what makes `n` usable: a search that stops at the end of the file forces you to
/// jump back manually every time.
pub fn find_next(
    matches: &[Range<CharOffset>],
    from: CharOffset,
    forward: bool,
) -> Option<Range<CharOffset>> {
    if matches.is_empty() {
        return None;
    }

    if forward {
        matches
            .iter()
            .find(|range| range.start > from)
            .or_else(|| matches.first())
            .cloned()
    } else {
        matches
            .iter()
            .rev()
            .find(|range| range.start < from)
            .or_else(|| matches.last())
            .cloned()
    }
}

/// Which match contains or starts at `offset`, for the `3 of 12` counter.
pub fn index_of(matches: &[Range<CharOffset>], offset: CharOffset) -> Option<usize> {
    matches
        .iter()
        .position(|range| range.start <= offset && offset < range.end)
}
