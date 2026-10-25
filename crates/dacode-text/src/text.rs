//! The document's text storage: a rope, plus every coordinate conversion.

use crate::coords::{ByteOffset, CharOffset, Point, Utf16Column};
use ropey::{Rope, RopeSlice};
use std::ops::Range;
use unicode_segmentation::GraphemeCursor;

/// Immutable-cloneable text storage.
///
/// Cloning is O(log n) and shares structure, which is what makes the "60fps contract" possible:
/// background services (syntax, LSP, search) operate on a cheap [`Text`] snapshot taken on the
/// UI thread, so they never need to lock or block the editor while it keeps accepting input.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct Text {
    rope: Rope,
}

impl Text {
    pub fn new() -> Self {
        Self::default()
    }

    // Named `from_str` deliberately for symmetry with `Rope::from_str`, which is the call this
    // wraps; `FromStr` is the wrong trait here because loading text is infallible.
    #[allow(clippy::should_implement_trait)]
    pub fn from_str(s: &str) -> Self {
        Self {
            rope: Rope::from_str(s),
        }
    }

    pub fn rope(&self) -> &Rope {
        &self.rope
    }

    pub fn slice(&self, range: Range<CharOffset>) -> RopeSlice<'_> {
        self.rope.slice(range.start.get()..range.end.get())
    }

    pub fn slice_to_string(&self, range: Range<CharOffset>) -> String {
        self.slice(range).to_string()
    }

    // ---- sizes -------------------------------------------------------------------------

    pub fn len_chars(&self) -> CharOffset {
        CharOffset(self.rope.len_chars())
    }

    pub fn len_bytes(&self) -> ByteOffset {
        ByteOffset(self.rope.len_bytes())
    }

    /// Number of lines. A document always has at least one line, and a trailing newline does
    /// **not** create a further line here (ropey counts it as one, matching editor intuition
    /// only after this adjustment — so be explicit and use [`Self::last_line`] for clamping).
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    pub fn last_line(&self) -> usize {
        self.rope.len_lines().saturating_sub(1)
    }

    pub fn is_empty(&self) -> bool {
        self.rope.len_chars() == 0
    }

    // ---- lines -------------------------------------------------------------------------

    /// The line's content **including** its trailing line break, if any.
    pub fn line(&self, line: usize) -> RopeSlice<'_> {
        self.rope.line(line.min(self.last_line()))
    }

    /// Length of a line in chars, excluding the trailing line break.
    pub fn line_len(&self, line: usize) -> usize {
        let slice = self.line(line);
        let mut len = slice.len_chars();
        // Strip \n then \r so both LF and CRLF report the same visible length.
        if len > 0 && slice.char(len - 1) == '\n' {
            len -= 1;
            if len > 0 && slice.char(len - 1) == '\r' {
                len -= 1;
            }
        }
        len
    }

    pub fn line_range(&self, line: usize) -> Range<CharOffset> {
        let start = self.line_to_char(line);
        CharOffset(start.get())..CharOffset(start.get() + self.line_len(line))
    }

    // ---- conversions -------------------------------------------------------------------

    pub fn char_to_byte(&self, offset: CharOffset) -> ByteOffset {
        ByteOffset(self.rope.char_to_byte(self.clamp(offset).get()))
    }

    pub fn byte_to_char(&self, offset: ByteOffset) -> CharOffset {
        let b = offset.get().min(self.rope.len_bytes());
        CharOffset(self.rope.byte_to_char(b))
    }

    pub fn char_to_line(&self, offset: CharOffset) -> usize {
        self.rope.char_to_line(self.clamp(offset).get())
    }

    pub fn line_to_char(&self, line: usize) -> CharOffset {
        CharOffset(self.rope.line_to_char(line.min(self.last_line())))
    }

    pub fn char_to_point(&self, offset: CharOffset) -> Point {
        let offset = self.clamp(offset);
        let line = self.rope.char_to_line(offset.get());
        let line_start = self.rope.line_to_char(line);
        Point::new(line, offset.get() - line_start)
    }

    /// Clamps out-of-range points into the document, and columns into their line, so callers
    /// can convert a remembered position after the text has shrunk without special-casing.
    pub fn point_to_char(&self, point: Point) -> CharOffset {
        let line = point.line.min(self.last_line());
        let line_start = self.rope.line_to_char(line);
        let column = point.column.min(self.line_len(line));
        CharOffset(line_start + column)
    }

    /// LSP positions are `(line, UTF-16 code units into that line)`.
    pub fn point_to_lsp(&self, point: Point) -> (usize, Utf16Column) {
        let line = point.line.min(self.last_line());
        let slice = self.line(line);
        let column = point.column.min(slice.len_chars());
        (line, Utf16Column(slice.char_to_utf16_cu(column)))
    }

    pub fn lsp_to_point(&self, line: usize, column: Utf16Column) -> Point {
        let line = line.min(self.last_line());
        let slice = self.line(line);
        let cu = column.get().min(slice.len_utf16_cu());
        Point::new(line, slice.utf16_cu_to_char(cu))
    }

    #[inline]
    pub fn clamp(&self, offset: CharOffset) -> CharOffset {
        CharOffset(offset.get().min(self.rope.len_chars()))
    }

    // ---- grapheme boundaries -----------------------------------------------------------
    //
    // Cursor motion moves by *grapheme cluster*, not by char: "e" + combining acute, a
    // family emoji, and a Devanagari cluster must each move as one unit.

    pub fn next_grapheme(&self, offset: CharOffset) -> CharOffset {
        self.grapheme_boundary(offset, true)
    }

    pub fn prev_grapheme(&self, offset: CharOffset) -> CharOffset {
        self.grapheme_boundary(offset, false)
    }

    fn grapheme_boundary(&self, offset: CharOffset, forward: bool) -> CharOffset {
        let offset = self.clamp(offset);
        let byte = self.rope.char_to_byte(offset.get());
        let (chunk, chunk_byte, _, _) = self.rope.chunk_at_byte(byte);
        let mut cursor = GraphemeCursor::new(byte, self.rope.len_bytes(), true);

        let result = if forward {
            cursor.next_boundary(chunk, chunk_byte)
        } else {
            cursor.prev_boundary(chunk, chunk_byte)
        };

        match result {
            Ok(Some(b)) => CharOffset(self.rope.byte_to_char(b)),
            // Boundary lies outside the chunk we handed the cursor; fall back to whole text.
            // Rare (only at chunk edges for multi-chunk clusters) and correct if slower.
            Err(_) => {
                let s = self.rope.to_string();
                let mut cursor = GraphemeCursor::new(byte, s.len(), true);
                let b = if forward {
                    cursor.next_boundary(&s, 0).ok().flatten()
                } else {
                    cursor.prev_boundary(&s, 0).ok().flatten()
                };
                CharOffset(self.rope.byte_to_char(b.unwrap_or(byte)))
            }
            Ok(None) => offset, // start or end of document
        }
    }

    // ---- mutation ----------------------------------------------------------------------
    //
    // Deliberately crate-private: all mutation must go through a `Transaction` so that undo
    // and background invalidation see every change. See `crate::edit`.

    pub(crate) fn replace(&mut self, range: Range<CharOffset>, replacement: &str) {
        let range = range.start.get()..range.end.get();
        if range.start != range.end {
            self.rope.remove(range.clone());
        }
        if !replacement.is_empty() {
            self.rope.insert(range.start, replacement);
        }
    }
}

impl std::fmt::Display for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.rope)
    }
}

impl std::fmt::Debug for Text {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Text({:?})", self.rope.to_string())
    }
}
