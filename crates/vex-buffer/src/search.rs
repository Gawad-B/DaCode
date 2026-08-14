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
///
/// Scanned **a line at a time**, straight out of the rope. It used to build one `String` of the
/// whole document — and a second, lowercased one for a case-insensitive query — on the UI thread,
/// for every character typed into the search box. On a large file that is two multi-megabyte
/// allocations per keystroke, which is the one thing this editor is not allowed to do.
///
/// A line is the right unit because the query is literal and comes from a single-line prompt: a
/// match cannot straddle a newline, so nothing is lost by not seeing across one. Most lines are
/// contiguous in a single rope chunk and are searched without any allocation at all; the rest cost
/// one line, never the document.
///
/// The honest limitation that remains: the *scan* is still linear in the document, so a rare query
/// in a very large file walks all of it. What is bounded now is memory, and — via
/// [`SearchOptions::limit`] — the result. Making the scan itself bounded means moving it to a
/// worker with a `Version` tag, like syntax and git; the shape is understood, it is simply not
/// this change.
pub fn find_all(text: &Text, query: &str, options: &SearchOptions) -> Vec<Range<CharOffset>> {
    if query.is_empty() {
        return Vec::new();
    }

    let sensitive = options.is_sensitive(query);
    let needle = if sensitive {
        query.to_string()
    } else {
        query.to_lowercase()
    };

    // A query that contains a newline genuinely does straddle lines, so it gets the old
    // whole-document scan. It cannot be typed into the search prompt, which is one line; it reaches
    // here only from a programmatic caller such as replace. Rare, explicit, and correct — rather
    // than silently finding nothing.
    if needle.contains('\n') {
        return find_across_lines(text, &needle, sensitive, options);
    }

    let mut matches = Vec::new();
    let mut joined = String::new();

    for line in 0..text.len_lines() {
        let slice = text.line(line);

        // `as_str` succeeds when the line lies in one chunk, which is the usual case. Otherwise one
        // allocation of a single line — reusing the same buffer — and never of the document.
        let contiguous = slice.as_str();
        if contiguous.is_none() {
            joined.clear();
            for chunk in slice.chunks() {
                joined.push_str(chunk);
            }
        }
        let raw = contiguous.unwrap_or(&joined);

        let base = text.line_to_char(line);
        if !find_in_line(raw, &needle, sensitive, base, options.limit, &mut matches) {
            break;
        }
    }

    matches
}

/// The whole-document scan, kept for the one case that needs it: a query spanning a newline.
///
/// This is what `find_all` used to do for every query, and the reason it is no longer the default:
/// it materialises the document, twice for a case-insensitive search.
fn find_across_lines(
    text: &Text,
    needle: &str,
    sensitive: bool,
    options: &SearchOptions,
) -> Vec<Range<CharOffset>> {
    let raw = text.to_string();
    let folded = (!sensitive).then(|| raw.to_lowercase());
    let haystack = match &folded {
        // Case folding that changes byte lengths would misplace every offset taken from the folded
        // copy. Nothing sensible is left to do for a multi-line query, so report nothing rather
        // than confident nonsense.
        Some(folded) if folded.len() != raw.len() => return Vec::new(),
        Some(folded) => folded.as_str(),
        None => raw.as_str(),
    };

    let mut matches = Vec::new();
    let mut from = 0usize;
    while let Some(found) = haystack[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();
        matches.push(
            text.byte_to_char(vex_text::ByteOffset(start))
                ..text.byte_to_char(vex_text::ByteOffset(end)),
        );
        if matches.len() >= options.limit {
            break;
        }
        from = start
            + haystack[start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        if from >= haystack.len() {
            break;
        }
    }
    matches
}

/// Appends the matches in one line. Returns false when the caller should stop: the limit is full.
fn find_in_line(
    raw: &str,
    needle: &str,
    sensitive: bool,
    base: CharOffset,
    limit: usize,
    matches: &mut Vec<Range<CharOffset>>,
) -> bool {
    // Case folding can change byte lengths (`İ` is the usual example), which would make offsets
    // taken from the folded copy point at the wrong place in the original. Only then is the slow
    // path needed, and now it is slow over one line rather than over the whole document.
    let folded = (!sensitive).then(|| raw.to_lowercase());
    let haystack = match &folded {
        Some(folded) if folded.len() == raw.len() => folded.as_str(),
        Some(folded) => return find_in_line_by_chars(raw, folded, needle, base, limit, matches),
        None => raw,
    };

    // Byte offsets from `find`, converted to char offsets by a cursor that only moves forward — so
    // a line with many matches stays linear rather than re-counting from the start each time.
    let (mut cursor_byte, mut cursor_char) = (0usize, 0usize);
    let mut from = 0usize;

    while let Some(found) = haystack[from..].find(needle) {
        let start = from + found;
        let end = start + needle.len();

        cursor_char += raw[cursor_byte..start].chars().count();
        cursor_byte = start;

        let start_char = cursor_char;
        let end_char = start_char + raw[start..end].chars().count();
        matches.push(CharOffset(base.get() + start_char)..CharOffset(base.get() + end_char));
        if matches.len() >= limit {
            return false;
        }

        // Advance one character past the start, so overlapping matches are still found.
        from = start
            + haystack[start..]
                .chars()
                .next()
                .map(char::len_utf8)
                .unwrap_or(1);
        if from >= haystack.len() {
            break;
        }
    }

    true
}

/// Character-by-character fallback for a line whose case folding changes its byte length.
fn find_in_line_by_chars(
    raw: &str,
    _folded: &str,
    needle_lower: &str,
    base: CharOffset,
    limit: usize,
    matches: &mut Vec<Range<CharOffset>>,
) -> bool {
    let chars: Vec<char> = raw.chars().collect();
    let needle: Vec<char> = needle_lower.chars().collect();
    if needle.is_empty() || chars.len() < needle.len() {
        return true;
    }

    for start in 0..=chars.len() - needle.len() {
        let candidate: String = chars[start..start + needle.len()]
            .iter()
            .flat_map(|c| c.to_lowercase())
            .collect();
        if candidate == needle_lower {
            matches.push(
                CharOffset(base.get() + start)..CharOffset(base.get() + start + needle.len()),
            );
            if matches.len() >= limit {
                return false;
            }
        }
    }

    true
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
