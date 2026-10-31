//! Resolving motions and text objects against real text.
//!
//! Separated from the state machine because these are *document queries*, not key handling:
//! they are testable without any notion of keys, and they are what an operator consumes.
//!
//! Vertical motion is absent here on purpose — it depends on soft-wrap geometry, which lives in
//! `dacode-display`. The caller routes `Motion::Up`/`Down` there.

use crate::action::{Motion, TextObject};
use dacode_text::{CharOffset, Point, Text};
use std::ops::Range;

/// Character classes for word motion. Vim's rule: a word is a run of one class, and switching
/// class starts a new word — which is why `foo.bar` is three words, not one.
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum Class {
    Whitespace,
    Word,
    Punctuation,
}

fn class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::Whitespace
    } else if c.is_alphanumeric() || c == '_' {
        Class::Word
    } else {
        Class::Punctuation
    }
}

/// `W`/`B`/`E` treat everything non-whitespace as one class.
fn big_class_of(c: char) -> Class {
    if c.is_whitespace() {
        Class::Whitespace
    } else {
        Class::Word
    }
}

/// Resolves a horizontal or absolute motion into a position.
///
/// `Motion::Up`/`Down` return `from` unchanged: they need display geometry, so the caller must
/// route them to `dacode-display` instead.
pub fn resolve_motion(text: &Text, from: CharOffset, motion: Motion, count: usize) -> CharOffset {
    let count = count.max(1);
    let mut at = from;

    for _ in 0..count {
        at = resolve_once(text, at, motion);
    }
    at
}

fn resolve_once(text: &Text, from: CharOffset, motion: Motion) -> CharOffset {
    let len = text.len_chars();
    let point = text.char_to_point(from);

    match motion {
        Motion::Left => text.prev_grapheme(from),
        Motion::Right => text.next_grapheme(from),
        // Vertical motion belongs to the display layer.
        Motion::Up | Motion::Down | Motion::PageUp | Motion::PageDown => from,
        Motion::SmartHome => {
            let first = resolve_once(text, from, Motion::FirstNonBlank);
            if from == first {
                text.point_to_char(Point::new(point.line, 0))
            } else {
                first
            }
        }

        Motion::LineStart => text.point_to_char(Point::new(point.line, 0)),
        Motion::LineEnd => text.point_to_char(Point::new(point.line, text.line_len(point.line))),
        Motion::FirstNonBlank => {
            let line_start = text.line_to_char(point.line);
            let length = text.line_len(point.line);
            let content = text.slice_to_string(line_start..CharOffset(line_start.get() + length));
            let leading = content.chars().take_while(|c| c.is_whitespace()).count();
            CharOffset(line_start.get() + leading)
        }

        Motion::FileStart => CharOffset::ZERO,
        Motion::FileEnd => text.point_to_char(Point::new(text.last_line(), 0)),
        Motion::ToLine(line) => text.point_to_char(Point::new(line.min(text.last_line()), 0)),

        Motion::WordForward => word_forward(text, from, false),
        Motion::WordForwardBig => word_forward(text, from, true),
        Motion::WordBackward => word_backward(text, from, false),
        Motion::WordBackwardBig => word_backward(text, from, true),
        Motion::WordEnd => word_end(text, from, false),
        Motion::WordEndBig => word_end(text, from, true),

        Motion::FindChar {
            target,
            forward,
            till,
        } => find_char(text, from, target, forward, till),

        Motion::MatchingBracket => matching_bracket(text, from).unwrap_or(from),

        Motion::ParagraphForward => paragraph(text, point.line, true, len),
        Motion::ParagraphBackward => paragraph(text, point.line, false, len),
    }
}

fn classify(text: &Text, index: usize, big: bool) -> Option<Class> {
    if index >= text.len_chars().get() {
        return None;
    }
    let c = text.rope().char(index);
    Some(if big { big_class_of(c) } else { class_of(c) })
}

fn word_forward(text: &Text, from: CharOffset, big: bool) -> CharOffset {
    let len = text.len_chars().get();
    let mut index = from.get();
    let Some(start_class) = classify(text, index, big) else {
        return from;
    };

    // Leave the current run...
    if start_class != Class::Whitespace {
        while index < len && classify(text, index, big) == Some(start_class) {
            index += 1;
        }
    }
    // ...then skip whitespace to the start of the next word.
    while index < len && classify(text, index, big) == Some(Class::Whitespace) {
        index += 1;
    }
    CharOffset(index)
}

fn word_backward(text: &Text, from: CharOffset, big: bool) -> CharOffset {
    let mut index = from.get();
    if index == 0 {
        return from;
    }
    index -= 1;

    while index > 0 && classify(text, index, big) == Some(Class::Whitespace) {
        index -= 1;
    }
    let Some(target_class) = classify(text, index, big) else {
        return CharOffset(index);
    };
    if target_class == Class::Whitespace {
        return CharOffset(index);
    }

    while index > 0 && classify(text, index - 1, big) == Some(target_class) {
        index -= 1;
    }
    CharOffset(index)
}

fn word_end(text: &Text, from: CharOffset, big: bool) -> CharOffset {
    let len = text.len_chars().get();
    let mut index = from.get();
    if index >= len {
        return from;
    }

    index += 1;
    while index < len && classify(text, index, big) == Some(Class::Whitespace) {
        index += 1;
    }
    let Some(target_class) = classify(text, index, big) else {
        return CharOffset(index.min(len));
    };
    while index + 1 < len && classify(text, index + 1, big) == Some(target_class) {
        index += 1;
    }
    CharOffset(index)
}

/// `f`/`F`/`t`/`T`. Searches within the current line only, as in Vim — a find that jumps lines
/// makes `dt)` unpredictable.
fn find_char(text: &Text, from: CharOffset, target: char, forward: bool, till: bool) -> CharOffset {
    let point = text.char_to_point(from);
    let line_start = text.line_to_char(point.line).get();
    let line_end = line_start + text.line_len(point.line);
    let content: Vec<char> = text
        .slice_to_string(CharOffset(line_start)..CharOffset(line_end))
        .chars()
        .collect();
    let cursor = from.get() - line_start;

    if forward {
        for (offset, c) in content.iter().enumerate().skip(cursor + 1) {
            if *c == target {
                let landing = if till { offset - 1 } else { offset };
                return CharOffset(line_start + landing);
            }
        }
    } else {
        for offset in (0..cursor).rev() {
            if content[offset] == target {
                let landing = if till { offset + 1 } else { offset };
                return CharOffset(line_start + landing);
            }
        }
    }

    from
}

const PAIRS: [(char, char); 4] = [('(', ')'), ('[', ']'), ('{', '}'), ('<', '>')];

/// `%` — nesting-aware, so it lands on the *matching* bracket rather than the next one.
fn matching_bracket(text: &Text, from: CharOffset) -> Option<CharOffset> {
    let len = text.len_chars().get();
    let mut index = from.get();

    // Find the bracket at or after the cursor, on this line.
    let point = text.char_to_point(from);
    let line_end = text.line_to_char(point.line).get() + text.line_len(point.line);
    let (open, close, forward) = loop {
        if index >= line_end || index >= len {
            return None;
        }
        let c = text.rope().char(index);
        if let Some(&(open, close)) = PAIRS.iter().find(|(open, _)| *open == c) {
            break (open, close, true);
        }
        if let Some(&(open, close)) = PAIRS.iter().find(|(_, close)| *close == c) {
            break (open, close, false);
        }
        index += 1;
    };

    let mut depth = 0isize;
    if forward {
        for scan in index..len {
            let c = text.rope().char(scan);
            if c == open {
                depth += 1;
            } else if c == close {
                depth -= 1;
                if depth == 0 {
                    return Some(CharOffset(scan));
                }
            }
        }
    } else {
        for scan in (0..=index).rev() {
            let c = text.rope().char(scan);
            if c == close {
                depth += 1;
            } else if c == open {
                depth -= 1;
                if depth == 0 {
                    return Some(CharOffset(scan));
                }
            }
        }
    }
    None
}

fn paragraph(text: &Text, line: usize, forward: bool, len: CharOffset) -> CharOffset {
    let is_blank = |line: usize| text.line_len(line) == 0;
    let last = text.last_line();
    let mut current = line;

    if forward {
        while current < last && is_blank(current) {
            current += 1;
        }
        while current < last && !is_blank(current) {
            current += 1;
        }
        if current >= last {
            return len;
        }
    } else {
        while current > 0 && is_blank(current) {
            current -= 1;
        }
        while current > 0 && !is_blank(current) {
            current -= 1;
        }
    }

    text.point_to_char(Point::new(current, 0))
}

/// The range an operator should act on for a motion, honoring inclusive/exclusive and linewise
/// semantics. `dw` excludes the landing character; `de` includes it; `dj` takes whole lines.
pub fn operator_range(
    text: &Text,
    from: CharOffset,
    to: CharOffset,
    motion: Motion,
) -> Range<CharOffset> {
    if motion.is_linewise() {
        let (first, last) = if from <= to {
            (text.char_to_line(from), text.char_to_line(to))
        } else {
            (text.char_to_line(to), text.char_to_line(from))
        };
        return line_range(text, first, last);
    }

    let (start, mut end) = if from <= to { (from, to) } else { (to, from) };
    if motion.is_inclusive() {
        end = text.next_grapheme(end);
    }
    start..end
}

/// Whole lines `first..=last`, including the trailing newline so `dd` removes the line rather
/// than leaving a blank one behind.
pub fn line_range(text: &Text, first: usize, last: usize) -> Range<CharOffset> {
    let start = text.line_to_char(first);
    let last = last.min(text.last_line());
    let line = text.line_range(last);
    let end = if last < text.last_line() {
        // Include the line break.
        text.line_to_char(last + 1)
    } else {
        line.end
    };
    start..end
}

/// Resolves a text object into a range.
pub fn resolve_object(
    text: &Text,
    at: CharOffset,
    object: TextObject,
) -> Option<Range<CharOffset>> {
    match object {
        TextObject::Word { around, big } => Some(word_object(text, at, around, big)),
        TextObject::Quote { delimiter, around } => quote_object(text, at, delimiter, around),
        TextObject::Pair {
            open,
            close,
            around,
        } => pair_object(text, at, open, close, around),
        TextObject::Paragraph { around } => Some(paragraph_object(text, at, around)),
        TextObject::Line { around } => {
            let line = text.char_to_line(at);
            Some(if around {
                line_range(text, line, line)
            } else {
                text.line_range(line)
            })
        }
        // Needs the syntax tree, which the editor holds and this crate does not.
        TextObject::Syntax { .. } => None,
    }
}

fn word_object(text: &Text, at: CharOffset, around: bool, big: bool) -> Range<CharOffset> {
    let len = text.len_chars().get();
    let mut start = at.get().min(len.saturating_sub(1));
    let Some(target) = classify(text, start, big) else {
        return at..at;
    };

    while start > 0 && classify(text, start - 1, big) == Some(target) {
        start -= 1;
    }
    let mut end = at.get();
    while end < len && classify(text, end, big) == Some(target) {
        end += 1;
    }

    if around {
        // `aw` takes the trailing whitespace as well.
        while end < len && classify(text, end, big) == Some(Class::Whitespace) {
            end += 1;
        }
    }

    CharOffset(start)..CharOffset(end)
}

fn quote_object(
    text: &Text,
    at: CharOffset,
    delimiter: char,
    around: bool,
) -> Option<Range<CharOffset>> {
    // Quotes do not nest, so scan the line and pick the pair containing the cursor.
    let point = text.char_to_point(at);
    let line_start = text.line_to_char(point.line).get();
    let content: Vec<char> = text
        .slice_to_string(text.line_range(point.line))
        .chars()
        .collect();

    let positions: Vec<usize> = content
        .iter()
        .enumerate()
        .filter(|(_, c)| **c == delimiter)
        .map(|(i, _)| i)
        .collect();

    let cursor = at.get() - line_start;
    for pair in positions.chunks(2) {
        if pair.len() < 2 {
            break;
        }
        let (open, close) = (pair[0], pair[1]);
        if cursor >= open && cursor <= close {
            return Some(if around {
                CharOffset(line_start + open)..CharOffset(line_start + close + 1)
            } else {
                CharOffset(line_start + open + 1)..CharOffset(line_start + close)
            });
        }
    }
    None
}

fn pair_object(
    text: &Text,
    at: CharOffset,
    open: char,
    close: char,
    around: bool,
) -> Option<Range<CharOffset>> {
    let len = text.len_chars().get();
    let cursor = at.get();

    // Walk backward for the unmatched opening delimiter, then forward for its partner. This is
    // what makes `di(` work from anywhere inside nested calls.
    let mut depth = 0isize;
    let mut start = None;
    for scan in (0..=cursor.min(len.saturating_sub(1))).rev() {
        let c = text.rope().char(scan);
        if c == close && scan != cursor {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                start = Some(scan);
                break;
            }
            depth -= 1;
        }
    }
    let start = start?;

    depth = 0;
    let mut end = None;
    for scan in start..len {
        let c = text.rope().char(scan);
        if c == open {
            depth += 1;
        } else if c == close {
            depth -= 1;
            if depth == 0 {
                end = Some(scan);
                break;
            }
        }
    }
    let end = end?;

    Some(if around {
        CharOffset(start)..CharOffset(end + 1)
    } else {
        CharOffset(start + 1)..CharOffset(end)
    })
}

fn paragraph_object(text: &Text, at: CharOffset, around: bool) -> Range<CharOffset> {
    let line = text.char_to_line(at);
    let is_blank = |line: usize| text.line_len(line) == 0;
    let last = text.last_line();

    let mut first = line;
    while first > 0 && !is_blank(first - 1) {
        first -= 1;
    }
    let mut end = line;
    while end < last && !is_blank(end + 1) {
        end += 1;
    }
    if around {
        while end < last && is_blank(end + 1) {
            end += 1;
        }
    }

    line_range(text, first, end)
}
